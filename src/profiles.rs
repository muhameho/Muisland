//! Claude Desktop auth-profile snapshots. No networking; DPAPI encrypted on disk.
//! This is an unofficial compatibility layer; Claude can change its storage format.
use serde_json::{Map, Value};
use std::{
    env, fs, io,
    path::{Path, PathBuf},
    process::Command,
};

#[repr(C)]
struct Blob {
    len: u32,
    data: *mut u8,
}
#[link(name = "crypt32")]
extern "system" {
    fn CryptProtectData(
        input: *const Blob,
        description: *const u16,
        entropy: *const Blob,
        reserved: *mut core::ffi::c_void,
        prompt: *mut core::ffi::c_void,
        flags: u32,
        output: *mut Blob,
    ) -> i32;
    fn CryptUnprotectData(
        input: *const Blob,
        description: *mut *mut u16,
        entropy: *const Blob,
        reserved: *mut core::ffi::c_void,
        prompt: *mut core::ffi::c_void,
        flags: u32,
        output: *mut Blob,
    ) -> i32;
}
#[link(name = "kernel32")]
extern "system" {
    fn LocalFree(ptr: *mut core::ffi::c_void) -> *mut core::ffi::c_void;
}

fn json<T>(result: serde_json::Result<T>) -> io::Result<T> {
    result.map_err(io::Error::other)
}
pub fn dpapi(bytes: &[u8], encrypt: bool) -> io::Result<Vec<u8>> {
    let input = Blob {
        len: bytes.len() as u32,
        data: bytes.as_ptr() as *mut u8,
    };
    let mut output = Blob {
        len: 0,
        data: std::ptr::null_mut(),
    };
    let ok = unsafe {
        if encrypt {
            CryptProtectData(
                &input,
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                1,
                &mut output,
            )
        } else {
            CryptUnprotectData(
                &input,
                std::ptr::null_mut(),
                std::ptr::null(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                1,
                &mut output,
            )
        }
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    let result = unsafe { std::slice::from_raw_parts(output.data, output.len as usize).to_vec() };
    unsafe {
        LocalFree(output.data.cast());
    }
    Ok(result)
}
pub fn base() -> io::Result<PathBuf> {
    Ok(PathBuf::from(
        env::var_os("LOCALAPPDATA").ok_or_else(|| io::Error::other("LOCALAPPDATA missing"))?,
    )
    .join("ClaudeIsland"))
}
pub fn account_dir(index: usize) -> io::Result<PathBuf> {
    Ok(base()?.join(format!("account-{index}")))
}
fn auth_items() -> &'static [&'static str] {
    &[
        "Local State",
        "Network",
        "Local Storage",
        "Session Storage",
        "Cookies",
        "Cookies-wal",
        "Cookies-shm",
        "Cookies-journal",
    ]
}
fn config_keys() -> &'static [&'static str] {
    &[
        "oauth:tokenCacheV2",
        "oauth:tokenCache",
        "lastKnownAccountUuid",
    ]
}

pub fn find_claude() -> io::Result<PathBuf> {
    let local = PathBuf::from(
        env::var_os("LOCALAPPDATA").ok_or_else(|| io::Error::other("LOCALAPPDATA missing"))?,
    );
    let roaming =
        PathBuf::from(env::var_os("APPDATA").ok_or_else(|| io::Error::other("APPDATA missing"))?);
    // Newer Store builds: LOCALAPPDATA/Claude-Data/claude_...; old Store builds: Packages/.../LocalCache/Roaming/Claude.
    let modern = local.join("Claude-Data");
    if modern.is_dir() {
        let mut candidates = fs::read_dir(modern)?
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| {
                p.is_dir()
                    && p.file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .starts_with("claude_")
            })
            .collect::<Vec<_>>();
        candidates.sort();
        if let Some(p) = candidates
            .into_iter()
            .find(|p| p.join("config.json").exists())
        {
            return Ok(p);
        }
    }
    let legacy = local
        .join("Packages")
        .join("Claude_pzs8sxrjxfjjc")
        .join("LocalCache")
        .join("Roaming")
        .join("Claude");
    if legacy.join("config.json").exists() {
        return Ok(legacy);
    }
    let standalone = roaming.join("Claude");
    if standalone.join("config.json").exists() {
        return Ok(standalone);
    }
    Err(io::Error::new(
        io::ErrorKind::NotFound,
        "Claude config.json not found in known locations",
    ))
}

#[derive(Clone)]
pub enum LaunchTarget {
    Store(String),
    Executable(PathBuf),
    Uri,
}
impl LaunchTarget {
    pub fn open(&self) -> io::Result<()> {
        match self {
            Self::Store(id) => {
                Command::new("explorer.exe")
                    .arg(format!("shell:AppsFolder\\{id}"))
                    .spawn()?;
            }
            Self::Executable(path) => {
                Command::new(path).spawn()?;
            }
            Self::Uri => {
                Command::new("explorer.exe").arg("claude://").spawn()?;
            }
        }
        Ok(())
    }
}
fn is_desktop_executable(path: &Path) -> bool {
    let p = path.to_string_lossy().to_lowercase();
    ![
        "\\claude-code\\",
        "\\.claude\\",
        "\\.local\\bin\\",
        "\\node_modules\\",
        "\\roaming\\npm\\",
    ]
    .iter()
    .any(|part| p.contains(part))
}
fn desktop_processes() -> io::Result<Vec<(u32, PathBuf)>> {
    use windows_sys::Win32::{
        Foundation::{CloseHandle, INVALID_HANDLE_VALUE},
        System::{Diagnostics::ToolHelp::*, Threading::*},
    };
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snapshot == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        let result = (|| {
            let mut entry: PROCESSENTRY32W = std::mem::zeroed();
            entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
            let mut processes = Vec::new();
            let mut found = Process32FirstW(snapshot, &mut entry);
            while found != 0 {
                let end = entry
                    .szExeFile
                    .iter()
                    .position(|c| *c == 0)
                    .unwrap_or(entry.szExeFile.len());
                if String::from_utf16_lossy(&entry.szExeFile[..end])
                    .eq_ignore_ascii_case("claude.exe")
                {
                    let process =
                        OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, entry.th32ProcessID);
                    if process.is_null() {
                        return Err(io::Error::other("Cannot inspect a Claude process. Close Claude Desktop manually before retrying."));
                    }
                    let mut name = vec![0u16; 32768];
                    let mut count = name.len() as u32;
                    let ok = QueryFullProcessImageNameW(process, 0, name.as_mut_ptr(), &mut count);
                    CloseHandle(process);
                    if ok == 0 {
                        return Err(io::Error::other("Cannot identify a Claude process safely."));
                    }
                    let path = PathBuf::from(String::from_utf16_lossy(&name[..count as usize]));
                    if is_desktop_executable(&path) {
                        processes.push((entry.th32ProcessID, path));
                    }
                }
                found = Process32NextW(snapshot, &mut entry);
            }
            Ok(processes)
        })();
        CloseHandle(snapshot);
        result
    }
}
pub fn claude_launch_target() -> LaunchTarget {
    use windows_sys::Win32::{Foundation::CloseHandle, System::Threading::*};
    #[link(name = "kernel32")]
    extern "system" {
        fn GetApplicationUserModelId(
            process: *mut core::ffi::c_void,
            length: *mut u32,
            id: *mut u16,
        ) -> i32;
    }
    if let Ok(processes) = desktop_processes() {
        for (pid, path) in processes {
            unsafe {
                let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
                if !process.is_null() {
                    let mut n = 0;
                    GetApplicationUserModelId(process, &mut n, std::ptr::null_mut());
                    if n > 1 && n < 4096 {
                        let mut id = vec![0; n as usize];
                        let ok = GetApplicationUserModelId(process, &mut n, id.as_mut_ptr());
                        CloseHandle(process);
                        if ok == 0 {
                            let length = id.iter().position(|c| *c == 0).unwrap_or(id.len());
                            return LaunchTarget::Store(String::from_utf16_lossy(&id[..length]));
                        }
                    } else {
                        CloseHandle(process);
                    }
                }
            }
            return LaunchTarget::Executable(path);
        }
    }
    if let Ok(dir) = find_claude() {
        if dir
            .to_string_lossy()
            .to_lowercase()
            .contains("claude_pzs8sxrjxfjjc")
        {
            return LaunchTarget::Store("Claude_pzs8sxrjxfjjc!Claude".into());
        }
    }
    LaunchTarget::Uri
}
unsafe fn quit_menu_command(
    menu: windows_sys::Win32::UI::WindowsAndMessaging::HMENU,
    depth: u32,
) -> Option<u32> {
    use windows_sys::Win32::UI::WindowsAndMessaging::*;
    if menu.is_null() || depth > 8 {
        return None;
    }
    for position in 0..GetMenuItemCount(menu) {
        let mut label = [0u16; 256];
        let count = GetMenuStringW(
            menu,
            position as u32,
            label.as_mut_ptr(),
            label.len() as i32,
            MF_BYPOSITION,
        );
        let name = String::from_utf16_lossy(&label[..count.max(0) as usize]).replace('&', "");
        let name = name.split('\t').next().unwrap_or("").trim().to_lowercase();
        let id = GetMenuItemID(menu, position);
        if [
            "exit",
            "quit",
            "exit claude",
            "quit claude",
            "خروج",
            "إنهاء",
        ]
        .contains(&name.as_str())
            && id != u32::MAX
        {
            return Some(id);
        }
        if let Some(command) = quit_menu_command(GetSubMenu(menu, position), depth + 1) {
            return Some(command);
        }
    }
    None
}
pub fn close_claude_for_save() -> io::Result<()> {
    use std::time::{Duration, Instant};
    use windows_sys::Win32::{Foundation::*, UI::WindowsAndMessaging::*};
    let processes = desktop_processes()?;
    if processes.is_empty() {
        return Ok(());
    }
    let ids = processes.into_iter().map(|(id, _)| id).collect::<Vec<_>>();
    unsafe extern "system" fn close_window(hwnd: HWND, data: LPARAM) -> BOOL {
        let ids = &*(data as *const Vec<u32>);
        let mut pid = 0;
        GetWindowThreadProcessId(hwnd, &mut pid);
        if ids.contains(&pid) {
            if let Some(command) = quit_menu_command(GetMenu(hwnd), 0) {
                PostMessageW(hwnd, WM_COMMAND, command as WPARAM, 0);
            } else if IsWindowVisible(hwnd) != 0 {
                PostMessageW(hwnd, WM_CLOSE, 0, 0);
            }
        }
        1
    }
    unsafe {
        EnumWindows(Some(close_window), (&ids as *const Vec<u32>) as LPARAM);
    }
    let wait = |limit: Duration| -> io::Result<bool> {
        let deadline = Instant::now() + limit;
        loop {
            if desktop_processes()?.is_empty() {
                std::thread::sleep(Duration::from_millis(350));
                if desktop_processes()?.is_empty() {
                    return Ok(true);
                }
            }
            if Instant::now() >= deadline {
                return Ok(false);
            }
            std::thread::sleep(Duration::from_millis(150));
        }
    };
    if wait(Duration::from_secs(15))? {
        return Ok(());
    }
    Err(io::Error::other("Claude is still running in its tray or finishing work. Quit it normally from its tray, then retry. No account files were changed and Muisland did not force-close it."))
}
pub fn claude_running() -> bool {
    desktop_processes().map(|p| !p.is_empty()).unwrap_or(true)
}
fn check_closed() -> io::Result<()> {
    if claude_running() {
        Err(io::Error::other(
            "Close Claude Desktop completely (including system tray), then retry.",
        ))
    } else {
        Ok(())
    }
}
pub fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("island-tmp");
    fs::write(&tmp, bytes)?;
    fs::rename(tmp, path)?;
    Ok(())
}
fn copy_encrypted(src: &Path, dst: &Path, encrypt: bool) -> io::Result<()> {
    if src.is_dir() {
        fs::create_dir_all(dst)?;
        for item in fs::read_dir(src)? {
            let item = item?;
            let name = item.file_name();
            if item.file_type()?.is_symlink() {
                return Err(io::Error::other("Refusing symlink in session data"));
            }
            copy_encrypted(&item.path(), &dst.join(name), encrypt)?;
        }
    } else if src.is_file() {
        let raw = fs::read(src)?;
        write_atomic(dst, &dpapi(&raw, encrypt)?)?;
    }
    Ok(())
}
fn save_snapshot(source: &Path, dest: &Path) -> io::Result<()> {
    if dest.exists() {
        fs::remove_dir_all(dest)?;
    }
    fs::create_dir_all(dest)?;
    for item in auth_items() {
        let p = source.join(item);
        if p.exists() {
            copy_encrypted(&p, &dest.join(item), true)?;
        }
    }
    let config: Value = json(serde_json::from_slice(&fs::read(
        source.join("config.json"),
    )?))?;
    let mut selected = Map::new();
    for key in config_keys() {
        if let Some(v) = config.get(*key) {
            selected.insert((*key).to_string(), v.clone());
        }
    }
    if selected.is_empty() {
        return Err(io::Error::other(
            "No supported authentication fields found in config.json",
        ));
    }
    write_atomic(
        &dest.join("auth.json"),
        &dpapi(&json(serde_json::to_vec(&Value::Object(selected)))?, true)?,
    )?;
    Ok(())
}
fn restore_snapshot(snapshot: &Path, dest: &Path) -> io::Result<()> {
    let selected: Value = json(serde_json::from_slice(&dpapi(
        &fs::read(snapshot.join("auth.json"))?,
        false,
    )?))?;
    let mut config: Value = json(serde_json::from_slice(&fs::read(dest.join("config.json"))?))?;
    let object = config
        .as_object_mut()
        .ok_or_else(|| io::Error::other("Invalid Claude config"))?;
    for key in config_keys() {
        object.remove(*key);
    }
    for (k, v) in selected
        .as_object()
        .ok_or_else(|| io::Error::other("Invalid auth snapshot"))?
    {
        if config_keys().contains(&k.as_str()) {
            object.insert(k.clone(), v.clone());
        }
    }
    // Decode all data before changing Claude's files.
    let mut files: Vec<(PathBuf, Vec<u8>)> = Vec::new();
    for item in auth_items() {
        let folder = snapshot.join(item);
        if folder.exists() {
            collect_decrypted(&folder, &dest.join(item), &mut files)?;
        }
    }
    for item in auth_items() {
        let path = dest.join(item);
        if path.is_dir() {
            fs::remove_dir_all(path)?;
        } else if path.is_file() {
            fs::remove_file(path)?;
        }
    }
    for (p, b) in files {
        write_atomic(&p, &b)?;
    }
    write_atomic(
        &dest.join("config.json"),
        &json(serde_json::to_vec_pretty(&config))?,
    )?;
    Ok(())
}
fn collect_decrypted(src: &Path, dst: &Path, out: &mut Vec<(PathBuf, Vec<u8>)>) -> io::Result<()> {
    if src.is_dir() {
        for item in fs::read_dir(src)? {
            let item = item?;
            if item.file_type()?.is_symlink() {
                return Err(io::Error::other("Symlink not supported"));
            }
            collect_decrypted(&item.path(), &dst.join(item.file_name()), out)?;
        }
    } else if src.is_file() {
        out.push((dst.to_path_buf(), dpapi(&fs::read(src)?, false)?));
    }
    Ok(())
}

// Windows ships SQLite with the OS; keep the native build dependency-free.
#[link(name = "winsqlite3")]
unsafe extern "C" {
    fn sqlite3_open_v2(
        path: *const i8,
        db: *mut *mut core::ffi::c_void,
        flags: i32,
        vfs: *const i8,
    ) -> i32;
    fn sqlite3_prepare_v2(
        db: *mut core::ffi::c_void,
        sql: *const i8,
        bytes: i32,
        statement: *mut *mut core::ffi::c_void,
        tail: *mut *const i8,
    ) -> i32;
    fn sqlite3_step(statement: *mut core::ffi::c_void) -> i32;
    fn sqlite3_finalize(statement: *mut core::ffi::c_void) -> i32;
    fn sqlite3_close(db: *mut core::ffi::c_void) -> i32;
}
pub fn session_cookie_present(root: &Path) -> bool {
    for relative in ["Network/Cookies", "Cookies"] {
        let path = root.join(relative);
        if !path.is_file() || path.is_symlink() {
            continue;
        }
        let Ok(path) = std::ffi::CString::new(path.to_string_lossy().as_bytes()) else {
            continue;
        };
        let mut db = std::ptr::null_mut();
        let mut statement = std::ptr::null_mut();
        // Names only. This read-only connection includes live WAL records.
        let sql = c"SELECT 1 FROM cookies WHERE name='sessionKey' AND host_key IN ('claude.ai','.claude.ai','claude.com','.claude.com') LIMIT 1";
        let found = unsafe {
            let opened = sqlite3_open_v2(path.as_ptr(), &mut db, 1, std::ptr::null()) == 0;
            let ready = opened
                && sqlite3_prepare_v2(db, sql.as_ptr(), -1, &mut statement, std::ptr::null_mut())
                    == 0;
            let row = ready && sqlite3_step(statement) == 100;
            if !statement.is_null() {
                sqlite3_finalize(statement);
            }
            if !db.is_null() {
                sqlite3_close(db);
            }
            row
        };
        if found {
            return true;
        }
    }
    false
}

pub fn account_identity(index: usize) -> io::Result<String> {
    let config: Value = json(serde_json::from_slice(&dpapi(
        &fs::read(account_dir(index)?.join("auth.json"))?,
        false,
    )?))?;
    config["lastKnownAccountUuid"]
        .as_str()
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| io::Error::other("Save this account again to identify it"))
}
pub fn is_current(index: usize) -> io::Result<bool> {
    Ok(account_identity(index)? == crate::usage::current_account_identity()?)
}
/// Removing a slot is reversible: archive its encrypted snapshot, never touch Claude.
pub fn archive_account(slot: usize) -> io::Result<Option<PathBuf>> {
    if !(1..=crate::accounts::MAX_ACCOUNTS).contains(&slot) {
        return Err(io::Error::other("Invalid account slot"));
    }
    let source = account_dir(slot)?;
    if !source.exists() {
        return Ok(None);
    }
    let root = base()?.join("removed-accounts");
    fs::create_dir_all(&root)?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let dest = root.join(format!("account-{slot}-{stamp}"));
    fs::rename(source, &dest)?;
    Ok(Some(dest))
}
pub fn restore_archived_account(slot: usize, path: &Path) -> io::Result<()> {
    let root = base()?.join("removed-accounts");
    if path.parent() != Some(root.as_path()) {
        return Err(io::Error::other("Invalid archive path"));
    }
    fs::rename(path, account_dir(slot)?)
}
pub fn saved(index: usize) -> bool {
    account_dir(index)
        .map(|p| p.join("auth.json").is_file())
        .unwrap_or(false)
}
pub fn capture(index: usize, expected_identity: &str) -> io::Result<()> {
    if !(1..=crate::accounts::MAX_ACCOUNTS).contains(&index) {
        return Err(io::Error::other("Invalid account"));
    }
    check_closed()?;
    let identity=crate::usage::current_account_identity().map_err(|_|io::Error::other("Claude's signed-in session is not available on disk. Your existing saved account was kept. Sign in to Claude again, let it finish loading, then retry."))?;
    if identity != expected_identity {
        return Err(io::Error::other("Claude's account changed while closing. Your existing saved account was kept; sign in and retry."));
    }
    let source = find_claude()?;
    let dest = account_dir(index)?;
    let stage = dest.with_extension("stage");
    save_snapshot(&source, &stage)?;
    install_snapshot(&stage, &dest, &base()?.join("saved-versions"), index)?;
    Ok(())
}
fn install_snapshot(stage: &Path, dest: &Path, versions: &Path, index: usize) -> io::Result<()> {
    let archived = if dest.exists() {
        fs::create_dir_all(versions)?;
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let previous = versions.join(format!("account-{index}-{stamp}"));
        fs::rename(&dest, &previous)?;
        prune_versions(versions, index);
        Some(previous)
    } else {
        None
    };
    if let Err(error) = fs::rename(&stage, &dest) {
        if let Some(previous) = archived {
            let _ = fs::rename(previous, &dest);
        }
        return Err(error);
    }
    Ok(())
}
/// Superseded copies are a safety net, not an archive: keep the newest three per slot.
fn prune_versions(versions: &Path, index: usize) {
    let prefix = format!("account-{index}-");
    let Ok(entries) = fs::read_dir(versions) else {
        return;
    };
    let mut stamps = entries
        .filter_map(Result::ok)
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let stamp = name.strip_prefix(&prefix)?.parse::<u128>().ok()?;
            Some((stamp, e.path()))
        })
        .collect::<Vec<_>>();
    stamps.sort();
    let extra = stamps.len().saturating_sub(3);
    for (_, path) in stamps.into_iter().take(extra) {
        let _ = fs::remove_dir_all(path);
    }
}
/// The saved slot, other than `except`, that already holds this Claude account.
pub fn slot_holding(identity: &str, except: usize) -> Option<usize> {
    (1..=crate::accounts::MAX_ACCOUNTS)
        .filter(|slot| *slot != except && saved(*slot))
        .find(|slot| account_identity(*slot).ok().as_deref() == Some(identity))
}
/// Claude renews its session while it runs, so the copy taken at the last Save can
/// stop working. Re-capture the signed-in account into its own slot before leaving it.
fn refresh_current_slot(except: usize) {
    // Requires a usable sign-in on disk, so a signed-out Claude never replaces a good copy.
    if let Ok(identity) = crate::usage::current_account_identity() {
        if let Some(slot) = slot_holding(&identity, except) {
            let _ = capture(slot, &identity);
        }
    }
}
fn clear_sign_in(dest: &Path) -> io::Result<()> {
    let mut config: Value = json(serde_json::from_slice(&fs::read(dest.join("config.json"))?))?;
    let object = config
        .as_object_mut()
        .ok_or_else(|| io::Error::other("Invalid Claude config"))?;
    for key in config_keys() {
        object.remove(*key);
    }
    // "Local State" holds this computer's storage key, not a sign-in; it stays.
    for item in auth_items()
        .iter()
        .copied()
        .filter(|item| *item != "Local State")
    {
        let path = dest.join(item);
        if path.is_dir() {
            fs::remove_dir_all(path)?;
        } else if path.is_file() {
            fs::remove_file(path)?;
        }
    }
    write_atomic(
        &dest.join("config.json"),
        &json(serde_json::to_vec_pretty(&config))?,
    )
}
/// Make room for another account without Claude's own Log out, which ends the
/// session on Anthropic's side and would make the saved copy useless. This only
/// forgets the sign-in on this computer after refreshing the saved copy.
pub fn sign_in_another(saved_slot: usize, identity: &str) -> io::Result<()> {
    check_closed()?;
    capture(saved_slot, identity)?;
    let dest = find_claude()?;
    let backup = base()?.join("last-switch-backup");
    save_snapshot(&dest, &backup)?;
    if let Err(error) = clear_sign_in(&dest) {
        let restored = restore_snapshot(&backup, &dest).is_ok();
        return Err(io::Error::other(format!("Could not prepare another sign-in: {error}. Previous session restored: {restored}. Backup kept.")));
    }
    Ok(())
}
pub fn switch(index: usize, protect_local: bool) -> io::Result<()> {
    if is_current(index).unwrap_or(false) {
        return Err(io::Error::other("This account is already logged in."));
    }
    if !(1..=crate::accounts::MAX_ACCOUNTS).contains(&index) {
        return Err(io::Error::other("Invalid account"));
    }
    check_closed()?;
    let dest = find_claude()?;
    let profile = account_dir(index)?;
    if !profile.join("auth.json").exists() {
        return Err(io::Error::other("Save this account first"));
    }
    if protect_local {
        crate::backup::checkpoint(&dest)?;
        check_closed()?;
    }
    refresh_current_slot(index);
    let backup = base()?.join("last-switch-backup");
    save_snapshot(&dest, &backup)?;
    if let Err(err) = restore_snapshot(&profile, &dest) {
        let recovery = restore_snapshot(&backup, &dest);
        return Err(io::Error::other(format!(
            "Switch failed: {err}. Rollback: {}",
            if recovery.is_ok() {
                "OK"
            } else {
                "FAILED — backup preserved"
            }
        )));
    }
    Ok(())
}
pub fn restore_backup() -> io::Result<()> {
    check_closed()?;
    let backup = base()?.join("last-switch-backup");
    if !backup.join("auth.json").exists() {
        return Err(io::Error::other("No switch backup exists"));
    }
    restore_snapshot(&backup, &find_claude()?)
}

fn finish_save(
    save: impl FnOnce() -> io::Result<()>,
    reopen: impl FnOnce() -> io::Result<()>,
) -> io::Result<()> {
    let saved = save();
    let opened = reopen();
    match (saved, opened) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(e), Ok(())) => Err(e),
        (Ok(()), Err(_)) => Err(io::Error::other(
            "Account saved, but Claude could not be reopened. Open Claude manually.",
        )),
        (Err(e), Err(_)) => Err(io::Error::other(format!(
            "{e} Open Claude manually to continue."
        ))),
    }
}
pub fn save_and_reopen(index: usize, expected: &str, target: &LaunchTarget) -> io::Result<()> {
    finish_save(|| capture(index, expected), || target.open())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn save_reopens_claude_on_success_and_on_capture_failure() {
        use std::cell::RefCell;
        let events = RefCell::new(Vec::new());
        assert!(finish_save(
            || {
                events.borrow_mut().push("save");
                Ok(())
            },
            || {
                events.borrow_mut().push("reopen");
                Ok(())
            }
        )
        .is_ok());
        assert_eq!(*events.borrow(), ["save", "reopen"]);
        events.borrow_mut().clear();
        assert!(finish_save(
            || {
                events.borrow_mut().push("save");
                Err(io::Error::other("No valid session"))
            },
            || {
                events.borrow_mut().push("reopen");
                Ok(())
            }
        )
        .is_err());
        assert_eq!(*events.borrow(), ["save", "reopen"]);
    }
    #[test]
    fn replacing_a_snapshot_preserves_old_copy_and_failed_install_rolls_back() {
        let root = std::env::temp_dir().join(format!(
            "muisland-save-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&root).unwrap();
        let dest = root.join("account");
        let stage = root.join("stage");
        let versions = root.join("versions");
        fs::create_dir(&dest).unwrap();
        fs::write(dest.join("auth.bin"), b"old").unwrap();
        fs::create_dir(&stage).unwrap();
        fs::write(stage.join("auth.bin"), b"new").unwrap();
        install_snapshot(&stage, &dest, &versions, 1).unwrap();
        assert_eq!(fs::read(dest.join("auth.bin")).unwrap(), b"new");
        let archived = fs::read_dir(&versions)
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        assert_eq!(fs::read(archived.join("auth.bin")).unwrap(), b"old");
        assert!(install_snapshot(&root.join("missing"), &dest, &versions, 1).is_err());
        assert_eq!(fs::read(dest.join("auth.bin")).unwrap(), b"new");
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn superseded_copies_are_pruned_per_slot_and_sign_in_is_cleared_locally() {
        let root = std::env::temp_dir().join(format!(
            "muisland-prune-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let versions = root.join("versions");
        for name in [
            "account-1-10",
            "account-1-20",
            "account-1-30",
            "account-1-40",
            "account-1-9",
            "account-12-5",
            "account-2-1",
        ] {
            fs::create_dir_all(versions.join(name)).unwrap();
        }
        prune_versions(&versions, 1);
        let mut left = fs::read_dir(&versions)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        left.sort();
        assert_eq!(
            left,
            [
                "account-1-20",
                "account-1-30",
                "account-1-40",
                "account-12-5",
                "account-2-1"
            ]
        );
        let claude = root.join("claude");
        fs::create_dir_all(claude.join("Network")).unwrap();
        fs::create_dir_all(claude.join("Local Storage").join("leveldb")).unwrap();
        fs::write(claude.join("Network").join("Cookies"), b"session").unwrap();
        fs::write(claude.join("Local State"), b"{}").unwrap();
        fs::write(
            claude.join("config.json"),
            br#"{"oauth:tokenCache":"x","lastKnownAccountUuid":"a","theme":"dark"}"#,
        )
        .unwrap();
        clear_sign_in(&claude).unwrap();
        assert!(!claude.join("Network").exists());
        assert!(!claude.join("Local Storage").exists());
        assert!(claude.join("Local State").exists());
        let config: Value =
            serde_json::from_slice(&fs::read(claude.join("config.json")).unwrap()).unwrap();
        assert_eq!(config, serde_json::json!({"theme":"dark"}));
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn desktop_process_filter_excludes_bundled_and_user_cli() {
        assert!(is_desktop_executable(Path::new(
            r"C:\Program Files\WindowsApps\Claude_4\app\Claude.exe"
        )));
        assert!(!is_desktop_executable(Path::new(
            r"C:\Users\me\AppData\Local\Claude-Data\claude-code\2\claude.exe"
        )));
        assert!(!is_desktop_executable(Path::new(
            r"C:\Users\me\.local\bin\claude.exe"
        )));
    }
    #[test]
    fn quit_menu_never_selects_sign_out() {
        unsafe {
            use windows_sys::Win32::UI::WindowsAndMessaging::*;
            let menu = CreatePopupMenu();
            let label = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<_>>();
            AppendMenuW(menu, MF_STRING, 10, label("Sign out").as_ptr());
            AppendMenuW(menu, MF_STRING, 11, label("Exit fullscreen").as_ptr());
            AppendMenuW(menu, MF_STRING, 12, label("E&xit\tAlt+F4").as_ptr());
            assert_eq!(quit_menu_command(menu, 0), Some(12));
            DestroyMenu(menu);
        }
    }
}

#[cfg(test)]
mod cookie_layout_tests {
    use super::*;
    unsafe extern "C" {
        fn sqlite3_exec(
            db: *mut core::ffi::c_void,
            sql: *const i8,
            callback: *const core::ffi::c_void,
            data: *mut core::ffi::c_void,
            error: *mut *mut i8,
        ) -> i32;
    }
    #[test]
    fn root_and_network_cookies_are_recognised_without_oauth() {
        let root =
            std::env::temp_dir().join(format!("muisland-cookie-layout-{}", std::process::id()));
        std::fs::create_dir_all(root.join("Network")).unwrap();
        let path =
            std::ffi::CString::new(root.join("Cookies").to_string_lossy().as_bytes()).unwrap();
        let mut db = std::ptr::null_mut();
        unsafe {
            assert_eq!(
                sqlite3_open_v2(path.as_ptr(), &mut db, 6, std::ptr::null()),
                0
            );
            assert_eq!(sqlite3_exec(db, c"CREATE TABLE cookies(name TEXT,host_key TEXT); INSERT INTO cookies VALUES('sessionKey','.evilclaude.ai')".as_ptr(), std::ptr::null(), std::ptr::null_mut(), std::ptr::null_mut()),0);
            assert!(!session_cookie_present(&root));
            assert_eq!(
                sqlite3_exec(
                    db,
                    c"INSERT INTO cookies VALUES('sessionKey','.claude.ai')".as_ptr(),
                    std::ptr::null(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut()
                ),
                0
            );
            sqlite3_close(db);
        }
        assert!(session_cookie_present(&root));
        std::fs::rename(root.join("Cookies"), root.join("Network/Cookies")).unwrap();
        assert!(session_cookie_present(&root));
        std::fs::remove_dir_all(root).unwrap();
    }
}
