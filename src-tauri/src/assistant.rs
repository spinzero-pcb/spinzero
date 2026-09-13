//! Connecting an AI assistant to the review server, without editing anybody's files.
//!
//! **We never write another program's configuration.** Not `~/.claude.json`, not
//! `~/.cursor/mcp.json`, not VS Code's `mcp.json`. Those files belong to their
//! products, they are hand-edited, several of them tolerate comments and trailing
//! commas that a strict writer would destroy, and a corrupted one is a support
//! incident with no undo. Where a client offers a command, we run THAT command and let
//! it edit its own file. Where it does not, we hand the user a block and the path, and
//! they paste it.
//!
//! **The command carries no secret.** The licence key lives in one file on this
//! machine (`~/.spinzero/licence.key`), which the server reads for itself, so the
//! registration line is safe to print on screen, screenshot, or paste into a ticket —
//! and rotating a key means editing one line rather than every client's config.
//!
//! **Detection is a question about this machine, not about the user.** Is the CLI on
//! PATH? Does the config directory exist? Nothing is read out of those files and
//! nothing about them is reported anywhere.

use std::path::PathBuf;
use std::process::Command;

use serde::Serialize;

/// The MCP server name every client registers. Fixed, because the user types it into a
/// conversation ("run a SpinZero review") and a name that varies per install is a name
/// nobody can be told.
const SERVER_NAME: &str = "spinzero";

/// How a client is told about an MCP server.
#[derive(Serialize, Clone, Copy, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum HowToAdd {
    /// It has a CLI that edits its own config. We can run it.
    Command,
    /// It has a config file the user edits. We show the block and the path.
    ConfigFile,
}

#[derive(Serialize)]
pub struct AssistantClient {
    /// Stable id the frontend keys on, and what `register_assistant` takes back.
    pub id: String,
    pub label: String,
    pub how: HowToAdd,
    /// Did we find it on this machine? A client we cannot see is still listed — a user
    /// who is about to install Cursor should not have to wonder where it went.
    pub installed: bool,
    /// For `Command`: the line we would run, ready to show. For `ConfigFile`: empty.
    pub command: String,
    /// For `ConfigFile`: where its MCP servers live. For `Command`: empty.
    pub config_path: String,
    /// `mcpServers` for almost everyone, `servers` for VS Code. The block the frontend
    /// renders has to use the right one or the client ignores it silently.
    pub config_key: String,
}

#[derive(Serialize)]
pub struct AssistantSetup {
    /// The server executable, resolved rather than typed: it sits beside this app.
    pub server_command: String,
    /// Empty when we found the executable. Otherwise why not, in a sentence.
    pub server_problem: String,
    pub licence_file: String,
    /// Is there a key in that file yet? Never the key itself.
    pub licence_present: bool,
    pub clients: Vec<AssistantClient>,
}

fn exe_name(stem: &str) -> String {
    if cfg!(windows) {
        format!("{stem}.exe")
    } else {
        stem.to_string()
    }
}

/// The MCP server the installer put beside this app.
fn server_command() -> Result<PathBuf, String> {
    let dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(PathBuf::from))
        .ok_or_else(|| "could not work out where SpinZero is installed".to_string())?;
    let candidate = dir.join(exe_name("spinzero-mcp"));
    if candidate.is_file() {
        return Ok(candidate);
    }
    Err(format!(
        "the review server is not beside this app (looked for {}). A development build \
         runs it from the source checkout instead.",
        candidate.display()
    ))
}

/// `~/.spinzero/licence.key` — the same path `licenceKey.ts` computes in the server.
///
/// Duplicated rather than shared because the two programs share no code by design, and
/// a constant that must not drift is cheaper to test in two places than to link.
pub fn licence_file() -> PathBuf {
    if let Some(explicit) = std::env::var("SPINZERO_LICENCE_FILE")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
    {
        return PathBuf::from(explicit);
    }
    let home = std::env::var("SPINZERO_MCP_HOME")
        .ok()
        .map(PathBuf::from)
        .or_else(dirs_home)
        .unwrap_or_else(std::env::temp_dir);
    if std::env::var("SPINZERO_MCP_HOME").is_ok() {
        home.join("licence.key")
    } else {
        home.join(".spinzero").join("licence.key")
    }
}

fn dirs_home() -> Option<PathBuf> {
    std::env::var("USERPROFILE")
        .ok()
        .or_else(|| std::env::var("HOME").ok())
        .map(PathBuf::from)
}

/// The placeholder the customer pastes into. Written once; never rewritten, because
/// after the first paste the file holds something we must not lose.
fn ensure_licence_file() -> PathBuf {
    let path = licence_file();
    if path.exists() {
        return path;
    }
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let template = "# Your SpinZero licence key.\n\
                    #\n\
                    # Paste it on a line of its own, below. Lines starting with # are ignored.\n\
                    #\n\
                    # Nothing else needs the key. Every AI assistant you connect reads it from\n\
                    # this file, so rotating a key means editing this one line.\n\n\n";
    let _ = std::fs::write(&path, template);
    path
}

/// Is there a key in the file? The first line that is neither blank nor a comment.
fn licence_present(path: &PathBuf) -> bool {
    std::fs::read_to_string(path)
        .map(|t| {
            t.lines()
                .map(str::trim)
                .any(|l| !l.is_empty() && !l.starts_with('#'))
        })
        .unwrap_or(false)
}

/// Replace the key in the licence file, keeping whatever commentary is already there.
///
/// The old key is commented out rather than deleted. A customer who pastes the wrong
/// one — a staging key, an expired one — can see what was there before and put it
/// back, and a file that silently ate the previous value is a file people stop
/// trusting with the current one.
pub fn write_licence_key(key: &str) -> Result<String, String> {
    let key = key.trim();
    if key.is_empty() {
        return Err("a licence key cannot be blank".into());
    }
    if key.contains(['\n', '\r']) {
        return Err("a licence key is one line".into());
    }
    let path = ensure_licence_file();
    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    let mut out = String::new();
    let mut replaced = false;
    for line in existing.lines() {
        let trimmed = line.trim();
        if !replaced && !trimmed.is_empty() && !trimmed.starts_with('#') {
            out.push_str(&format!("# replaced: {trimmed}\n{key}\n"));
            replaced = true;
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    if !replaced {
        if !out.ends_with('\n') && !out.is_empty() {
            out.push('\n');
        }
        out.push_str(key);
        out.push('\n');
    }
    std::fs::write(&path, out).map_err(|e| format!("could not write {}: {e}", path.display()))?;
    log::info!("licence key written to the licence file");
    Ok(path.display().to_string())
}

/// Is `program` runnable on this machine?
///
/// `--version` rather than a PATH scan: a CLI can be a shim, a shell function's target,
/// or a Windows `.cmd`, and the only question that matters is whether spawning it
/// works. Output is discarded; only the fact that it started is used.
fn runnable(program: &str) -> bool {
    Command::new(program)
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok()
}

fn config_home() -> Option<PathBuf> {
    if cfg!(windows) {
        std::env::var("APPDATA").ok().map(PathBuf::from)
    } else {
        dirs_home().map(|h| h.join(".config"))
    }
}

/// Every client we know how to tell about SpinZero.
///
/// The list is deliberately longer than "the ones installed here": it is also the
/// answer to "does SpinZero work with X", asked at the moment somebody is deciding
/// whether to install X.
fn clients(server: &str) -> Vec<AssistantClient> {
    let home = dirs_home().unwrap_or_else(std::env::temp_dir);
    let appdata = config_home().unwrap_or_else(|| home.clone());

    // `-s user`, not the default `-s local`. The default registers the server for the
    // one directory the CLI happens to be in, which for an app-launched command is the
    // install directory — so the user would connect SpinZero to a folder with no
    // boards in it and find no tools anywhere they actually work.
    let claude = format!("claude mcp add {SERVER_NAME} -s user -- \"{server}\"");
    let codex = format!("codex mcp add {SERVER_NAME} -- \"{server}\"");
    let gemini = format!("gemini mcp add {SERVER_NAME} \"{server}\"");

    vec![
        AssistantClient {
            id: "claude-code".into(),
            label: "Claude Code".into(),
            how: HowToAdd::Command,
            installed: runnable("claude"),
            command: claude,
            config_path: String::new(),
            config_key: "mcpServers".into(),
        },
        AssistantClient {
            id: "codex".into(),
            label: "Codex CLI".into(),
            how: HowToAdd::Command,
            installed: runnable("codex"),
            command: codex,
            config_path: String::new(),
            config_key: "mcpServers".into(),
        },
        AssistantClient {
            id: "gemini".into(),
            label: "Gemini CLI".into(),
            how: HowToAdd::Command,
            installed: runnable("gemini"),
            command: gemini,
            config_path: String::new(),
            config_key: "mcpServers".into(),
        },
        AssistantClient {
            id: "claude-desktop".into(),
            label: "Claude Desktop".into(),
            how: HowToAdd::ConfigFile,
            installed: claude_desktop_dir(&home, &appdata).is_some(),
            command: String::new(),
            config_path: claude_desktop_dir(&home, &appdata)
                .map(|d| d.join("claude_desktop_config.json").display().to_string())
                .unwrap_or_default(),
            config_key: "mcpServers".into(),
        },
        AssistantClient {
            id: "cursor".into(),
            label: "Cursor".into(),
            how: HowToAdd::ConfigFile,
            installed: home.join(".cursor").is_dir(),
            command: String::new(),
            config_path: home.join(".cursor").join("mcp.json").display().to_string(),
            config_key: "mcpServers".into(),
        },
        AssistantClient {
            id: "vscode".into(),
            label: "VS Code (Copilot)".into(),
            how: HowToAdd::ConfigFile,
            installed: vscode_user_dir(&home, &appdata).is_some(),
            command: String::new(),
            config_path: vscode_user_dir(&home, &appdata)
                .map(|d| d.join("mcp.json").display().to_string())
                .unwrap_or_default(),
            // VS Code is the odd one out and calls the object `servers`. A block using
            // `mcpServers` is ignored without an error, which is the worst kind.
            config_key: "servers".into(),
        },
        AssistantClient {
            id: "windsurf".into(),
            label: "Windsurf".into(),
            how: HowToAdd::ConfigFile,
            installed: home.join(".codeium").join("windsurf").is_dir(),
            command: String::new(),
            config_path: home
                .join(".codeium")
                .join("windsurf")
                .join("mcp_config.json")
                .display()
                .to_string(),
            config_key: "mcpServers".into(),
        },
    ]
}

fn claude_desktop_dir(home: &PathBuf, appdata: &PathBuf) -> Option<PathBuf> {
    let candidates = if cfg!(target_os = "macos") {
        vec![home.join("Library").join("Application Support").join("Claude")]
    } else {
        vec![appdata.join("Claude")]
    };
    candidates.into_iter().find(|p| p.is_dir())
}

fn vscode_user_dir(home: &PathBuf, appdata: &PathBuf) -> Option<PathBuf> {
    let candidates = if cfg!(target_os = "macos") {
        vec![home
            .join("Library")
            .join("Application Support")
            .join("Code")
            .join("User")]
    } else if cfg!(windows) {
        vec![appdata.join("Code").join("User")]
    } else {
        vec![home.join(".config").join("Code").join("User")]
    };
    candidates.into_iter().find(|p| p.is_dir())
}

/// Everything the "Connect your AI assistant" screen needs, in one call.
pub fn setup() -> AssistantSetup {
    let (server_command, server_problem) = match server_command() {
        Ok(p) => (p.display().to_string(), String::new()),
        Err(e) => (String::new(), e),
    };
    let licence = ensure_licence_file();
    AssistantSetup {
        clients: clients(&server_command),
        server_command,
        server_problem,
        licence_present: licence_present(&licence),
        licence_file: licence.display().to_string(),
    }
}

#[derive(Serialize)]
pub struct RegisterOutcome {
    pub ok: bool,
    /// What we ran, so the user can run it themselves if it failed.
    pub command: String,
    /// The client's own words. Its message about its own config file beats anything we
    /// could write about it.
    pub detail: String,
}

/// Run one client's own `mcp add`.
///
/// A failure here is not an error dialog: it is the command, on screen, for the user to
/// run in their own terminal. The likeliest causes — the CLI is not on PATH for the
/// process we spawned, or it wants an interactive login — are both things a user fixes
/// in a terminal and neither is something we can fix for them.
pub fn register(client_id: &str) -> Result<RegisterOutcome, String> {
    let setup = setup();
    if setup.server_command.is_empty() {
        return Err(setup.server_problem);
    }
    let client = setup
        .clients
        .into_iter()
        .find(|c| c.id == client_id)
        .ok_or_else(|| format!("{client_id} is not a client SpinZero can register with"))?;
    if client.how != HowToAdd::Command {
        return Err(format!(
            "{} has no command to run — paste the block into {}",
            client.label, client.config_path
        ));
    }

    let (program, args) = split_command(&client.command)?;
    log::info!("registering SpinZero with {}", client.label);
    let output = Command::new(&program)
        .args(&args)
        .stdin(std::process::Stdio::null())
        .output();
    match output {
        Ok(out) if out.status.success() => Ok(RegisterOutcome {
            ok: true,
            command: client.command,
            detail: String::from_utf8_lossy(&out.stdout).trim().to_string(),
        }),
        Ok(out) => {
            let detail = String::from_utf8_lossy(&out.stderr).trim().to_string();
            log::warn!("{} refused the registration: {detail}", client.label);
            Ok(RegisterOutcome {
                ok: false,
                command: client.command,
                detail: if detail.is_empty() {
                    format!("{} exited with {}", program, out.status)
                } else {
                    detail
                },
            })
        }
        Err(e) => Ok(RegisterOutcome {
            ok: false,
            command: client.command,
            detail: format!("could not run {program}: {e}"),
        }),
    }
}

/// Split our own command line back into a program and arguments.
///
/// Only double quotes, because the only quoted thing in these lines is the server path
/// and we are the ones who put the quotes there. This is not a shell and must not grow
/// into one.
fn split_command(line: &str) -> Result<(String, Vec<String>), String> {
    let mut parts: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    let mut started = false;
    for c in line.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                started = true;
            }
            c if c.is_whitespace() && !quoted => {
                if started || !current.is_empty() {
                    parts.push(std::mem::take(&mut current));
                }
                started = false;
            }
            c => current.push(c),
        }
    }
    if started || !current.is_empty() {
        parts.push(current);
    }
    if parts.is_empty() {
        return Err("empty command".into());
    }
    let program = parts.remove(0);
    Ok((program, parts))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_quoted_path_with_spaces_stays_one_argument() {
        let (program, args) =
            split_command(r#"claude mcp add spinzero -s user -- "C:\Program Files\SpinZero\spinzero-mcp.exe""#)
                .expect("splits");
        assert_eq!(program, "claude");
        assert_eq!(args.last().unwrap(), r"C:\Program Files\SpinZero\spinzero-mcp.exe");
        assert_eq!(args.len(), 6);
    }

    #[test]
    fn the_registration_line_carries_no_licence_key() {
        // The whole point of the licence file: this line is safe to print, screenshot
        // and paste into a support thread.
        for client in clients("/opt/SpinZero/spinzero-mcp") {
            assert!(
                !client.command.contains("LICENCE"),
                "{} would leak the key",
                client.label
            );
        }
    }

    #[test]
    fn claude_code_registers_for_the_user_not_the_install_directory() {
        let claude = clients("/opt/SpinZero/spinzero-mcp")
            .into_iter()
            .find(|c| c.id == "claude-code")
            .expect("claude code is listed");
        assert!(claude.command.contains("-s user"), "{}", claude.command);
    }

    #[test]
    fn vscode_is_told_servers_and_everyone_else_mcp_servers() {
        for client in clients("/opt/SpinZero/spinzero-mcp") {
            let expected = if client.id == "vscode" { "servers" } else { "mcpServers" };
            assert_eq!(client.config_key, expected, "{}", client.label);
        }
    }
}
