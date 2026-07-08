//! Logging wiring around `tauri-plugin-log`.
//!
//! The official plugin owns the actual sinks — a rotating file in the OS app-log
//! dir, a stdout mirror, and the webview console — and is installed in
//! [`crate::run`]. All code logs through the `log` facade (`log::info!` /
//! `warn!` / `error!` / `debug!`). This module only adds the one thing the
//! plugin does not provide: a panic hook that records a panic on ANY thread
//! (command / watcher / crunch) to the same log.
//!
//! Everything is best-effort: a logging failure must never affect the operation
//! it reports on.

use std::fmt::Arguments;
use std::sync::OnceLock;

use tauri_plugin_log::fern::FormatCallback;
use tauri_plugin_log::TimezoneStrategy;
use time::format_description::FormatItem;
use time::macros::format_description;

/// Fixed log-file stem, so the on-disk path is deterministic: the plugin writes
/// `<app_log_dir>/spinzero.log`.
pub const LOG_FILE_STEM: &str = "spinzero";

/// Log timestamp = the machine's LOCAL wall-clock plus an explicit UTC offset, e.g.
/// `2026-07-07 15:30:00+05:30`. SpinZero runs on desktops all over the world, so a bare
/// UTC time (the plugin's default) forces every reader to convert, and a bare local time
/// is ambiguous once a log leaves the machine that wrote it. Local time carries the offset
/// so it reads naturally for the user who filed it (an IST user sees IST) yet stays
/// unambiguous for whoever debugs it — the offset makes UTC a subtraction away.
const LOG_TS_FORMAT: &[FormatItem<'_>] = format_description!(
    "[year]-[month]-[day] [hour]:[minute]:[second][offset_hour sign:mandatory]:[offset_minute]"
);

/// The plugin's per-line formatter, overriding its default (UTC, no offset). Same shape
/// as the plugin's built-in line — `<ts>[LEVEL][target] message` — with our timestamp.
/// `now_local()` falls back to UTC (offset `+00:00`) when the OS can't resolve the zone,
/// which stays truthful because the printed offset then really is UTC.
pub fn format_line(out: FormatCallback<'_>, message: &Arguments<'_>, record: &log::Record<'_>) {
    let ts = TimezoneStrategy::UseLocal
        .get_now()
        .format(&LOG_TS_FORMAT)
        .unwrap_or_default();
    out.finish(format_args!(
        "{}[{}][{}] {}",
        ts,
        record.level(),
        record.target(),
        message
    ));
}

/// One-time startup wiring, called from the Tauri `setup` hook AFTER the log
/// plugin is active: install the panic hook. Idempotent.
pub fn init() {
    install_panic_hook();
    log::info!("SpinZero {} starting", env!("CARGO_PKG_VERSION"));
}

/// Chain a panic-logging hook onto whatever hook is already installed (Sentry's
/// panic integration, then the default). Ours runs FIRST, so the panic lands in
/// the log file regardless of what the later hooks do, then delegates so Sentry
/// still captures it and dev builds keep their backtrace. Idempotent.
fn install_panic_hook() {
    static INSTALLED: OnceLock<()> = OnceLock::new();
    if INSTALLED.set(()).is_err() {
        return;
    }
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        log::error!("[PANIC] {}", describe_panic(info));
        prev(info);
    }));
}

/// A one-line, log-friendly description of a panic: location + message payload.
fn describe_panic(info: &std::panic::PanicHookInfo<'_>) -> String {
    let payload = info
        .payload()
        .downcast_ref::<&str>()
        .map(|s| (*s).to_string())
        .or_else(|| info.payload().downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "<non-string panic payload>".into());
    match info.location() {
        Some(l) => format!(
            "thread panicked at {}:{}:{}: {payload}",
            l.file(),
            l.line(),
            l.column()
        ),
        None => format!("thread panicked: {payload}"),
    }
}

/// Log a fatal line from the top-level run guard, where the runtime is already
/// tearing down and there is no `AppHandle`. Goes through the `log` facade like
/// everything else.
pub fn fatal(msg: &str) {
    log::error!("[FATAL] {msg}");
}
