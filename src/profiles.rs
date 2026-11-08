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
    &["Local State", "Network", "Local Storage", "Session Storage"]
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

// Called only after the user chooses Close & save inside the island.
// Never capture until all Claude processes are confirmed stopped.
pub fn close_claude_for_save() -> io::Result<()> {
    use std::os::windows::process::CommandExt;
    use std::time::{Duration, Instant};
    if !claude_running() {
        return Ok(());
    }
    let result = Command::new("taskkill")
        .args(["/IM", "Claude.exe", "/T", "/F"])
        .creation_flags(0x08000000)
        .output()?;
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if !claude_running() {
            return Ok(());
        }
        if !result.status.success() || Instant::now() >= deadline {
            return Err(io::Error::other("Claude could not be closed. Nothing was saved. Close Claude manually and try again."));
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

pub fn claude_running() -> bool {
    use std::os::windows::process::CommandExt;
    if let Ok(out) = Command::new("tasklist")
        .args(["/FI", "IMAGENAME eq Claude.exe", "/NH"])
        .creation_flags(0x08000000)
        .output()
    {
        if !out.status.success() {
            return true;
        }
        let s = String::from_utf8_lossy(&out.stdout).to_lowercase();
        return s.lines().any(|l| l.trim_start().starts_with("claude.exe"));
    }
    // Fail closed if process status can't be determined.
    true
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

pub fn saved(index: usize) -> bool {
    account_dir(index)
        .map(|p| p.join("auth.json").is_file())
        .unwrap_or(false)
}
pub fn capture(index: usize) -> io::Result<()> {
    if !(1..=2).contains(&index) {
        return Err(io::Error::other("Invalid account"));
    }
    check_closed()?;
    let source = find_claude()?;
    let dest = account_dir(index)?;
    let stage = dest.with_extension("stage");
    save_snapshot(&source, &stage)?;
    if dest.exists() {
        fs::remove_dir_all(&dest)?;
    }
    fs::rename(stage, dest)?;
    Ok(())
}
pub fn switch(index: usize, protect_local: bool) -> io::Result<()> {
    if !(1..=2).contains(&index) {
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
pub fn open_claude() {
    // AppUserModelID varies across releases. Use the app URI association as best effort.
    let _ = Command::new("explorer.exe").arg("claude://").spawn();
}
