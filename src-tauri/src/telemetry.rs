//! Anonymized telemetry via Sentry — ONE maintained SDK, no second vendor
//! (chosen over a hand-rolled uploader 2026-06-22; over Aptabase/PostHog
//! 2026-07-02). The signals, and where each comes from:
//!
//! - **Active users / installs**: release health sessions (`auto_session_tracking`)
//!   keyed on the anonymous per-install `user.id`; plus a one-shot `install`
//!   event on first run and an `update` event when the version changes.
//!   (Download counts are on the GitHub Releases API — no code needed.)
//! - **Crashes**: Sentry's panic integration + [`record_fatal`].
//! - **Errors in the logs**: [`wrap_logger`] bridges the `log` facade —
//!   `error!` → event, `warn!` → crash-context breadcrumb. `info!`/`debug!`
//!   stay in the local log file ONLY — they are never sent, so a line like
//!   `opened project '<name>'` can't leak off the machine.
//! - **Feature usage**: [`bump`] increments *lifetime* per-install counters
//!   (projects opened, review comments/sessions, crunches, searches) persisted
//!   in `telemetry.json`; [`on_exit`] ships the current totals as ONE event
//!   tagged `event_type=usage`, so usage is filterable apart from crash/error
//!   Issues even though Sentry stores every captured message as an Issue.
//!
//! The DSN bakes in at compile time from `.env` via `npm run tauri:release`
//! (see [`DSN_BAKED`]); without it every signal stays on the machine.
//!
//! # Privacy contract (the load-bearing part)
//!
//! - **Dev builds send nothing.** A debug build (`npm run tauri dev`) resolves no
//!   DSN at all, so the client is disabled — our own development crashes, errors
//!   and usage never reach the collector. Set `PCBREVIEW_SENTRY_DEV=1` to opt a
//!   dev run back in when testing telemetry itself.
//! - **DSN-gated.** With no DSN set (`PCBREVIEW_SENTRY_DSN`, the default) the
//!   Sentry client is disabled and nothing is transmitted — the capability is
//!   inert until an operator deliberately points it at a collector.
//! - **No PII.** `send_default_pii = false` (so Sentry never attaches the IP or
//!   OS user), and [`before_send`] additionally strips the hostname
//!   (`server_name`) and scrubs free text. The ONLY identifier attached is a
//!   random per-install UUID used as an anonymous `user.id`, so release-health
//!   can count installs without identifying anyone.
//! - **No design data.** Every value we attach is a typed tag / enum / duration
//!   sourced from code literals (`design_tool`, `trigger`, `ms`, …). The few
//!   free-text fields that could carry data — crash messages — pass through
//!   [`scrub`], which replaces anything path-shaped with `<path>`.
//!
//! # Reliability contract
//!
//! Consent is a runtime kill switch ([`set_enabled`]): when off, every record_*
//! entry point returns early and the Sentry client is unbound from the current
//! hub. Calls are cheap and never block the operation they observe.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use sentry::protocol::{Breadcrumb, Event, Level, User};
use sentry::{ClientInitGuard, ClientOptions};
use serde::{Deserialize, Serialize};

use crate::util::LockExt;

/// Env var that supplies the Sentry DSN. Checked at RUNTIME first (dev
/// override), then at COMPILE time via `option_env!` — `npm run tauri:release`
/// loads `.env` through dotenv-cli, so setting `PCBREVIEW_SENTRY_DSN` there
/// bakes the collector into the shipped binary. Neither set → the client is
/// disabled and nothing is ever sent.
const DSN_ENV: &str = "PCBREVIEW_SENTRY_DSN";
const DSN_BAKED: Option<&str> = option_env!("PCBREVIEW_SENTRY_DSN");

/// Escape hatch to exercise the telemetry path from a dev build (`tauri dev`),
/// which otherwise never resolves a DSN — see [`is_dev`].
const DEV_OPT_IN_ENV: &str = "PCBREVIEW_SENTRY_DEV";

/// True in a development build. `npm run tauri dev` compiles the app in debug,
/// releases in release, so `debug_assertions` is the dev/prod line. Dev runs
/// produce crashes, errors and usage counts that are ours, not a user's, so
/// they must never reach the collector and pollute release health — hence
/// [`init`] resolves NO DSN here (the client stays disabled and nothing is
/// transmitted), unless [`DEV_OPT_IN_ENV`] is set to deliberately test it.
fn is_dev() -> bool {
    if !cfg!(debug_assertions) {
        return false;
    }
    !matches!(
        std::env::var(DEV_OPT_IN_ENV).as_deref(),
        Ok("1") | Ok("true")
    )
}

/// Prefix marking an `error!` whose free text must NEVER reach Sentry — extractor
/// stderr and extractor error strings routinely embed design / sheet / net /
/// component / file names, and the token [`scrub`] can only catch path-*shaped*
/// tokens (separator-free names sail through). Records with this prefix are written
/// to the local log file but NOT captured as Sentry events. The failing stage is
/// still reported to Sentry as a normalised tag via the crunch transaction
/// ([`CrunchSpan::finish`]), so we lose no actionable signal. See [`wrap_logger`].
pub const LOCAL_ONLY: &str = "[local]";

static TELE: OnceLock<Telemetry> = OnceLock::new();

struct Telemetry {
    /// Random, anonymous per-install id (Sentry `user.id`). No PII.
    install_id: String,
    /// User consent. Gates every capture and toggles the live client.
    enabled: AtomicBool,
    /// Whether a DSN was configured (shown in Diagnostics).
    dsn_configured: bool,
    /// Where consent + install id persist.
    config_path: PathBuf,
    /// The initialized client, kept so consent can re-bind it after a disable.
    client: Option<Arc<sentry::Client>>,
    /// Lifetime, per-install usage counters (metric → running total), seeded from
    /// `telemetry.json` at init, incremented by [`bump`], and shipped as ONE
    /// summary event by [`on_exit`] (which also re-persists them). Cumulative, so
    /// a single event per install carries the whole history — no per-action
    /// pings, nothing that could carry a project/net/file name.
    counters: Mutex<BTreeMap<String, u64>>,
    /// Bumps since the counters were last flushed to disk. [`bump`] persists every
    /// [`SAVE_EVERY`] increments so a crash / force-kill can't lose the whole
    /// session's usage — exactly the crash-correlated usage worth keeping.
    bumps_since_save: std::sync::atomic::AtomicU64,
}

/// Flush the lifetime usage counters to disk after this many [`bump`]s (in addition
/// to the guaranteed flush in [`on_exit`]). Small enough that a crash loses little,
/// large enough that a burst of searches isn't one file write each.
const SAVE_EVERY: u64 = 5;

/// Persisted across sessions: the anonymous install id, the consent flag, and
/// the last app version seen (so an update can be detected on next launch).
#[derive(Serialize, Deserialize, Default)]
struct Persist {
    install_id: String,
    enabled: Option<bool>,
    last_version: Option<String>,
    /// Lifetime usage totals (metric → count). Cumulative across every session.
    #[serde(default)]
    counters: BTreeMap<String, u64>,
}

/// Telemetry consent state surfaced to the Privacy dialog (via the
/// `get_telemetry_info` command). Anonymized — no design data.
#[derive(Serialize, Clone)]
pub struct TelemetryInfo {
    pub install_id: String,
    pub enabled: bool,
    pub dsn_configured: bool,
}

// ----------------------------------------------------------------- init

/// Initialise Sentry and return its guard. MUST be called before the Tauri
/// builder and the returned guard kept alive for the whole run (it flushes
/// pending events on drop). Resolves its config via `dirs` (no `AppHandle`), so
/// it can run that early. Idempotent: a second call returns a disabled guard.
pub fn init() -> ClientInitGuard {
    let config_path = config_path();
    let mut persist = load(&config_path);
    let first_run = persist.install_id.is_empty();
    if first_run {
        persist.install_id = uuid::Uuid::new_v4().to_string();
    }
    let enabled = persist.enabled.unwrap_or(true);
    // Version the previous launch wrote — differs after an update installs.
    let prev_version = persist.last_version.clone();
    // Lifetime usage totals carried forward from prior sessions.
    let counters = std::mem::take(&mut persist.counters);
    // Make sure the file exists with a stable id even on first run (and stamp
    // the current version so the next launch can detect an update).
    save(&config_path, &persist.install_id, enabled, &counters);

    let dev = is_dev();
    // In a dev build we resolve no DSN at all, so the client is disabled and every
    // capture below (lifecycle, errors, panics, usage) is a no-op transport-wise.
    let dsn = (!dev)
        .then(|| {
            std::env::var(DSN_ENV)
                .ok()
                .filter(|s| !s.trim().is_empty())
                .or_else(|| DSN_BAKED.map(str::to_string).filter(|s| !s.trim().is_empty()))
        })
        .flatten();
    let dsn_configured = dsn.is_some();

    let guard = sentry::init(ClientOptions {
        dsn: dsn.and_then(|d| d.parse().ok()),
        release: sentry::release_name!(),
        // Performance monitoring (crunch transactions). Cheap for a desktop app.
        traces_sample_rate: 1.0,
        // Never attach IP / OS user.
        send_default_pii: false,
        // Release health = the "usage" signal (active installs / sessions).
        auto_session_tracking: true,
        before_send: Some(Arc::new(|mut event: Event<'static>| {
            // Strip the hostname the contexts integration would attach, then scrub
            // EVERY free-text field — not just `message`. Panic events land the
            // payload in `exception.values[*].value` and the log bridge builds a
            // `logentry`, both of which bypass a message-only scrub.
            scrub_event(&mut event);
            Some(event)
        })),
        // Breadcrumbs are attached raw to crash events and are NOT covered by
        // before_send, so scrub their messages here too — a warn! that mentions
        // a project name / path must not ride along on the next crash.
        before_breadcrumb: Some(Arc::new(|mut bc: Breadcrumb| {
            if let Some(msg) = bc.message.take() {
                bc.message = Some(scrub(&msg));
            }
            Some(bc)
        })),
        ..Default::default()
    });

    let client = sentry::Hub::current().client();

    // Stable anonymous identity so release-health counts installs, not people.
    let id = persist.install_id.clone();
    sentry::configure_scope(|scope| {
        scope.set_user(Some(User {
            id: Some(id),
            ..Default::default()
        }));
    });

    let tele = Telemetry {
        install_id: persist.install_id,
        enabled: AtomicBool::new(enabled),
        dsn_configured,
        config_path,
        client,
        counters: Mutex::new(counters),
        bumps_since_save: std::sync::atomic::AtomicU64::new(0),
    };
    let _ = TELE.set(tele);

    // Honor a persisted opt-out immediately.
    if !enabled {
        sentry::Hub::current().bind_client(None);
    }

    // One-shot lifecycle events (install count / update adoption). Versions are
    // code literals, so nothing here can carry user data.
    if enabled {
        if first_run {
            sentry::with_scope(
                |scope| scope.set_tag("event_type", "lifecycle"),
                || sentry::capture_message("install", Level::Info),
            );
        } else if prev_version.as_deref() != Some(env!("CARGO_PKG_VERSION")) {
            sentry::with_scope(
                |scope| {
                    scope.set_tag("event_type", "lifecycle");
                    scope.set_tag(
                        "from_version",
                        prev_version.as_deref().unwrap_or("unknown"),
                    );
                },
                || sentry::capture_message("update", Level::Info),
            );
        }
    }

    log::info!(
        "telemetry: enabled={enabled} dsn={}",
        if dsn_configured {
            "configured"
        } else if dev {
            "none (dev build — nothing is sent)"
        } else {
            "none"
        }
    );
    guard
}

fn get() -> Option<&'static Telemetry> {
    TELE.get()
}

fn is_enabled() -> bool {
    get().map(|t| t.enabled.load(Ordering::SeqCst)).unwrap_or(false)
}

// ----------------------------------------------------------------- consent

/// Flip telemetry consent (Diagnostics toggle). Persists the choice and binds /
/// unbinds the live Sentry client so the change takes effect immediately.
pub fn set_enabled(enabled: bool) -> bool {
    let Some(t) = get() else { return enabled };
    t.enabled.store(enabled, Ordering::SeqCst);
    if enabled {
        if let Some(c) = &t.client {
            sentry::Hub::current().bind_client(Some(c.clone()));
        }
    } else {
        sentry::Hub::current().bind_client(None);
    }
    save(&t.config_path, &t.install_id, enabled, &t.counters.lock_safe());
    log::info!("telemetry: consent set to {enabled}");
    enabled
}

/// Snapshot for the Diagnostics dialog. None until [`init`] has run.
pub fn info() -> Option<TelemetryInfo> {
    let t = get()?;
    Some(TelemetryInfo {
        install_id: t.install_id.clone(),
        enabled: t.enabled.load(Ordering::SeqCst),
        dsn_configured: t.dsn_configured,
    })
}

// ----------------------------------------------------------------- recording

/// Increment a lifetime usage counter (the metric name is normalised so a caller
/// can never smuggle free text into it). Nothing is transmitted here: the running
/// totals are held in memory, persisted + shipped as ONE summary event by
/// [`on_exit`]. No per-action breadcrumb — usage never rides along on a crash.
pub fn bump(metric: &str) {
    let Some(t) = get() else { return };
    if !t.enabled.load(Ordering::SeqCst) {
        return;
    }
    // Increment, and every SAVE_EVERY bumps snapshot the totals for a flush (done
    // outside the lock so the file write never blocks another bump).
    let snapshot = {
        let mut counters = t.counters.lock_safe();
        *counters.entry(normalize(metric)).or_insert(0) += 1;
        if t.bumps_since_save.fetch_add(1, Ordering::SeqCst) + 1 >= SAVE_EVERY {
            t.bumps_since_save.store(0, Ordering::SeqCst);
            Some(counters.clone())
        } else {
            None
        }
    };
    if let Some(counters) = snapshot {
        save(&t.config_path, &t.install_id, true, &counters);
    }
}

/// Increment a counter ONCE per install, and never again.
///
/// For "how many installs ever did X at all". [`bump`] counts events, which is right
/// for "reviews run" and wrong for "assistants connected": a user who disconnects and
/// reconnects has not installed anything twice, and counting the transition turns an
/// install count into a fidget count. The counter is lifetime and persisted, so its
/// own value is the flag — nothing extra is stored.
pub fn bump_once(metric: &str) {
    let Some(t) = get() else { return };
    if !t.enabled.load(Ordering::SeqCst) {
        return;
    }
    let snapshot = {
        let mut counters = t.counters.lock_safe();
        let key = normalize(metric);
        if counters.get(&key).is_some_and(|n| *n > 0) {
            return;
        }
        counters.insert(key, 1);
        // Persisted immediately rather than on the SAVE_EVERY cadence: this fires once
        // in the life of an install, and a crash before the next flush would lose the
        // one observation AND let it be counted again on the next run.
        t.bumps_since_save.store(0, Ordering::SeqCst);
        counters.clone()
    };
    save(&t.config_path, &t.install_id, true, &snapshot);
}

/// Persist the lifetime usage totals and ship them as ONE event tagged
/// `event_type=usage` (so usage is filterable apart from crash/error Issues),
/// then flush the transport. Called from the Tauri `RunEvent::Exit` handler —
/// the event loop may terminate the process without unwinding, so waiting for
/// the guard's drop-flush is not reliable.
pub fn on_exit() {
    let Some(t) = get() else { return };
    let counters = t.counters.lock_safe().clone();
    if t.enabled.load(Ordering::SeqCst) {
        // Re-persist the accumulated totals so they survive to the next launch.
        save(&t.config_path, &t.install_id, true, &counters);
        if !counters.is_empty() {
            sentry::with_scope(
                |scope| {
                    scope.set_tag("event_type", "usage");
                    for (metric, n) in &counters {
                        scope.set_extra(metric, (*n).into());
                    }
                },
                || sentry::capture_message("usage", Level::Info),
            );
        }
    }
    if let Some(client) = sentry::Hub::current().client() {
        client.flush(Some(std::time::Duration::from_secs(2)));
    }
}

// ----------------------------------------------------------------- log bridge

/// Wrap the real logger (tauri-plugin-log's file + stdout sinks) so records
/// ALSO feed Sentry: `error!` → an event ("errors in the logs"), `warn!` → a
/// crash-context breadcrumb. `info!`/`debug!` are NOT forwarded — they stay in
/// the local log file only, so descriptive lines (e.g. `opened project '<name>'`)
/// never leave the machine. Panics and top-level fatals are demoted to
/// breadcrumbs here because they already produce their own richer Sentry event
/// (the panic integration / [`record_fatal`]) — without that they would be
/// double-reported. Consent still gates everything: with the client unbound,
/// captures and breadcrumbs go nowhere. (Any surviving breadcrumb message is
/// additionally scrubbed by the `before_breadcrumb` hook in [`init`].)
pub fn wrap_logger(dest: Box<dyn log::Log>) -> Box<dyn log::Log> {
    use sentry::integrations::log::{
        breadcrumb_from_record, event_from_record, RecordMapping, SentryLogger,
    };
    let logger = SentryLogger::with_dest(dest).mapper(|record| {
        let msg = record.args().to_string();
        match record.level() {
            log::Level::Error if msg.starts_with("[PANIC]") || msg.starts_with("[FATAL]") => {
                RecordMapping::Breadcrumb(breadcrumb_from_record(record))
            }
            // Untrusted free text (extractor errors, design/net/file names) — keep it in
            // the local log ONLY. Not even a breadcrumb: a breadcrumb would ride along
            // on the next crash and scrub can't sanitize separator-free names.
            log::Level::Error if msg.starts_with(LOCAL_ONLY) => RecordMapping::Ignore,
            log::Level::Error => RecordMapping::Event(event_from_record(record)),
            log::Level::Warn => RecordMapping::Breadcrumb(breadcrumb_from_record(record)),
            _ => RecordMapping::Ignore,
        }
    });
    Box::new(logger)
}

/// A fatal top-level exit. Flushes synchronously — the caller is about to
/// `process::exit`, which would skip the guard's drop-flush.
pub fn record_fatal(message: &str) {
    if !is_enabled() {
        return;
    }
    sentry::capture_message(&format!("[fatal] {}", scrub(message)), Level::Fatal);
    if let Some(client) = sentry::Hub::current().client() {
        client.flush(Some(std::time::Duration::from_secs(2)));
    }
}

/// A live performance span around an extraction (crunch). The returned guard is
/// finished by the caller once the crunch ends — that produces a Sentry
/// transaction carrying the duration + outcome, with no design data attached.
pub struct CrunchSpan(Option<sentry::Transaction>);

/// Begin timing a crunch. No-op (and zero overhead) when telemetry is off.
pub fn start_crunch(tool: &str, trigger: &str) -> CrunchSpan {
    if !is_enabled() {
        return CrunchSpan(None);
    }
    // Every crunch (open / create / manual / watcher) funnels through here, so
    // this is the one chokepoint for the lifetime crunch count.
    bump("crunches");
    let tx = sentry::start_transaction(sentry::TransactionContext::new("crunch", "crunch"));
    tx.set_data("design_tool", normalize(tool).into());
    tx.set_data("trigger", normalize(trigger).into());
    CrunchSpan(Some(tx))
}

impl CrunchSpan {
    /// Finish the crunch span, tagging the outcome (and failing stage, if any).
    pub fn finish(self, ok: bool, stage: Option<&str>) {
        if let Some(tx) = self.0 {
            if let Some(st) = stage {
                tx.set_data("fail_stage", normalize(st).into());
            }
            tx.set_status(if ok {
                sentry::protocol::SpanStatus::Ok
            } else {
                sentry::protocol::SpanStatus::InternalError
            });
            tx.finish();
        }
    }
}

// ----------------------------------------------------------------- scrubbing

/// Normalise a tag/action to a safe `[a-z0-9_.]` token so it can never carry
/// free-form data.
fn normalize(s: &str) -> String {
    let mut out: String = s
        .chars()
        .map(|c| {
            let c = c.to_ascii_lowercase();
            if c.is_ascii_alphanumeric() || c == '_' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect();
    out.truncate(48);
    if out.is_empty() {
        out.push_str("unknown");
    }
    out
}

/// Scrub every free-text field of an outbound event through [`scrub`] and drop the
/// hostname. `before_send` only ever saw `message`, but a panic's payload rides in
/// `exception.values[*].value`, the log bridge builds a `logentry`, and `culprit`
/// can carry a path — all of which would otherwise leave the machine unscrubbed.
fn scrub_event(event: &mut Event<'static>) {
    event.server_name = None;
    if let Some(msg) = event.message.take() {
        event.message = Some(scrub(&msg));
    }
    if let Some(entry) = event.logentry.as_mut() {
        entry.message = scrub(&entry.message);
    }
    for exc in event.exception.values.iter_mut() {
        if let Some(v) = exc.value.take() {
            exc.value = Some(scrub(&v));
        }
    }
    if let Some(culprit) = event.culprit.take() {
        event.culprit = Some(scrub(&culprit));
    }
}

/// Replace path-shaped tokens with `<path>` and truncate, so a crash message
/// can't carry a file path / project name off the machine.
fn scrub(msg: &str) -> String {
    let mut out = String::new();
    for tok in msg.split_whitespace() {
        if !out.is_empty() {
            out.push(' ');
        }
        if looks_path_like(tok) {
            out.push_str("<path>");
        } else {
            out.push_str(tok);
        }
        if out.len() >= 280 {
            out.truncate(280);
            out.push('…');
            break;
        }
    }
    out
}

fn looks_path_like(t: &str) -> bool {
    let b = t.as_bytes();
    t.contains('/')
        || t.contains('\\')
        || (b.len() >= 3 && b[1] == b':' && (b[2] == b'\\' || b[2] == b'/'))
}

// ----------------------------------------------------------------- persistence

/// `<os_local_data>/<APP_IDENTIFIER>/telemetry.json` — app-global, so the install id
/// + consent are shared across all projects on this machine.
fn config_path() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join(crate::project::APP_IDENTIFIER)
        .join("telemetry.json")
}

fn load(path: &PathBuf) -> Persist {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save(path: &PathBuf, install_id: &str, enabled: bool, counters: &BTreeMap<String, u64>) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let persist = Persist {
        install_id: install_id.to_string(),
        enabled: Some(enabled),
        // Always the running version: whoever writes the file IS that version.
        last_version: Some(env!("CARGO_PKG_VERSION").to_string()),
        counters: counters.clone(),
    };
    if let Ok(json) = serde_json::to_string_pretty(&persist) {
        let _ = std::fs::write(path, json);
    }
}
