//! The app you open decides which review server Claude Code runs.
//!
//! A review runs through the user's own agent, and the agent finds the review server
//! through its `spinzero` registration (`agent.rs`). On a developer's machine three
//! builds of the app exist side by side, and each one should review with its own
//! server:
//!
//! | This app | `spinzero` runs |
//! |---|---|
//! | A debug build (`tauri dev`) | The server from source, as `~/.spinzero/dev-lane.json` describes it |
//! | A release build that was not installed (`target/release`) | The `spinzero-mcp` beside this app |
//! | An installed build | The `spinzero-mcp` beside this app |
//!
//! So before each review, and once at start-up, `sync` compares the registration with
//! this build's lane and fixes it through `claude mcp`, the same way the Connect screen
//! does. We still never write `~/.claude.json` ourselves: we read it, and the CLI edits
//! it (see `assistant.rs`).
//!
//! Three rules keep this from surprising anyone:
//!
//! * **A pin wins.** If `~/.spinzero/lane-pin` exists, a developer chose a lane by hand,
//!   and nothing here touches the registration.
//! * **An installed build only corrects, it never adds.** A customer who has not
//!   connected Claude Code has not asked us to, so with no `spinzero` entry we do
//!   nothing and the Connect screen stays the way in.
//! * **The dev lane is described, not known.** This repo must not name the private
//!   one, so a debug build reads the source server's command from `dev-lane.json`,
//!   which the private repo's `use-lane.mjs auto` writes.
//!
//! A dev build also starts the dev service its server talks to, when it is not running
//! (`ensure_dev_service`), from the same file.
//!
//! The service address is not a lane. A machine that reaches the box through a tunnel
//! says so in `~/.spinzero/service-url`, which every server build reads for itself.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde::Deserialize;

const SERVER_NAME: &str = "spinzero";
/// Names older developer setups used. Removed whenever we fix the registration, so only
/// one server is live and the agent cannot pick the wrong one.
const OLD_NAMES: &[&str] = &["spinzero-dev"];

/// Which build of the app this is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lane {
    Dev,
    Candidate,
    Installed,
}

impl Lane {
    pub fn name(self) -> &'static str {
        match self {
            Lane::Dev => "dev",
            Lane::Candidate => "candidate",
            Lane::Installed => "installed",
        }
    }
}

/// This build's lane. A release build counts as installed when the installer's
/// `uninstall.exe` sits beside it, which a `target/release` folder never has.
pub fn current() -> Lane {
    let dir = exe_dir();
    lane_for(cfg!(debug_assertions), dir.as_deref())
}

fn lane_for(debug: bool, exe_dir: Option<&Path>) -> Lane {
    if debug {
        return Lane::Dev;
    }
    match exe_dir {
        Some(d) if d.join("uninstall.exe").is_file() => Lane::Installed,
        // A macOS or Linux install has no uninstaller beside it. A `target` folder
        // above the exe is the one sure sign of a local build there.
        Some(d) if !cfg!(windows) && !d.ancestors().any(|a| a.file_name().is_some_and(|n| n == "target")) => {
            Lane::Installed
        }
        _ => Lane::Candidate,
    }
}

fn exe_dir() -> Option<PathBuf> {
    std::env::current_exe().ok()?.parent().map(PathBuf::from)
}

fn exe_name(stem: &str) -> String {
    if cfg!(windows) {
        format!("{stem}.exe")
    } else {
        stem.to_string()
    }
}

/// `~/.spinzero`, or `SPINZERO_MCP_HOME`: the same root the server uses.
fn state_root() -> PathBuf {
    if let Some(explicit) = std::env::var("SPINZERO_MCP_HOME").ok().filter(|s| !s.trim().is_empty()) {
        return PathBuf::from(explicit);
    }
    home().join(".spinzero")
}

fn home() -> PathBuf {
    std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir())
}

/// How to start a review server: what `claude mcp add` is given.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub struct ServerSpec {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

impl ServerSpec {
    /// Same server? Paths compare with either slash, and without case on Windows,
    /// because the CLI stores what it was given and we may have given it either form.
    fn same_as(&self, other: &ServerSpec) -> bool {
        let norm = |s: &str| {
            let s = s.replace('\\', "/");
            if cfg!(windows) {
                s.to_lowercase()
            } else {
                s
            }
        };
        norm(&self.command) == norm(&other.command)
            && self.args.len() == other.args.len()
            && self.args.iter().zip(&other.args).all(|(a, b)| norm(a) == norm(b))
            && self.env.len() == other.env.len()
            && self.env.iter().all(|(k, v)| other.env.get(k).is_some_and(|w| norm(v) == norm(w)))
    }
}

/// The dev lane's server, from `dev-lane.json`.
fn dev_spec(root: &Path) -> Result<ServerSpec, String> {
    let path = root.join("dev-lane.json");
    let text = std::fs::read_to_string(&path).map_err(|_| {
        format!(
            "this is a development build, and {} is missing, so it cannot tell Claude Code where the \
             review server's source is. Run `node scripts/dev/use-lane.mjs auto` in the private checkout once.",
            path.display()
        )
    })?;
    let spec: ServerSpec =
        serde_json::from_str(&text).map_err(|e| format!("{} is not valid: {e}", path.display()))?;
    if spec.command.trim().is_empty() {
        return Err(format!("{} names no command", path.display()));
    }
    Ok(spec)
}

/// The server this build should review with.
fn desired(lane: Lane, root: &Path, exe_dir: Option<&Path>) -> Result<ServerSpec, String> {
    if lane == Lane::Dev {
        return dev_spec(root);
    }
    let server = exe_dir
        .map(|d| d.join(exe_name("spinzero-mcp")))
        .filter(|p| p.is_file())
        .ok_or_else(|| "the review server (spinzero-mcp) is not beside this app".to_string())?;
    Ok(ServerSpec {
        command: server.display().to_string(),
        ..Default::default()
    })
}

/// One registration of ours found in `~/.claude.json`.
#[derive(Debug)]
struct Found {
    name: String,
    /// `None` for the user scope, the project folder for a local-scope entry.
    dir: Option<String>,
    spec: ServerSpec,
}

/// Every `spinzero` and old-named entry, in every scope. Read only; any read or parse
/// problem is "none found", and the CLI then reports its own trouble when we call it.
fn registrations(config: &Path) -> Vec<Found> {
    let Ok(text) = std::fs::read_to_string(config) else {
        return Vec::new();
    };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    let mut collect = |servers: Option<&serde_json::Value>, dir: Option<String>| {
        let Some(map) = servers.and_then(|s| s.as_object()) else {
            return;
        };
        for (name, value) in map {
            if name == SERVER_NAME || OLD_NAMES.contains(&name.as_str()) {
                let spec = serde_json::from_value(value.clone()).unwrap_or_default();
                found.push(Found { name: name.clone(), dir: dir.clone(), spec });
            }
        }
    };
    collect(json.get("mcpServers"), None);
    if let Some(projects) = json.get("projects").and_then(|p| p.as_object()) {
        for (dir, project) in projects {
            collect(project.get("mcpServers"), Some(dir.clone()));
        }
    }
    found
}

/// What `sync` did, for the log.
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    Pinned,
    AlreadyRight,
    NotConnected,
    Fixed,
}

/// Point `spinzero` at this build's own server, if it points anywhere else.
///
/// Err means this build could not set its own lane. A dev or candidate build treats
/// that as a reason not to start a review, because the review would run on some other
/// build's server and look like it ran on this one.
pub fn sync() -> Result<Outcome, String> {
    let lane = current();
    let root = state_root();
    if root.join("lane-pin").is_file() {
        log::info!("review lane is pinned by hand; leaving the spinzero registration alone");
        return Ok(Outcome::Pinned);
    }
    let dir = exe_dir();
    let want = desired(lane, &root, dir.as_deref())?;
    let found = registrations(&home().join(".claude.json"));
    let plan = plan(lane, &want, &found);
    match plan {
        Plan::Nothing(outcome) => Ok(outcome),
        Plan::Replace { remove } => {
            log::info!("pointing the spinzero registration at the {} lane", lane.name());
            for (name, scope, cwd) in remove {
                // A local-scope entry is removed from its own folder, the only place the
                // CLI can see it. A folder that no longer exists cannot shadow anything.
                if cwd.as_deref().is_some_and(|d| !Path::new(d).is_dir()) {
                    continue;
                }
                let _ = run_claude(&["mcp", "remove", &name, "-s", scope], cwd.as_deref());
            }
            let mut args: Vec<String> = vec!["mcp".into(), "add".into(), SERVER_NAME.into(), "-s".into(), "user".into()];
            for (k, v) in &want.env {
                args.push("-e".into());
                args.push(format!("{k}={v}"));
            }
            args.push("--".into());
            args.push(want.command.clone());
            args.extend(want.args.iter().cloned());
            let refs: Vec<&str> = args.iter().map(String::as_str).collect();
            run_claude(&refs, None)?;
            Ok(Outcome::Fixed)
        }
    }
}

enum Plan {
    Nothing(Outcome),
    /// Remove these (name, scope, folder), then add ours at user scope.
    Replace { remove: Vec<(String, &'static str, Option<String>)> },
}

/// Decide what `sync` must do. Pure, so the rules have tests.
fn plan(lane: Lane, want: &ServerSpec, found: &[Found]) -> Plan {
    let user = found.iter().find(|f| f.dir.is_none() && f.name == SERVER_NAME);
    let strays: Vec<&Found> = found
        .iter()
        .filter(|f| !(f.dir.is_none() && f.name == SERVER_NAME))
        .collect();
    if lane == Lane::Installed && user.is_none() {
        return Plan::Nothing(Outcome::NotConnected);
    }
    if user.is_some_and(|u| u.spec.same_as(want)) && strays.is_empty() {
        return Plan::Nothing(Outcome::AlreadyRight);
    }
    let mut remove: Vec<(String, &'static str, Option<String>)> = strays
        .iter()
        .map(|f| (f.name.clone(), if f.dir.is_some() { "local" } else { "user" }, f.dir.clone()))
        .collect();
    if user.is_some() {
        remove.push((SERVER_NAME.into(), "user", None));
    }
    Plan::Replace { remove }
}

fn run_claude(args: &[&str], cwd: Option<&str>) -> Result<(), String> {
    let mut cmd = Command::new("claude");
    cmd.args(args).stdin(Stdio::null());
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let out = cmd.output().map_err(|e| format!("could not run claude: {e}"))?;
    if out.status.success() {
        return Ok(());
    }
    let said = String::from_utf8_lossy(&out.stderr).trim().to_string();
    Err(format!(
        "claude {} failed: {}",
        args.first().copied().unwrap_or(""),
        if said.is_empty() { out.status.to_string() } else { said }
    ))
}

/// For a dev build: take `SPINZERO_BOM_RULES_BIN` from `dev-lane.json`, so the app's own
/// rule checks run the fresh `bom-rules` build, the same one the dev server runs.
/// A variable already set wins. Call before any thread starts.
pub fn adopt_dev_sidecars() {
    if current() != Lane::Dev || std::env::var_os("SPINZERO_BOM_RULES_BIN").is_some() {
        return;
    }
    if let Some(bin) = dev_spec(&state_root()).ok().and_then(|s| s.env.get("SPINZERO_BOM_RULES_BIN").cloned()) {
        std::env::set_var("SPINZERO_BOM_RULES_BIN", bin);
    }
}

/// How to start the dev service: `dev-lane.json`'s `service` field.
#[derive(Clone, Debug, Deserialize, PartialEq)]
struct DevService {
    command: String,
    #[serde(default)]
    args: Vec<String>,
    port: u16,
}

#[derive(Deserialize)]
struct DevLaneFile {
    service: Option<DevService>,
}

/// None when the file is missing, unreadable, or older than the `service` field.
fn dev_service(root: &Path) -> Option<DevService> {
    let text = std::fs::read_to_string(root.join("dev-lane.json")).ok()?;
    serde_json::from_str::<DevLaneFile>(&text).ok()?.service
}

fn listening(port: u16) -> bool {
    let addr = std::net::SocketAddr::from((std::net::Ipv4Addr::LOCALHOST, port));
    std::net::TcpStream::connect_timeout(&addr, std::time::Duration::from_millis(300)).is_ok()
}

/// How long a freshly started dev service gets to open its port.
const SERVICE_START_WAIT: std::time::Duration = std::time::Duration::from_secs(15);

/// For a dev build: start the dev service (licence, parts and the candidate pack on
/// port 8790) when nothing listens on its port.
///
/// Detached, so it outlives this app: `tauri dev` restarts the app on every Rust
/// change, and a service that restarted with it would lose its parts cache warm-up
/// each time. Its output goes to `~/.spinzero/dev-service.log`. Anything already on the
/// port counts as the service, so a copy you started by hand is left alone.
///
/// A no-op for every other build, and for a `dev-lane.json` that names no service.
pub fn ensure_dev_service() -> Result<(), String> {
    if current() != Lane::Dev {
        return Ok(());
    }
    let root = state_root();
    let Some(service) = dev_service(&root) else {
        return Ok(());
    };
    if listening(service.port) {
        return Ok(());
    }
    let log_path = root.join("dev-service.log");
    let log = std::fs::File::create(&log_path)
        .map_err(|e| format!("could not write {}: {e}", log_path.display()))?;
    let log_err = log
        .try_clone()
        .map_err(|e| format!("could not write {}: {e}", log_path.display()))?;
    let mut cmd = Command::new(&service.command);
    cmd.args(&service.args)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(log_err));
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        cmd.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("could not start the dev service ({}): {e}", service.command))?;
    log::info!("started the dev service on port {} (pid {})", service.port, child.id());

    let started = std::time::Instant::now();
    while started.elapsed() < SERVICE_START_WAIT {
        if listening(service.port) {
            return Ok(());
        }
        if let Ok(Some(status)) = child.try_wait() {
            return Err(format!(
                "the dev service stopped at start-up ({status}). Its output is in {}",
                log_path.display()
            ));
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
    Err(format!(
        "the dev service did not open port {} within {}s. Its output is in {}",
        service.port,
        SERVICE_START_WAIT.as_secs(),
        log_path.display()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(command: &str) -> ServerSpec {
        ServerSpec { command: command.into(), ..Default::default() }
    }

    fn user(command: &str) -> Found {
        Found { name: SERVER_NAME.into(), dir: None, spec: spec(command) }
    }

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("sz-lane-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_debug_build_is_the_dev_lane() {
        assert_eq!(lane_for(true, None), Lane::Dev);
    }

    #[test]
    fn a_release_build_with_an_uninstaller_beside_it_is_installed() {
        let dir = temp("installed");
        assert_eq!(lane_for(false, Some(&dir.join("target").join("release"))), Lane::Candidate);
        std::fs::write(dir.join("uninstall.exe"), b"").unwrap();
        assert_eq!(lane_for(false, Some(&dir)), Lane::Installed);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn paths_match_with_either_slash() {
        let a = spec(r"C:\Apps\SpinZero\spinzero-mcp.exe");
        let b = spec("C:/Apps/SpinZero/spinzero-mcp.exe");
        assert!(a.same_as(&b));
        assert!(!a.same_as(&spec("C:/Other/spinzero-mcp.exe")));
    }

    #[test]
    fn an_installed_build_never_adds_a_registration() {
        // The customer has not connected Claude Code, so that is theirs to do.
        assert!(matches!(
            plan(Lane::Installed, &spec("/opt/sz/spinzero-mcp"), &[]),
            Plan::Nothing(Outcome::NotConnected)
        ));
    }

    #[test]
    fn an_installed_build_corrects_a_registration_that_points_elsewhere() {
        let found = [user("node /src/local/server.ts")];
        assert!(matches!(plan(Lane::Installed, &spec("/opt/sz/spinzero-mcp"), &found), Plan::Replace { .. }));
    }

    #[test]
    fn a_dev_build_adds_its_server_when_none_is_registered() {
        let Plan::Replace { remove } = plan(Lane::Dev, &spec("node"), &[]) else {
            panic!("should add");
        };
        assert!(remove.is_empty());
    }

    #[test]
    fn a_right_registration_is_left_alone() {
        let found = [user("/opt/sz/spinzero-mcp")];
        assert!(matches!(
            plan(Lane::Candidate, &spec("/opt/sz/spinzero-mcp"), &found),
            Plan::Nothing(Outcome::AlreadyRight)
        ));
    }

    #[test]
    fn a_second_name_is_removed_even_when_spinzero_is_right() {
        // Two names let the agent pick either server.
        let found = [
            user("/opt/sz/spinzero-mcp"),
            Found { name: "spinzero-dev".into(), dir: Some("/boards".into()), spec: spec("node") },
        ];
        let Plan::Replace { remove } = plan(Lane::Candidate, &spec("/opt/sz/spinzero-mcp"), &found) else {
            panic!("should replace");
        };
        assert!(remove.contains(&("spinzero-dev".into(), "local", Some("/boards".into()))));
    }

    #[test]
    fn the_dev_lane_is_read_from_its_file_and_says_how_to_make_it() {
        let root = temp("devlane");
        let err = dev_spec(&root).unwrap_err();
        assert!(err.contains("use-lane.mjs auto"), "{err}");
        std::fs::write(
            root.join("dev-lane.json"),
            r#"{"command":"node","args":["/src/local/server.ts"],"env":{"SPINZERO_LICENCE_URL":"http://127.0.0.1:8790"}}"#,
        )
        .unwrap();
        let spec = dev_spec(&root).unwrap();
        assert_eq!(spec.command, "node");
        assert_eq!(spec.env.get("SPINZERO_LICENCE_URL").map(String::as_str), Some("http://127.0.0.1:8790"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_dev_service_is_read_from_the_dev_lane_file() {
        let root = temp("devservice");
        assert_eq!(dev_service(&root), None);
        // A file from before the `service` field: no service, and no error.
        std::fs::write(root.join("dev-lane.json"), r#"{"command":"node","args":[],"env":{}}"#).unwrap();
        assert_eq!(dev_service(&root), None);
        std::fs::write(
            root.join("dev-lane.json"),
            r#"{"command":"node","service":{"command":"node","args":["/src/scripts/dev/serve.mjs"],"port":8790}}"#,
        )
        .unwrap();
        let service = dev_service(&root).unwrap();
        assert_eq!(service.port, 8790);
        assert_eq!(service.args, ["/src/scripts/dev/serve.mjs"]);
        // And the server spec still reads from the same file, ignoring the extra field.
        assert_eq!(dev_spec(&root).unwrap().command, "node");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn registrations_are_read_from_every_scope() {
        let dir = temp("config");
        let config = dir.join(".claude.json");
        std::fs::write(
            &config,
            r#"{"mcpServers":{"spinzero":{"type":"stdio","command":"node","args":["a"],"env":{}},"other":{"command":"x"}},
               "projects":{"C:/boards":{"mcpServers":{"spinzero-dev":{"command":"node"}}}}}"#,
        )
        .unwrap();
        let found = registrations(&config);
        assert_eq!(found.len(), 2);
        assert!(found.iter().any(|f| f.name == "spinzero" && f.dir.is_none() && f.spec.args == ["a"]));
        assert!(found.iter().any(|f| f.name == "spinzero-dev" && f.dir.as_deref() == Some("C:/boards")));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
