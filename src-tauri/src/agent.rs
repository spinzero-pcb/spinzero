//! Run a BOM review through the user's own AI agent, over MCP.
//!
//! An agent is a command line program that drives a model and speaks MCP. SpinZero
//! starts one, hands it a prompt, and gets out of the way. The agent calls the
//! SpinZero review server, which the USER registered with it, and the findings come
//! back through the review drop-box (`bomcheck::inbox_dir`) like every review that
//! ran outside this window.
//!
//! Three rules shape this module, and each one replaced something that used to be
//! here:
//!
//! * **Any agent, not one.** The binary, the arguments and the way the prompt is
//!   handed over are a PROFILE. Claude Code is one of them, not the assumption.
//! * **We add no flags of our own.** The old code passed `--mcp-config`,
//!   `--strict-mcp-config` and `--allowedTools mcp__spinzero`. The last of those
//!   blocked the sub-agents the review server asks for, so the app broke the review
//!   it was starting. Permissions, sub-agents and MCP registration belong to the
//!   agent and to its owner.
//! * **We answer the questions we already hold.** The server stops and asks for the
//!   end application and the column mapping. SpinZero knows both. The prompt carries
//!   them, and tells the agent that nobody is there to be asked.
//!
//! Progress does NOT come from this module. The review server writes
//! `~/.spinzero/mcp-runs/<review_id>/status.json` and `mcpstatus.rs` watches it. What
//! the agent prints is kept as a sign of life, and is never parsed.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};

use crate::mcpstatus::RunStatus;

/// Progress from a running agent review, streamed to the frontend as `agent-event`.
///
/// Serialised with the box transparent, so the frontend sees `{kind: "status",
/// status: {...}}` and not a level of nesting that exists for a Rust reason.
#[derive(Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AgentEvent {
    Started { agent: String },
    /// One line of the agent's output, already trimmed. Never a finding, and never
    /// parsed — it exists so a stalled run looks different from a quiet one.
    Progress { line: String },
    /// The review server's own account of the run. This is the progress bar. Boxed
    /// because it is far larger than the other variants, and every one of them would
    /// otherwise be sized for it — these are emitted a few times a second.
    Status { status: Box<RunStatus> },
    /// The agent's process ended. The findings, if any, are in the review inbox.
    Finished { seconds: u64 },
    Failed { detail: String },
}

pub fn emit(app: &AppHandle, ev: AgentEvent) {
    let _ = app.emit("agent-event", ev);
}

/// How an agent wants its prompt.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum PromptVia {
    /// Substituted into `{prompt}` in the argument list.
    #[default]
    Arg,
    /// Written to the child's standard input, and `{prompt}` arguments are dropped.
    Stdin,
}

/// How to start one agent.
///
/// `{prompt}` and `{project_dir}` are the only placeholders, and the code does the
/// quoting — every argument is passed as one argument, so a path with a space cannot
/// become two. `mcpConfig.ts` holds the tests for that exact failure on the config
/// block; this is the same rule on the command line.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct AgentProfile {
    pub id: String,
    pub label: String,
    /// The executable. Empty means "whatever `id` names on PATH" is NOT assumed — an
    /// empty binary is a profile that cannot run, and is refused at start.
    pub bin: String,
    #[serde(default)]
    pub prompt_via: PromptVia,
    #[serde(default)]
    pub args: Vec<String>,
    /// Has SpinZero run this profile end to end? A profile we have not is offered,
    /// because the alternative is a user who cannot use their own agent at all — but
    /// the screen says so, and does not present a guess as a fact.
    #[serde(default)]
    pub verified: bool,
}

impl AgentProfile {
    fn new(id: &str, label: &str, bin: &str, args: &[&str], verified: bool) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            bin: bin.into(),
            prompt_via: PromptVia::Arg,
            args: args.iter().map(|a| (*a).to_string()).collect(),
            verified,
        }
    }
}

/// The profiles SpinZero ships.
///
/// Claude Code is the only one marked verified, because it is the only one this
/// machine could run end to end. The rest are starting points: the picker fills the
/// fields in and the user edits them, which is a better answer than leaving somebody
/// with Codex to work out the flags from nothing. See `docs/bom-review-flow.md` section C, "The status file".
pub fn builtin_profiles() -> Vec<AgentProfile> {
    vec![
        AgentProfile::new("claude-code", "Claude Code", "claude", &["-p", "{prompt}"], true),
        AgentProfile::new("codex-cli", "Codex CLI", "codex", &["exec", "{prompt}"], false),
        AgentProfile::new("gemini-cli", "Gemini CLI", "gemini", &["-p", "{prompt}"], false),
        AgentProfile::new("cursor-cli", "Cursor CLI", "cursor-agent", &["-p", "{prompt}"], false),
        AgentProfile::new("custom", "Something else", "", &["{prompt}"], false),
    ]
}

/// What SpinZero already knows and the review server is about to ask for.
///
/// Both answers are here because the server stops for both, and an agent with no user
/// cannot obtain either. Passing them is what turns a stall into a run.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct ReviewBrief {
    /// The end application profile id, or empty when the user has not stated one.
    #[serde(default)]
    pub profile: String,
    /// One line per confirmed field: the review's field name and the BOM column that
    /// feeds it. An empty column means "this BOM has no such column".
    #[serde(default)]
    pub mapping: Vec<MappedColumn>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct MappedColumn {
    pub field: String,
    pub column: String,
}

/// What we ask the agent to do.
///
/// Short on purpose. The server's own instructions and the `next` line on every tool
/// result carry the workflow, and repeating it here would give the model two sources
/// of truth about a process only one of them can see. This says the three things the
/// server cannot know: which board, what the answers to its preflight are, and that
/// there is nobody to ask.
fn prompt(project_dir: &Path, brief: &ReviewBrief) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "Run a SpinZero BOM review of {}.\n\n\
         NOBODY IS IN THIS CONVERSATION. You are being run by the SpinZero app, and there is no \
         user to answer a question or approve anything. Do not ask; act on what is below.\n\n\
         Call spinzero_start_review with that path and non_interactive: true. It will stop and \
         ask for the end application and the column mapping. You already have both answers:\n\n",
        project_dir.display()
    ));
    if brief.profile.trim().is_empty() {
        out.push_str(
            "* End application: the user has not stated one. Call spinzero_confirm_setup with NO \
             profile. The review then runs under the strictest rules and says nobody stated one. \
             Do not guess it from the board.\n",
        );
    } else {
        out.push_str(&format!(
            "* End application: {}. Pass it as the `profile`.\n",
            brief.profile.trim()
        ));
    }
    if brief.mapping.is_empty() {
        out.push_str("* Column mapping: the user approved the mapping the server resolves on its own. Pass no mapping_overrides.\n");
    } else {
        out.push_str(
            "* Column mapping: the user has already checked and corrected it. Pass these as \
             `mapping_overrides`, exactly as written, and change nothing else:\n",
        );
        for m in &brief.mapping {
            out.push_str(&format!("    {} = {}\n", m.field, quoted(&m.column)));
        }
    }
    out.push_str(
        "\nThen follow the `next` field on every result until the review is finished. Run each \
         step in a fresh sub-agent, as the server's instructions tell you to. Account for every \
         part in every batch, including the ones with nothing wrong. Do not claim to have read a \
         datasheet the server did not obtain.\n\n\
         When it is done, report where the findings landed and repeat the coverage numbers \
         verbatim, including anything the review could not check.\n",
    );
    out
}

/// An empty column is an instruction, not a blank, so it must be visible as one.
fn quoted(column: &str) -> String {
    if column.trim().is_empty() {
        "\"\" (this BOM has no such column — suppress the server's guess)".to_string()
    } else {
        format!("\"{column}\"")
    }
}

/// Fill the placeholders. Every argument stays one argument.
fn build_args(profile: &AgentProfile, project_dir: &Path, prompt: &str) -> Vec<String> {
    let dir = project_dir.to_string_lossy().to_string();
    profile
        .args
        .iter()
        .filter(|a| !(profile.prompt_via == PromptVia::Stdin && a.contains("{prompt}")))
        .map(|a| a.replace("{prompt}", prompt).replace("{project_dir}", &dir))
        .collect()
}

/// A running agent review. One at a time per app: two agents reviewing the same board
/// would race on the drop-box and file two sets of comments for one board.
#[derive(Default)]
pub struct AgentRun {
    running: Arc<AtomicBool>,
}

impl AgentRun {
    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    /// Spawn the agent and stream its output. Returns as soon as the process is up;
    /// everything after that arrives as `agent-event`.
    pub fn start(
        &self,
        app: AppHandle,
        project_dir: PathBuf,
        profile: AgentProfile,
        brief: ReviewBrief,
    ) -> Result<(), String> {
        let bin = profile.bin.trim().to_string();
        if bin.is_empty() {
            return Err("this agent has no command to run: fill in its program in the review setup.".into());
        }
        if self.running.swap(true, Ordering::SeqCst) {
            return Err("a review is already running through your agent.".into());
        }
        let running = self.running.clone();
        let text = prompt(&project_dir, &brief);
        let args = build_args(&profile, &project_dir, &text);
        let label = profile.label.clone();

        std::thread::spawn(move || {
            let started = std::time::Instant::now();
            emit(&app, AgentEvent::Started { agent: label.clone() });

            let spawned = Command::new(&bin)
                .args(&args)
                .current_dir(&project_dir)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .stdin(if profile.prompt_via == PromptVia::Stdin {
                    Stdio::piped()
                } else {
                    Stdio::null()
                })
                .spawn();

            let mut child = match spawned {
                Ok(c) => c,
                Err(e) => {
                    log::warn!("agent review could not start {bin}: {e}");
                    emit(
                        &app,
                        AgentEvent::Failed {
                            detail: format!(
                                "could not start {bin}: {e}. Check the program name in the review setup, \
                                 and that it is installed and on PATH."
                            ),
                        },
                    );
                    running.store(false, Ordering::SeqCst);
                    return;
                }
            };

            if profile.prompt_via == PromptVia::Stdin {
                if let Some(mut stdin) = child.stdin.take() {
                    let _ = stdin.write_all(text.as_bytes());
                    // Dropped here, which closes the pipe: an agent reading its prompt
                    // from stdin waits for end-of-file, so a pipe left open is a run
                    // that never starts.
                }
            }

            if let Some(out) = child.stdout.take() {
                for line in BufReader::new(out).lines().map_while(Result::ok) {
                    let line = line.trim().to_string();
                    if !line.is_empty() {
                        emit(&app, AgentEvent::Progress { line });
                    }
                }
            }

            // stderr is the agent's own diagnostics. Kept for the log and for the
            // failure message: it is where a misconfiguration explains itself, and it
            // is not something to put in front of an engineer mid-review.
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
                                "{label} exited without finishing ({s}). {}",
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

#[cfg(test)]
mod tests {
    use super::*;

    fn brief() -> ReviewBrief {
        ReviewBrief {
            profile: "automotive-safety".into(),
            mapping: vec![
                MappedColumn { field: "mpn".into(), column: "Mfr Part #".into() },
                MappedColumn { field: "alt_mpn".into(), column: String::new() },
            ],
        }
    }

    #[test]
    fn the_prompt_carries_both_preflight_answers_and_says_nobody_is_there() {
        let p = prompt(Path::new("C:/boards/MC-02"), &brief());
        assert!(p.contains("C:/boards/MC-02"));
        assert!(p.contains("non_interactive: true"));
        assert!(p.contains("NOBODY IS IN THIS CONVERSATION"));
        assert!(p.contains("automotive-safety"));
        assert!(p.contains("mpn = \"Mfr Part #\""));
        // A suppression reads as one, rather than as a blank somebody might drop.
        assert!(p.contains("alt_mpn = \"\" (this BOM has no such column"));
    }

    #[test]
    fn an_unstated_application_is_stated_as_unstated() {
        let p = prompt(Path::new("/b"), &ReviewBrief::default());
        assert!(p.contains("has not stated one"));
        assert!(!p.contains("Pass it as the `profile`"));
    }

    #[test]
    fn a_prompt_with_spaces_stays_one_argument() {
        let profile = AgentProfile::new("x", "X", "x", &["-p", "{prompt}", "--dir", "{project_dir}"], false);
        let args = build_args(&profile, Path::new("C:/Program Files/b"), "run a review of b");
        assert_eq!(args.len(), 4);
        assert_eq!(args[1], "run a review of b");
        assert_eq!(args[3], "C:/Program Files/b");
    }

    #[test]
    fn a_stdin_agent_gets_no_prompt_argument() {
        let mut profile = AgentProfile::new("x", "X", "x", &["exec", "{prompt}"], false);
        profile.prompt_via = PromptVia::Stdin;
        assert_eq!(build_args(&profile, Path::new("/b"), "hello"), vec!["exec".to_string()]);
    }

    #[test]
    fn the_shipped_profiles_all_name_a_program_except_the_custom_one() {
        for p in builtin_profiles() {
            assert!(!p.label.is_empty());
            assert_eq!(p.bin.is_empty(), p.id == "custom");
        }
        // The first is what a fresh install runs with, so it must be one we have run.
        assert!(builtin_profiles()[0].verified);
    }
}
