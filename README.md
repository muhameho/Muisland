# Muisland

Native Windows floating account switcher for Claude Desktop, written in Rust.

## Run

Open `Muisland.exe`. Build with `.\build.ps1` (Rust + Visual Studio C++ Build Tools required).
`Muisland.exe --preview` uses labelled sample quota data and never reads credentials, contacts Anthropic, closes Claude, writes snapshots, or persists preferences.

## Accounts

- Choose **+ Add account** to add a named account slot (up to 64). Two cards are shown per page; arrows navigate the pages.
- Sign into Claude, then choose Save on the intended slot. If Claude is running, the island shows Cancel / Close & save, with an explicit warning about interrupted local work.
- The card's (...) opens an animated details view for renaming and inspecting automatically updated usage. Arabic and other Unicode names are supported.
- Click an account card to switch. Muisland first verifies whether that saved account is already Claude's current account. If so, an **Already logged in** notice appears inside the island, without closing, saving, restoring, or relaunching Claude.
- Otherwise switching requires Claude to be closed; if necessary, the island presents Close & switch confirmation.
- All status, errors, confirmations, settings, and navigation are rendered inside the island. There are no MessageBox dialogs or native popup menus.

## Live usage

There is no Refresh button or connection confirmation. The app automatically reads real quota measurements for saved accounts, refreshes usage every five minutes, and checks the current account every ten seconds. Failed reads retain their original cached timestamp and show an unavailable/stale state. A quota reset countdown is not a guarantee of remaining working hours.

The reader decrypts Electron v10/AES-GCM token caches locally using Windows DPAPI. It selects only the saved account's user:profile credential. Fresh Desktop credentials are used only after matching the saved account UUID. Read-only HTTPS GET requests go only to `https://api.anthropic.com/api/oauth/usage`; redirects are disabled and Windows validates TLS certificates. Tokens are not printed or sent to other hosts; chats and files are not sent. Tokens are not refreshed or modified by this query. An expired or unsupported session requires signing into Claude and saving the slot again.

This uses an unofficial endpoint and can break when Claude changes. The integration was researched against https://github.com/steipete/CodexBar/blob/main/docs/claude.md and implemented independently.
The executable also supports `--check-usage` as a read-only diagnostic for the currently signed-in account. This prints only parsed quota numbers, reset timestamps, plan label, and capture time, never credentials.

## Appearance and interaction

- Main (...) or right-click opens the island's own Preferences view.
- **Menu size** adjusts the open menu from 75% to 160%; **Floating island size** independently adjusts the collapsed pill/circle and hover card from 60% to 180%. Text, hit targets, and rounded geometry follow their own view's scale.
- **Circle with usage ring** changes the collapsed state into a circle with an M and the current account's session-usage ring (weekly usage is used if the session window is unavailable). Hover morphs it into a compact view with the account name, session/weekly usage and reset countdowns; clicking opens all accounts.
- Rounded edges use per-pixel transparency with an antialiased mask. No outline stroke or window region is drawn.
- Open/close, circle hover, screen navigation, size changes, and hover feedback animate; Reduce motion disables these transitions.
- Clickable controls use a Hand cursor; the name field uses an I-beam. Drag the left header to move the island. Tab/Enter/Space navigate controls, Escape goes back/collapses, and name editing supports Ctrl+A/Ctrl+V and Enter to save.

## Work, chats, and backup

Switching changes selected authentication data, not server-side account ownership. Cloud chats and Cowork sessions remain with their original account and are not transferred to another account. Local running tasks and unsaved work can be interrupted by closing Claude. Cloud tasks may continue but local file access needs the desktop connection. See https://support.claude.com/en/articles/15520349-use-claude-cowork-on-web-desktop-and-mobile

**Back up local data before switch** creates an encrypted, chunked checkpoint of ordinary files under the detected Claude app-data folder, with Claude fully closed. A failed checkpoint cancels switching. Large data folders can take time and disk space. Reparse points are rejected. This does not export cloud chats, back up connected project directories outside Claude's app-data folder, or checkpoint a running process/VM in memory.

Preferences > **Recover local backup to folder** decrypts the newest complete checkpoint into a new folder, leaving Claude's live data untouched. It does not automatically import sessions into Claude. The recovered folder contains sensitive application data.
**Restore previous sign-in** restores the auth-only last-switch backup and is separate from local recovery.

## Storage

Encrypted snapshots remain under `%LOCALAPPDATA%\ClaudeIsland` for backwards compatibility. Names, quota caches, scale, circle mode, and reduced-motion preferences are DPAPI-encrypted in `muisland-settings.bin`. Local backups are in `local-backups`, recovery output in `recovered`. DPAPI is tied to this Windows user and does not protect against programs running as that user.

## Validation

Build: `cargo build --release --offline`; format: `cargo fmt --check`; tests: `cargo test --offline`.
Offline tests cover Unicode names, settings migration, additional accounts and limits, hit targets beyond the second account, current-account short circuit, close-confirmation ordering, quota parsing/reset times/account identity isolation, known-vector AES-GCM and tamper rejection, and encrypted large-file backup/recovery safety.

Live authenticated quota fetching is pending explicit approval from the environment's automatic approval reviewer. No rejected live request was executed. The assistant uses only preview mode until that authorization is resolved.

## Latest checks

Nineteen offline tests pass. Actual preview-window checks confirmed the Already logged in notice, in-island settings, 110% scaling, circle collapse and hover details, adding a third account, and Arabic name entry with Enter to save. Rendered PNG previews are under `preview` and contain sample data.
`Muisland.exe --no-network` runs the actual interface with usage network requests disabled for that process only. It retains local account detection, naming, appearance settings, and explicitly requested save/switch operations. This diagnostic session flag is not persisted; a normal launch automatically updates usage. The assistant uses it while live API authorization is pending.

## Account removal and separate sizes

Account details now include **Remove account**, confirmed inside the island. Removing an entry does not log Claude out or delete server-side chats/files. Its encrypted sign-in snapshot is moved to `removed-accounts` under the existing local data directory. Other accounts keep stable storage-slot IDs even when the visible list changes. A reused slot starts without the removed snapshot. Settings-save failures attempt to restore the archived snapshot and keep the account entry. Removing the last entry leaves an empty state with Add account available.

Preferences now have independent **Menu size** (75–160%) and **Floating island size** (60–180%) controls. Existing single-scale settings migrate to both scales. The floating scale covers the collapsed pill/circle and its hover card; opening the menu uses the menu scale.
Buttons use measured Segoe UI text metrics, balanced padding, consistent rounded rectangles and a subdued destructive style. Account pagination uses a short opacity fade without horizontal movement; details and settings use a quick crossfade. Window expansion retains its spring animation.

The latest preview-window checks verified centered button labels, independent 110% menu / 80% floating scales through collapse and reopen, and removing a simulated account while preserving the remaining account. No real account was removed in verification.


## Header and switch confirmation

Clicking anywhere in the open header collapses the island; the explicit (...) settings button keeps opening Preferences. Dragging the left header still moves the window rather than triggering a click.
Selecting a different account always opens **Switch account?** inside the island, even when Claude is already closed. Only **Confirm switch** proceeds; it closes Claude first if necessary. Cancel, Escape, or collapsing the header abandons the prompt without switching. Enter defaults to Cancel. Selecting the current account still reports **Already logged in** without closing or changing Claude.
