//! Run a detailed review through the user's own AI assistant, over MCP.
//!
//! This is surface B of the review plan, driven from the app instead of from a
//! terminal: SpinZero writes an MCP config naming its own review server, spawns the
//! `claude` CLI with it, and the assistant's model does the reasoning on the user's
//! own subscription. We supply the workflow, the evidence, the ordering and the
//! validation; they supply the tokens.
//!
//! Three properties make this worth a whole module rather than a shell-out:
//!
//! * **Nothing about the design leaves the machine.** The MCP server runs locally and
//!   only looks up manufacturer part numbers. The pre-flight says so, and the claim
//!   has to stay literally true.
//! * **The config is ours, not theirs.** `--strict-mcp-config` means the spawned
//!   assistant sees exactly one server: this one. A review cannot wander into the
//!   user's other tools, and a user who has never run `claude mcp add` still gets a
//!   working review.
//! * **The findings come back through the drop-box.** The assistant writes
//!   `findings.json` into `<project>/reviews/inbox/`, and the user imports it from the
//!   review launcher exactly as they would a review produced any other way. There is
//!   no second ingestion path (see `bomcheck::inbox_dir`).
//!
//! The subprocess is deliberately not given a way to touch anything else:
//! `--allowedTools mcp__spinzero` is the entire tool surface it is permitted.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};


/// Progress from a running agent review, streamed to the frontend as `agent-event`.
///
/// Deliberately coarse. The assistant's own narration is its business and most of it
/// is not useful to a hardware engineer watching a progress bar; what the app needs
/// is that something is happening, roughly where it has got to, and what went wrong
/// if anything did.
#[derive(Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AgentEvent {
    Started { assistant: String },
    /// One line of the assistant's output, already trimmed. Never a finding.
    Progress { line: String },
    /// The assistant finished. The findings, if any, are in the review inbox.
    Finished { seconds: u64 },
    Failed { detail: String },
}

fn emit(app: &AppHandle, ev: AgentEvent) {
    let _ = app.emit("agent-event", ev);
}

/// Where the `claude` binary and this app's MCP server live on this machine.
///
/// Both are settings rather than constants because neither has a location we can
/// assume: `claude` is wherever the user's package manager put it, and until the
/// server ships as a bundled binary (M2) it is a checkout path. An empty `claude_bin`
/// means "whatever is on PATH", which is the common case.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct AgentConfig {
    /// Path to the `claude` executable, or empty for PATH lookup.
    #[serde(default)]
    pub claude_bin: String,
    /// Command that starts the SpinZero MCP server, e.g. "node".
    #[serde(default)]
    pub server_command: String,
    /// Arguments for it, e.g. ["/path/to/mcp/src/server.ts"].
    #[serde(default)]
    pub server_args: Vec<String>,
    /// Extra environment for the server process: credentials, binary paths.
    #[serde(default)]
    pub server_env: BTreeMap<String, String>,
}

impl AgentConfig {
    /// Is there enough here to start a review? Reported to the UI so the option can
    /// be offered as "needs setting up" rather than failing on click.
    pub fn is_configured(&self) -> bool {
        !self.server_command.trim().is_empty() && !self.server_args.is_empty()
    }
}

/// The MCP config file handed to the assistant. Written into the run's own scratch
/// directory, never into the project folder.
fn write_mcp_config(dir: &Path, cfg: &AgentConfig) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let env: serde_json::Map<String, serde_json::Value> = cfg
        .server_env
        .iter()
        .map(|(k, v)| (k.clone(), serde_json::Value::String(v.clone())))
        .collect();
    let doc = serde_json::json!({
        "mcpServers": {
            "spinzero": {
                "command": cfg.server_command,
                "args": cfg.server_args,
                "env": env,
            }
        }
    });
    let path = dir.join("mcp-config.json");
    std::fs::write(&path, serde_json::to_vec_pretty(&doc).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    Ok(path)
}

/// What we ask the assistant to do.
///
/// Short on purpose. The server's own `instructions` and the `next` field on every
/// tool result carry the workflow; repeating it here would give the model two
/// sources of truth about a process only one of them can actually see. What this
/// prompt has to do is name the project, name the profile, and insist on the two
/// things a wandering client gets wrong: following `next`, and accounting for every
/// part.
fn prompt(project_dir: &Path, profile: &str) -> String {
    format!(
        "Run a SpinZero BOM review of {} with the {} profile.\n\n\
         Use the spinzero MCP tools and follow the `next` field on every result until the review \
         is finished. Account for every part in every batch, including the ones with nothing \
         wrong. Do not claim to have read a datasheet the server did not obtain.\n\n\
         When it is done, report where the findings landed and repeat the coverage numbers \
         verbatim, including anything the review could not check.",
        project_dir.display(),
        profile
    )
}

/// A running agent review. One at a time per app: two assistants reviewing the same
/// board would race on the drop-box and file two sets of comments for one board.
#[derive(Default)]
pub struct AgentRun {
    running: Arc<AtomicBool>,
}

impl AgentRun {
    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    /// Spawn the assistant and stream its output. Returns as soon as the process is
    /// up; everything after that arrives as `agent-event`.
    pub fn start(
        &self,
        app: AppHandle,
        project_dir: PathBuf,
        scratch: PathBuf,
        profile: String,
        cfg: AgentConfig,
    ) -> Result<(), String> {
        if !cfg.is_configured() {
            return Err(
                "the AI assistant review is not set up yet: tell SpinZero how to start its MCP \
                 server in Settings."
                    .into(),
            );
        }
        if self.running.swap(true, Ordering::SeqCst) {
            return Err("a review is already running through your assistant.".into());
        }
        let running = self.running.clone();
        let config_path = write_mcp_config(&scratch, &cfg).inspect_err(|_| {
            running.store(false, Ordering::SeqCst);
        })?;

        let bin = if cfg.claude_bin.trim().is_empty() {
            "claude".to_string()
        } else {
            cfg.claude_bin.clone()
        };

        std::thread::spawn(move || {
            let started = std::time::Instant::now();
            emit(&app, AgentEvent::Started { assistant: bin.clone() });

            let spawned = Command::new(&bin)
                .arg("-p")
                .arg(prompt(&project_dir, &profile))
                .arg("--mcp-config")
                .arg(&config_path)
                // Exactly one server, and exactly one family of tools. A review must
                // not be able to reach the user's other MCP servers, their shell or
                // their filesystem: everything it is allowed to do goes through the
                // harness, which is the whole design.
                .arg("--strict-mcp-config")
                .arg("--allowedTools")
                .arg("mcp__spinzero")
                .current_dir(&project_dir)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .stdin(Stdio::null())
                .spawn();

            let mut child = match spawned {
                Ok(c) => c,
                Err(e) => {
                    log::warn!("agent review could not start {bin}: {e}");
                    emit(
                        &app,
                        AgentEvent::Failed {
                            detail: format!(
                                "could not start {bin}: {e}. Is the Claude CLI installed and on PATH?"
                            ),
                        },
                    );
                    running.store(false, Ordering::SeqCst);
                    return;
                }
            };

            if let Some(out) = child.stdout.take() {
                for line in BufReader::new(out).lines().map_while(Result::ok) {
                    let line = line.trim().to_string();
                    if !line.is_empty() {
                        emit(&app, AgentEvent::Progress { line });
                    }
                }
            }

            // stderr is the server's own diagnostics plus the CLI's. Kept for the log
            // only: it is where a misconfiguration explains itself, and it is not
            // something to put in front of an engineer mid-review.
            let mut tail = String::new();
            if let Some(err) = child.stderr.take() {
                for line in BufReader::new(err).lines().map_while(Result::ok) {
                    log::info!("{} agent review: {line}", crate::telemetry::LOCAL_ONLY);
                    tail.push_str(&line);
                    tail.push('\n');
                    if tail.len() > 4000 {
                        let cut = tail.len() - 2000;
                        tail = tail.split_off(cut);
                    }
                }
            }

            let status = child.wait();
            running.store(false, Ordering::SeqCst);
            let seconds = started.elapsed().as_secs();
            match status {
                Ok(s) if s.success() => {
                    log::info!("agent review finished in {seconds}s");
                    emit(&app, AgentEvent::Finished { seconds });
                }
                Ok(s) => {
                    log::warn!("agent review exited {s}");
                    emit(
                        &app,
                        AgentEvent::Failed {
                            detail: format!(
                                "your assistant exited without finishing ({s}). {}",
                                tail.lines().last().unwrap_or_default()
                            ),
                        },
                    );
                }
                Err(e) => emit(&app, AgentEvent::Failed { detail: e.to_string() }),
            }
        });
        Ok(())
    }
}
