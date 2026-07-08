; SpinZero NSIS customizations.
;
; Tauri !include-s this file near the TOP of its generated installer.nsi — before the
; MUI pages are inserted — so a !define here is in scope when MUI_PAGE_WELCOME reads it.
; (MUI_PAGE_WELCOME only sets its own default with !ifndef, so an earlier define wins.)
; It is wired via tauri.conf.json -> bundle.windows.nsis.installerHooks.
;
; Replace the stock MUI welcome text. The default — "It is recommended that you close
; all other applications before starting Setup. This will make it possible to update
; relevant system files without having to reboot your computer." — is per-machine,
; system-file boilerplate that is irrelevant, and needlessly alarming, for SpinZero's
; per-user, no-admin install (installMode: currentUser).
!define MUI_WELCOMEPAGE_TEXT "Setup will install SpinZero on your computer.$\r$\n$\r$\nSpinZero installs just for you and needs no administrator rights.$\r$\n$\r$\n$_CLICK"
