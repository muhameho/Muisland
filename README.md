# Muisland

Native Windows floating account switcher for Claude Desktop, written in Rust.
Visual inspiration: https://dribbble.com/shots/26245359-Dynamic-Island-Buy-Crypto-Interaction

## Run

Open `Muisland.exe`, or run `.\build.ps1` to build and launch.
Building requires Rust and Visual Studio C++ Build Tools.
`Muisland.exe --preview` uses clearly labelled sample accounts and quota data; it never contacts Anthropic, closes Claude, saves credentials, or persists changes.

## Accounts and names

1. Sign into your first account in Claude Desktop. Choose Save on its slot in Muisland.
2. If Claude is open, the island shows a warning. Cancel leaves it open. Close & save closes Claude, including background processes, waits for exit, then captures login state. Local tasks may stop and unsaved work may be lost. Enter defaults to Cancel; Escape cancels.
3. Sign into the other account in Claude Desktop and save its slot.
4. With Claude fully closed, click a saved row to switch. Muisland attempts to launch `claude://`.
5. Select the small (...) on an account card to edit its name or refresh usage. Enter saves the name; Escape cancels the edit. Names support Unicode, including Arabic, and are stored locally.

## Real quota usage

The cards show session and weekly percent used, plus a reset countdown and the age of the last reading. Hours indicate time until reset, not guaranteed remaining working hours or a fixed number of messages. Missing measurements stay unavailable; reaching a cached reset time does not fabricate a fresh zero reading.

Live usage starts disabled. Choose Refresh live usage (or menu > Refresh all account usage), read the in-island notice, and enable it if desired. The notice explicitly explains the use of saved sign-in credentials with `api.anthropic.com`.
After enabling, the app refreshes at most once every five minutes automatically. Manual refresh is also available. Turn live usage off in the main menu. Previous cached readings retain their timestamp after failures.

The local reader decrypts Electron v10/AES-GCM token caches with Windows DPAPI. It selects only credentials explicitly bound to that saved account ID with user:profile scope. Fresh Desktop credentials are preferred only when their account ID matches the saved slot. Otherwise it uses that slot's encrypted snapshot. It does not read another browser or Claude Code's credentials, rotate tokens, or modify Claude's login for a usage query.
Read-only HTTPS GET requests use `/api/oauth/usage` on `api.anthropic.com`. Redirects are disabled and Windows validates TLS certificates. No chats, files, telemetry, or credentials are sent to other hosts. This unofficial endpoint and Claude's local storage can change. A 401 requires opening Claude and saving the account again. Missing quota fields and request failures remain visible; the app never substitutes estimates or API billing data.

Research reference (independent implementation, no third-party code incorporated):
https://github.com/steipete/CodexBar/blob/main/docs/claude.md
https://support.claude.com/en/articles/11647753-how-do-usage-and-length-limits-work

## What switching preserves

The switcher changes selected authentication data, not server-side account ownership. Chats and cloud Cowork sessions stay with their original account; switching does not transfer them to the other account. Normal switching does not intentionally remove Cowork folders, IndexedDB, or user project files. It is not an export or a guarantee that a local running task will resume.

Cloud Cowork tasks can continue while the desktop app is closed, but local-file access requires the desktop connection. Older local tasks and in-memory work can be interrupted by closing Claude. See:
https://support.claude.com/en/articles/15520349-use-claude-cowork-on-web-desktop-and-mobile

## Optional local checkpoint

Enable **Back up local data before switch** to checkpoint all ordinary files and directories under the detected Claude app-data folder before replacing authentication data. Claude must already be fully closed. If the checkpoint fails, the switch is cancelled. This setting is off by default.

Checkpoints are Windows DPAPI encrypted in bounded-size chunks, support large files, and live under `%LOCALAPPDATA%\ClaudeIsland\local-backups`. Incomplete checkpoints have a `.partial` suffix and are not selected for recovery. Reparse points, including junctions and symbolic links, are rejected rather than followed. Large Claude data folders can take time and disk space. Old checkpoints are retained until you manage them yourself.

The menu's **Recover latest local backup to folder** decrypts the latest completed checkpoint into a new folder under `%LOCALAPPDATA%\ClaudeIsland\recovered` and opens it. This deliberately leaves Claude's live data untouched. The recovered copy contains sensitive application data, just like Claude's original data folder. Recovery does not automatically import sessions into Claude or transfer them between accounts.

This local checkpoint does not export cloud chats, copy connected project folders outside Claude's app-data folder, or preserve a running process/virtual machine in memory. Use Claude's own chat/Cowork export controls when you need a portable transcript or task export.

## Existing storage and rollback

Existing encrypted account snapshots and the auth-only last-switch backup remain in `%LOCALAPPDATA%\ClaudeIsland` for compatibility. Custom names, quota caches, and preferences are DPAPI-encrypted in `muisland-settings.bin` in the same directory.
The menu's Restore previous session restores the auth-only pre-switch backup while Claude is closed. It is separate from local checkpoint recovery.
DPAPI is tied to the current Windows user; it does not protect data from other software running as that user. Switching is unofficial and filesystem restore is best effort, not transactional.

## Interface

Click the pill or +/- to expand or collapse; drag the left header to move it.
The main (...) menu or right-click offers account details, quota refresh, reduced motion, backups, rollback, and exit. Tab/Enter/Space navigate the custom controls; Escape closes the current view.

## Validation

Windows build: `cargo build --release --offline`; formatting: `cargo fmt --check`.
Nine offline tests cover Unicode names, settings defaults, missing/invalid quota fields, timezone/reset handling, cross-account credential selection, AES-256-GCM against a known vector and tamper rejection, and encrypted chunked local-backup roundtrip/recovery safety.
The expanded UI, Arabic name editing and Enter-to-save, quota cards, and optional local-backup toggle were checked in preview mode. Samples are labelled in that mode.
Live authenticated quota fetching and real account switching/local checkpoints have not been exercised on the user's session. Live requests remain subject to user authorization.
