# Muisland

A native Windows floating account switcher for Claude Desktop, written in Rust.

## Build and run

Install Rust and Visual Studio Build Tools with the C++ desktop workload, then run:

```powershell
.\build.ps1
```

This builds the app, copies `Muisland.exe` to the project root, and launches it. Build outputs are intentionally excluded from Git. `Cargo.lock` is committed for reproducible dependency versions.

## Account controls

- Sign in to Claude Desktop, then choose **Save** on the intended account slot. If Claude is running, the island asks for **Close & save**. It requests the app's normal Exit/Quit action, waits for Desktop to stop, saves, then reopens the same app. It never force-kills Claude or unrelated Claude Code processes. If Claude stays in the tray, quit it normally and retry.
- Click a saved account to open **Switch account?**, then choose **Confirm switch**. Confirmation is required even if Claude is already closed. If needed, confirming closes Claude first.
- Selecting the account already logged into Claude shows **Already logged in**, without closing or changing Claude.
- **Cancel**, Escape, or collapsing the header abandons a displayed confirmation.
- Each card's **(...)** opens its details, where you can change its name or choose **Remove account**. Removal is confirmed inside the island. It does not log Claude out or delete cloud chats/files; the encrypted sign-in snapshot moves to a local archive. Remaining accounts keep their original storage IDs.
- **+ Add account** adds another slot, up to 64. Arrows navigate two cards per page. Removing the last entry leaves Add account available.

Click anywhere in the open header to collapse the island; the explicit header **(...)** opens Preferences. Drag the left header to move it. All confirmations, errors, and settings stay inside the island, with no Windows message dialogs or native popup menus.

## Appearance

Preferences contains independent **Menu size** (75–160%) and **Floating island size** (60–180%) controls. The floating scale applies to the collapsed pill/circle and its hover card.

**Circle with usage ring** shows an M and the current account's session usage, or weekly usage when session data is unavailable. Hover shows the account name, both usage windows, and reset countdowns. Click to open the menu.

The window has transparent antialiased edges without an outline. Controls use measured font metrics, consistent padding, and Hand cursors. Navigation uses a short fade; opening and resizing retain spring motion. **Reduce motion** disables animated transitions.

## Usage data

Usage starts automatically on a normal launch, with no in-app consent or Refresh button. The logged-in account is polled every 30 seconds, other saved accounts every two minutes, and current-account identity every five seconds. Save/switch schedules a fresh read. This is near-live polling rather than a server-pushed stream. Errors back off automatically, HTTP 429 honors Retry-After, and authentication failures pause retries for 15 minutes. Percentages describe quota consumed, and hours describe time until reset, not guaranteed remaining working time. Missing readings stay unavailable; errors retain the last successful reading and its timestamp.

The reader decrypts Claude's local Electron v10/AES-GCM cache using Windows DPAPI and selects credentials explicitly bound to the saved account. Fresh Desktop credentials are preferred only when their account ID matches that slot. Read-only HTTPS requests go only to `https://api.anthropic.com/api/oauth/usage`, with redirects disabled and normal Windows certificate validation. Chats/files are not sent, tokens are not printed, and this query does not refresh or modify tokens. Expired sessions require signing into Claude and saving the account again.

This is an unofficial compatibility technique; the endpoint and Claude's storage format may change. Reference: [CodexBar's Claude provider documentation](https://github.com/steipete/CodexBar/blob/main/docs/claude.md). Muisland's implementation is independent.

## Work and local backups

Switching changes authentication data, not server-side account ownership. Cloud chats and Cowork sessions stay with their original account and are not transferred to another account. Local running tasks or unsaved work can be interrupted by closing Claude. Cloud tasks may continue, but local-file access needs the desktop connection. See [Claude's Cowork documentation](https://support.claude.com/en/articles/15520349-use-claude-cowork-on-web-desktop-and-mobile).

**Back up local data before switch** creates an encrypted, chunked checkpoint of ordinary files in the detected Claude app-data folder, with Claude fully closed. If checkpointing fails, switching is cancelled. Large folders take time and disk space; links and junctions are rejected. It does not export cloud chats, copy external project folders, or capture a running process/VM in memory.

Preferences > **Recover local backup to folder** decrypts the latest complete checkpoint into a separate folder, leaving Claude's live data untouched. Recovered files contain sensitive application data and are not automatically imported into Claude. **Restore previous sign-in** is separate and restores the auth-only backup from the last switch.

## Local storage

Data remains under `%LOCALAPPDATA%\ClaudeIsland` for compatibility:

- `account-N`: encrypted sign-in snapshots.
- `muisland-settings.bin`: encrypted names, quota cache, and preferences.
- `local-backups`: encrypted local checkpoints.
- `removed-accounts`: archived snapshots from removed entries.
- `saved-versions`: previous encrypted snapshots retained before a successful replacement.
- `recovered`: separately decrypted checkpoint output.

Windows DPAPI ties encryption to the current Windows user. It does not protect against software running as that user. Filesystem restore is best effort rather than transactional.

## Verification and diagnostics

```powershell
cargo test --offline
cargo fmt --check
cargo build --release --offline
```

Offline builds require dependencies to be cached. Tests cover account identity isolation, stable slots during removal, empty lists, independent scales, header hit targets, switch confirmation ordering, quota/reset parsing, known-vector AES-GCM, and encrypted backup/recovery safety.

- `Muisland.exe --preview`: labelled sample data; no credential reads, network requests, Claude closure, saved snapshots, or persisted preference changes.
- `Muisland.exe --no-network`: real interface with usage requests disabled for that process only.
- `Muisland.exe --render-previews <folder>`: export sample renderings without opening a window.
- `Muisland.exe --check-usage`: read-only diagnostic printing parsed quota data for the current account, never credentials.

Generated binaries, `target`, previews, and local data are ignored by Git. Curated documentation images can be kept separately under `docs` if needed.

## Session-safe saving and screen limits

Saving validates the local signed-in account before closure and again before replacing a slot. If login data is missing or the account changed, the existing slot remains intact. Snapshot replacement retains the previous encrypted copy and attempts rollback on installation failure. Claude is reopened after the capture attempt even if capture failed, so the user can finish signing in. A login revoked by Claude itself cannot be recreated by a local backup.

Relaunching reuses the running app's Windows application identity or executable, including Microsoft Store activation, instead of assuming the `claude://` association points to the same installation. Closing uses native Exit/Quit menu commands with WM_CLOSE as a fallback; no taskkill /F is used. Save operations never write to Claude's active authentication files.

Dragging, expansion, hover, and size changes keep the whole island inside the nearest monitor's usable work area with an 8-pixel margin. Menus that exceed a small display fit automatically. Monitor removal, resolution changes, and taskbar/work-area changes re-check the bounds. Negative-coordinate secondary monitors are supported.

The close/reopen sequence and snapshot rollback are verified with offline tests rather than terminating a live user session. Screen clamping is tested at each edge, on a secondary monitor, and with oversized menus.


Automatic usage uses each user's own saved Claude session on that Windows account. No developer or previous user's credentials or consent are bundled or copied. Any authorization prompts from an external tool used to test the application are separate from Muisland's normal runtime behavior. The application does not click or accept operating-system or third-party permission dialogs on the user's behalf.


## Circle drag and outside-click dismissal

The collapsed pill/circle can be dragged. The circle stays fixed during hover; a separate rounded usage bubble appears beside it with a pointer, choosing left or right according to available monitor space and clamping vertically to the work area. Moving the pointer onto the card keeps it visible, and clicking it opens the menu. The card fades out when leaving both surfaces and hides during dragging. Dragging does not activate the open-menu click. The open menu remains draggable only from its header.
Clicking outside closes the open menu or hover card. Dismissal abandons pending, unconfirmed switches, does not auto-save unfinished account-name edits, and does not execute confirmations. Previously confirmed background operations finish safely, but their completion does not reopen a dismissed menu. Focus deactivation handles ordinary clicks; a mouse-state check covers outside clicks on non-activating surfaces. The app does not install a global input hook or log external clicks.
