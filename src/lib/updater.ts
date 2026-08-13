// Auto-update from GitHub Releases (item 6). The endpoint + signing pubkey live in
// tauri.conf.json (plugins.updater); the matching private key is held only by the
// release signer. This module drives the JS side: on launch, check for a newer signed
// build and download it quietly in the background, then surface a persistent notice
// pinned to the bottom of the left rail (UpdateBanner) offering "Relaunch to update".
//
// We never force an install MID-session — the running app keeps working and applies the
// new build on the user's nod. But if the user IGNORES that offer and simply closes the
// app, we honor "install it anyway on next launch" (batch1): the offered version is
// remembered, and the next launch applies it automatically (headless install + relaunch)
// instead of merely re-offering. A failure at any step just falls back to offering again,
// so an update never blocks launch.

import { check, type Update } from "@tauri-apps/plugin-updater";
import { ipc } from "./ipc";
import { useUpdateStore } from "../stores/updateStore";
import { useSettingsStore } from "../stores/settingsStore";

/** Version we downloaded + offered, but the user closed the app without applying. Kept
 *  in ui_settings.json (persists across launches) so the NEXT launch can auto-apply it
 *  rather than prompting again. Cleared once we're on the latest build.
 *  NOTE: a readDeferred() reader belongs here when auto-apply is re-enabled (see the
 *  paused block below) — `useSettingsStore.getState().updateDeferred`. It's omitted
 *  while paused so the build has no dead code. */
const writeDeferred = (v: string): void => {
  // Best-effort, like every settings write: a failure just means we re-offer next
  // launch instead of auto-applying.
  void useSettingsStore.getState().setUpdateDeferred(v);
};
const clearDeferred = (): void => {
  void useSettingsStore.getState().setUpdateDeferred(null);
};

/** Check the release endpoint for a newer signed build and either auto-apply a
 *  previously-ignored update or offer it via the left-rail banner. Best-effort: any
 *  failure — offline, no release published yet, or not running under Tauri (the plain
 *  browser dev server) — is logged to the file (WARN, never telemetry, so a normal
 *  offline launch doesn't spam Sentry) and swallowed, so a check never disrupts launch. */
export async function checkForUpdates(): Promise<void> {
  let update: Update | null = null;
  try {
    update = await check();
  } catch (e) {
    void ipc.logWarn(`update check failed: ${String(e)}`);
    return;
  }
  if (!update) {
    // Already on the latest build — drop any stale deferral so a future, different
    // version is offered fresh (and never auto-applied on its very first sighting).
    clearDeferred();
    return;
  }

  // Fetch the build now, while the app is idle, so applying it is instant. The download
  // does not survive a restart, so we re-fetch on each launch. Skip quietly on failure.
  try {
    await update.download();
  } catch (e) {
    void ipc.logWarn(`update download failed (v${update.version}): ${String(e)}`);
    return;
  }

  // PAUSED: auto-installing a previously-ignored update on next launch is disabled for
  // now. Updates are only ever applied on the user's explicit nod via the banner. The
  // deferred-version bookkeeping is kept below (harmless) so re-enabling is a one-block
  // change — restore the readDeferred()/applyNow() check here when ready.
  //
  // if (readDeferred() === update.version) {   // restore a readDeferred() reader (above) too
  //   const applied = await useUpdateStore.getState().applyNow(update);
  //   if (applied) return; // installing → relaunch (does not actually return on success)
  // }

  // Offer this version via the banner. We still remember it so re-enabling auto-apply
  // later needs no other change; for now it just drives the (paused) bookkeeping.
  writeDeferred(update.version);
  useUpdateStore.getState().setReady(update);
}
