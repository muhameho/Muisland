//! Codex file-auth profiles and read-only official app-server quota requests.
use crate::{
    accounts, profiles,
    usage::{Usage, Window},
};
use serde_json::{json, Value};
use std::{
    fs,
    io::{self, BufRead, BufReader, Write},
    path::PathBuf,
    process::{Command, Stdio},
    sync::mpsc,
    time::Duration,
};
pub fn home() -> io::Result<PathBuf> {
    std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("USERPROFILE").map(|p| PathBuf::from(p).join(".codex")))
        .ok_or_else(|| io::Error::other("Codex home not found"))
}
fn dir() -> io::Result<PathBuf> {
    Ok(profiles::base()?.join("codex"))
}
fn snapshot(slot: usize) -> io::Result<PathBuf> {
    Ok(dir()?.join(format!("account-{slot}.bin")))
}
fn read_live() -> io::Result<Value> {
    serde_json::from_slice(&fs::read(home()?.join("auth.json"))?).map_err(|_| {
        io::Error::other(
            "Codex sign-in is unreadable. Sign in using Codex CLI with file credential storage.",
        )
    })
}
fn stored(slot: usize) -> io::Result<Value> {
    serde_json::from_slice(&profiles::dpapi(&fs::read(snapshot(slot)?)?, false)?)
        .map_err(|_| io::Error::other("Saved Codex sign-in is invalid"))
}
pub fn identity(auth: &Value) -> io::Result<String> {
    auth["tokens"]["account_id"].as_str().filter(|id|!id.is_empty()).map(str::to_owned).ok_or_else(||io::Error::other("Sign in to Codex with ChatGPT using file credentials. API-key and OS-keyring logins cannot be saved as ChatGPT quota profiles."))
}
pub fn current_identity() -> io::Result<String> {
    identity(&read_live()?)
}
pub fn account_identity(slot: usize) -> io::Result<String> {
    identity(&stored(slot)?)
}
pub fn saved(slot: usize) -> bool {
    snapshot(slot).is_ok_and(|p| p.is_file())
}
pub fn is_current(slot: usize) -> bool {
    account_identity(slot)
        .ok()
        .zip(current_identity().ok())
        .is_some_and(|(a, b)| a == b)
}
fn file_store_policy(config: &str) -> bool {
    for line in config.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            break;
        }
        if let Some((key, value)) = line.split_once('=') {
            if key.trim() == "cli_auth_credentials_store" {
                return value
                    .split('#')
                    .next()
                    .unwrap_or("")
                    .trim()
                    .trim_matches(['\"', '\''])
                    == "file";
            }
        }
    }
    true
}
fn require_file_store() -> io::Result<()> {
    let config = fs::read_to_string(home()?.join("config.toml")).unwrap_or_default();
    if file_store_policy(&config) {
        Ok(())
    } else {
        Err(io::Error::other("Codex is configured for keyring/auto/ephemeral credentials. This file-profile switcher will not change that storage policy."))
    }
}
pub fn save(slot: usize) -> io::Result<()> {
    require_file_store()?;
    let first = fs::read(home()?.join("auth.json"))?;
    let auth: Value = serde_json::from_slice(&first).map_err(io::Error::other)?;
    identity(&auth)?;
    if first != fs::read(home()?.join("auth.json"))? {
        return Err(io::Error::other(
            "Codex refreshed its sign-in while saving. Please retry.",
        ));
    }
    if let Ok(previous) = fs::read(snapshot(slot)?) {
        profiles::write_atomic(
            &dir()?
                .join("saved-versions")
                .join(format!("{slot}-{}.bin", accounts::now())),
            &previous,
        )?;
    }
    profiles::write_atomic(&snapshot(slot)?, &profiles::dpapi(&first, true)?)
}
pub fn switch(slot: usize) -> io::Result<()> {
    require_file_store()?;
    let auth = stored(slot)?;
    identity(&auth)?;
    let target = home()?.join("auth.json");
    if let Ok(old) = fs::read(&target) {
        profiles::write_atomic(&dir()?.join("previous.bin"), &profiles::dpapi(&old, true)?)?;
    }
    profiles::write_atomic(
        &target,
        &serde_json::to_vec_pretty(&auth).map_err(io::Error::other)?,
    )
}
pub fn rollback() -> io::Result<()> {
    require_file_store()?;
    let bytes = profiles::dpapi(&fs::read(dir()?.join("previous.bin"))?, false)?;
    let auth: Value = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
    identity(&auth)?;
    profiles::write_atomic(&home()?.join("auth.json"), &bytes)
}
pub fn archive(slot: usize) -> io::Result<Option<PathBuf>> {
    let path = snapshot(slot)?;
    if !path.exists() {
        return Ok(None);
    }
    let archive = dir()?
        .join("removed")
        .join(format!("{slot}-{}.bin", accounts::now()));
    fs::create_dir_all(archive.parent().unwrap())?;
    fs::rename(path, &archive)?;
    Ok(Some(archive))
}
pub fn restore_archive(slot: usize, archive: &std::path::Path) -> io::Result<()> {
    fs::rename(archive, snapshot(slot)?)
}
fn executable() -> io::Result<PathBuf> {
    if let Some(p) = std::env::var_os("MUISLAND_CODEX_BIN") {
        let p = PathBuf::from(p);
        if p.is_file() {
            return Ok(p);
        }
    }
    if let Some(paths) = std::env::var_os("PATH") {
        for path in std::env::split_paths(&paths) {
            let exe = path.join("codex.exe");
            if exe.is_file() {
                return Ok(exe);
            }
        }
    }
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        let base = PathBuf::from(local).join("OpenAI/Codex/bin");
        if let Ok(entries) = fs::read_dir(base) {
            let mut candidates = entries
                .flatten()
                .map(|p| p.path().join("codex.exe"))
                .filter(|p| p.is_file())
                .collect::<Vec<_>>();
            candidates.sort();
            if let Some(p) = candidates.pop() {
                return Ok(p);
            }
        }
    }
    Err(io::Error::other(
        "Codex CLI not found. Install Codex CLI or set MUISLAND_CODEX_BIN to codex.exe.",
    ))
}
pub fn parse_limits(v: &Value) -> io::Result<Usage> {
    let limits = v
        .get("rateLimitsByLimitId")
        .and_then(|m| m.get("codex"))
        .or_else(|| v.get("rateLimits"))
        .ok_or_else(|| io::Error::other("Codex returned no quota windows"))?;
    let window = |key: &str| {
        limits[key].as_object().and_then(|w| {
            let used = w.get("usedPercent")?.as_f64()?;
            if !used.is_finite() || !(0.0..=100.0).contains(&used) {
                return None;
            }
            Some(Window {
                used,
                resets: w.get("resetsAt").and_then(Value::as_u64),
            })
        })
    };
    let session = window("primary");
    let weekly = window("secondary");
    if session.is_none() && weekly.is_none() {
        return Err(io::Error::other("This Codex account has no usage readings"));
    }
    Ok(Usage {
        session,
        weekly,
        updated: accounts::now(),
        plan: limits["planType"]
            .as_str()
            .or(v["planType"].as_str())
            .unwrap_or("Codex")
            .into(),
    })
}
pub fn fetch_current() -> io::Result<Usage> {
    fetch_auth(read_live()?)
}
pub fn fetch(slot: usize) -> io::Result<Usage> {
    let mut auth = stored(slot)?;
    if let Ok(live) = read_live() {
        if identity(&live).ok() == identity(&auth).ok() {
            auth = live;
        }
    }
    fetch_auth(auth)
}
fn fetch_auth(auth: Value) -> io::Result<Usage> {
    let id = identity(&auth)?;
    let access = auth["tokens"]["access_token"]
        .as_str()
        .ok_or_else(|| io::Error::other("Codex access token unavailable"))?;
    let mut command = Command::new(executable()?);
    command
        .args([
            "app-server",
            "--listen",
            "stdio://",
            "-c",
            "cli_auth_credentials_store=\"ephemeral\"",
            "-c",
            "analytics.enabled=false",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    use std::os::windows::process::CommandExt;
    command.creation_flags(0x08000000);
    let mut child = command.spawn()?;
    let mut input = child.stdin.take().unwrap();
    let output = child.stdout.take().unwrap();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(output).lines().map_while(Result::ok) {
            if let Ok(v) = serde_json::from_str::<Value>(&line) {
                if tx.send(v).is_err() {
                    break;
                }
            }
        }
    });
    let result = (|| {
        let mut call = |method: &str, params: Value, n: u64| -> io::Result<Value> {
            writeln!(input, "{}", json!({"id":n,"method":method,"params":params}))?;
            input.flush()?;
            let deadline = std::time::Instant::now() + Duration::from_secs(25);
            loop {
                let msg = rx
                    .recv_timeout(deadline.saturating_duration_since(std::time::Instant::now()))
                    .map_err(|_| io::Error::other("Codex usage request timed out"))?;
                if msg["id"].as_u64() == Some(n) {
                    if msg.get("error").is_some() {
                        return Err(io::Error::other("Codex could not read usage. Your session may have expired; sign in and save again."));
                    }
                    if n == 1 {
                        writeln!(input, "{}", json!({"method":"initialized"}))?;
                        input.flush()?;
                    }
                    return Ok(msg["result"].clone());
                }
                if msg.get("method").is_some() && msg.get("id").is_some() {
                    writeln!(
                        input,
                        "{}",
                        json!({"id":msg["id"],"error":{"code":-32601,"message":"Read-only usage client; token refresh unavailable"}})
                    )?;
                    input.flush()?;
                }
            }
        };
        call(
            "initialize",
            json!({"clientInfo":{"name":"muisland","version":"0.4.0"},"capabilities":{"experimentalApi":true}}),
            1,
        )?;
        call(
            "account/login/start",
            json!({"type":"chatgptAuthTokens","accessToken":access,"chatgptAccountId":id}),
            2,
        )?;
        parse_limits(&call("account/rateLimits/read", json!({}), 3)?)
    })();
    let _ = child.kill();
    let _ = child.wait();
    result
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn codex_quotas_keep_missing_windows_unavailable() {
        let u=parse_limits(&json!({"rateLimits":{"primary":{"usedPercent":34,"resetsAt":2000000000},"secondary":null}})).unwrap();
        assert_eq!(u.session.unwrap().used, 34.);
        assert!(u.weekly.is_none());
        assert!(parse_limits(&json!({"rateLimits":{"primary":{"usedPercent":234}}})).is_err());
    }
    #[test]
    fn file_profiles_do_not_override_keyring_policy() {
        assert!(file_store_policy(""));
        assert!(file_store_policy("cli_auth_credentials_store = \"file\""));
        assert!(!file_store_policy(
            "cli_auth_credentials_store = \"keyring\""
        ));
        assert!(!file_store_policy("cli_auth_credentials_store = \"auto\""));
    }
    #[test]
    fn rejects_api_key_only_auth() {
        assert!(identity(&json!({"OPENAI_API_KEY":"example"})).is_err());
    }
}
