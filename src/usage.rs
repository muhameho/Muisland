//! Read-only quota requests. Credentials never leave Anthropic's HTTPS origin.
use crate::{accounts, profiles};
use serde_json::{json, Value};
use std::{
    ffi::c_void,
    fs, io,
    mem::{size_of, zeroed},
    ptr::{null, null_mut},
};
use windows_sys::Win32::{
    Foundation::SYSTEMTIME, Networking::WinHttp::*, Security::Cryptography::*,
    System::Time::SystemTimeToFileTime,
};

#[derive(Clone, Debug)]
pub struct Window {
    pub used: f64,
    pub resets: Option<u64>,
}
#[derive(Clone, Debug)]
pub struct Usage {
    pub session: Option<Window>,
    pub weekly: Option<Window>,
    pub updated: u64,
    pub plan: String,
}
impl Window {
    fn parse(v: &Value, cache: bool) -> Option<Self> {
        let used = v[if cache { "used" } else { "utilization" }].as_f64()?;
        if !used.is_finite() || !(0.0..=100.0).contains(&used) {
            return None;
        }
        let resets = if cache {
            v["resets"].as_u64()
        } else {
            v["resets_at"].as_str().and_then(parse_time)
        };
        Some(Self { used, resets })
    }
    pub fn reset_label(&self, now: u64) -> String {
        match self.resets {
            None => "Reset time unavailable".into(),
            Some(t) if t <= now => "Reset due · refresh".into(),
            Some(t) => {
                let minutes = (t - now).div_ceil(60);
                if minutes >= 1440 {
                    format!("Resets in {}d {}h", minutes / 1440, minutes % 1440 / 60)
                } else {
                    format!("Resets in {}h {}m", minutes / 60, minutes % 60)
                }
            }
        }
    }
    fn cache(&self) -> Value {
        json!({"used":self.used,"resets":self.resets})
    }
}
impl Usage {
    pub fn parse(v: &Value, plan: String) -> io::Result<Self> {
        let session = Window::parse(&v["five_hour"], false);
        let weekly = Window::parse(&v["seven_day"], false)
            .or_else(|| Window::parse(&v["seven_day_oauth_apps"], false));
        if session.is_none() && weekly.is_none() {
            return Err(io::Error::other(
                "This account did not report quota limits.",
            ));
        }
        Ok(Self {
            session,
            weekly,
            updated: accounts::now(),
            plan,
        })
    }
    pub fn from_cache(v: &Value) -> Option<Self> {
        let session = Window::parse(&v["session"], true);
        let weekly = Window::parse(&v["weekly"], true);
        if session.is_none() && weekly.is_none() {
            return None;
        }
        Some(Self {
            session,
            weekly,
            updated: v["updated"].as_u64()?,
            plan: v["plan"].as_str().unwrap_or("").chars().take(32).collect(),
        })
    }
    pub fn to_cache(&self) -> Value {
        json!({"session":self.session.as_ref().map(Window::cache),"weekly":self.weekly.as_ref().map(Window::cache),"updated":self.updated,"plan":self.plan})
    }
}
fn parse_time(s: &str) -> Option<u64> {
    let year = s.get(0..4)?.parse().ok()?;
    let month = s.get(5..7)?.parse().ok()?;
    let day = s.get(8..10)?.parse().ok()?;
    if s.get(4..5)? != "-"
        || s.get(7..8)? != "-"
        || s.get(10..11)? != "T"
        || s.get(13..14)? != ":"
        || s.get(16..17)? != ":"
    {
        return None;
    }
    let time = SYSTEMTIME {
        wYear: year,
        wMonth: month,
        wDayOfWeek: 0,
        wDay: day,
        wHour: s.get(11..13)?.parse().ok()?,
        wMinute: s.get(14..16)?.parse().ok()?,
        wSecond: s.get(17..19)?.parse().ok()?,
        wMilliseconds: 0,
    };
    let mut ft = unsafe { zeroed() };
    if unsafe { SystemTimeToFileTime(&time, &mut ft) } == 0 {
        return None;
    }
    let suffix = s.get(19..)?;
    let zone = if suffix.starts_with('.') {
        let n = suffix[1..].find(|c: char| !c.is_ascii_digit())? + 1;
        &suffix[n..]
    } else {
        suffix
    };
    let offset = if zone == "Z" {
        0i64
    } else {
        if zone.len() != 6 || &zone[3..4] != ":" {
            return None;
        }
        let hours: i64 = zone.get(1..3)?.parse().ok()?;
        let minutes: i64 = zone.get(4..6)?.parse().ok()?;
        if hours > 23 || minutes > 59 {
            return None;
        }
        let sign = match &zone[0..1] {
            "+" => 1,
            "-" => -1,
            _ => return None,
        };
        sign * (hours * 3600 + minutes * 60)
    };
    let ticks = ((ft.dwHighDateTime as u64) << 32) | ft.dwLowDateTime as u64;
    let seconds = (ticks / 10_000_000).checked_sub(11_644_473_600)? as i64 - offset;
    u64::try_from(seconds).ok()
}
fn decode64(s: &str) -> io::Result<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let mut bits = 0u32;
    let mut count = 0;
    let mut padding = false;
    for b in s.bytes() {
        if b == b'=' {
            padding = true;
            continue;
        }
        if padding {
            return Err(io::Error::other("Invalid credential encoding"));
        }
        let n = match b {
            b'A'..=b'Z' => b - b'A',
            b'a'..=b'z' => b - b'a' + 26,
            b'0'..=b'9' => b - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return Err(io::Error::other("Invalid credential encoding")),
        };
        bits = (bits << 6) | n as u32;
        count += 6;
        if count >= 8 {
            count -= 8;
            out.push((bits >> count) as u8);
            bits &= (1 << count) - 1;
        }
    }
    if count == 6 || bits != 0 {
        return Err(io::Error::other("Invalid credential encoding"));
    }
    Ok(out)
}
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}
fn decrypt_cache(encrypted: &str, local: &Value) -> io::Result<Value> {
    let encrypted = decode64(encrypted)?;
    if encrypted.len() < 31 || !encrypted.starts_with(b"v10") {
        return Err(io::Error::other("Unsupported Claude credential encryption"));
    }
    let wrapped = decode64(
        local["os_crypt"]["encrypted_key"]
            .as_str()
            .ok_or_else(|| io::Error::other("Claude encryption key unavailable"))?,
    )?;
    if !wrapped.starts_with(b"DPAPI") {
        return Err(io::Error::other("Unsupported Claude key protection"));
    }
    let mut secret = profiles::dpapi(&wrapped[5..], false)?;
    let result = decrypt_gcm(&secret, &encrypted).and_then(|mut plain| {
        let result = serde_json::from_slice(&plain)
            .map_err(|_| io::Error::other("Unsupported Claude session format"));
        plain.fill(0);
        result
    });
    secret.fill(0);
    result
}
fn decrypt_gcm(secret: &[u8], encrypted: &[u8]) -> io::Result<Vec<u8>> {
    if encrypted.len() < 31 {
        return Err(io::Error::other("Invalid encrypted session"));
    }
    unsafe {
        let mut algorithm = null_mut();
        if BCryptOpenAlgorithmProvider(&mut algorithm, BCRYPT_AES_ALGORITHM, null(), 0) < 0 {
            return Err(io::Error::other(
                "Could not initialize Windows cryptography",
            ));
        }
        let mode = wide("ChainingModeGCM");
        let status = BCryptSetProperty(
            algorithm,
            BCRYPT_CHAINING_MODE,
            mode.as_ptr().cast(),
            (mode.len() * 2) as u32,
            0,
        );
        let mut key = null_mut();
        let generated = if status >= 0 {
            BCryptGenerateSymmetricKey(
                algorithm,
                &mut key,
                null_mut(),
                0,
                secret.as_ptr(),
                secret.len() as u32,
                0,
            )
        } else {
            status
        };
        let mut plain = vec![0; encrypted.len() - 31];
        let mut written = 0;
        let mut info: BCRYPT_AUTHENTICATED_CIPHER_MODE_INFO = zeroed();
        info.cbSize = size_of::<BCRYPT_AUTHENTICATED_CIPHER_MODE_INFO>() as u32;
        info.dwInfoVersion = 1;
        info.pbNonce = encrypted[3..15].as_ptr() as *mut u8;
        info.cbNonce = 12;
        info.pbTag = encrypted[encrypted.len() - 16..].as_ptr() as *mut u8;
        info.cbTag = 16;
        let decrypted = if generated >= 0 {
            BCryptDecrypt(
                key,
                encrypted[15..encrypted.len() - 16].as_ptr(),
                plain.len() as u32,
                (&info as *const BCRYPT_AUTHENTICATED_CIPHER_MODE_INFO).cast(),
                null_mut(),
                0,
                plain.as_mut_ptr(),
                plain.len() as u32,
                &mut written,
                0,
            )
        } else {
            generated
        };
        if !key.is_null() {
            BCryptDestroyKey(key);
        }
        BCryptCloseAlgorithmProvider(algorithm, 0);

        if decrypted < 0 {
            plain.fill(0);
            Err(io::Error::other("Could not decrypt this Claude session"))
        } else {
            plain.truncate(written as usize);
            Ok(plain)
        }
    }
}

struct Credential {
    token: String,
    plan: String,
}
fn select_token(cache: &Value, uuid: &str) -> Option<Credential> {
    if uuid.is_empty() {
        return None;
    }
    let prefix = format!("acct:{uuid}|");
    cache
        .as_object()?
        .iter()
        .filter(|(k, v)| {
            k.starts_with(&prefix)
                && k.contains(":https://api.anthropic.com:")
                && k.split(":https://api.anthropic.com:")
                    .nth(1)
                    .unwrap_or("")
                    .split_whitespace()
                    .any(|s| s == "user:profile")
                && v["token"].is_string()
        })
        .min_by_key(|(k, _)| k.len())
        .and_then(|(_, v)| {
            let token = v["token"].as_str()?;
            if token.is_empty() || token.len() > 16384 || token.contains(['\r', '\n']) {
                return None;
            }
            Some(Credential {
                token: token.into(),
                plan: v["subscriptionType"]
                    .as_str()
                    .unwrap_or("")
                    .chars()
                    .take(32)
                    .collect(),
            })
        })
}
fn credential(index: usize) -> io::Result<Credential> {
    let dir = profiles::account_dir(index)?;
    let snapshot: Value =
        serde_json::from_slice(&profiles::dpapi(&fs::read(dir.join("auth.json"))?, false)?)
            .map_err(|_| io::Error::other("Could not read saved account"))?;
    let uuid = snapshot["lastKnownAccountUuid"]
        .as_str()
        .ok_or_else(|| io::Error::other("Save this account again to identify it"))?;
    let mut auth = snapshot.clone();
    let mut local = None;
    // Prefer fresh Desktop credentials only after verifying the slot's account ID.
    if let Ok(live) = profiles::find_claude() {
        if let Ok(bytes) = fs::read(live.join("config.json")) {
            if let Ok(config) = serde_json::from_slice::<Value>(&bytes) {
                if config["lastKnownAccountUuid"].as_str() == Some(uuid) {
                    if let Ok(bytes) = fs::read(live.join("Local State")) {
                        if let Ok(state) = serde_json::from_slice::<Value>(&bytes) {
                            auth = config;
                            local = Some(state);
                        }
                    }
                }
            }
        }
    }
    let local = match local {
        Some(state) => state,
        None => serde_json::from_slice(&profiles::dpapi(
            &fs::read(dir.join("Local State"))?,
            false,
        )?)
        .map_err(|_| io::Error::other("Saved encryption key unavailable · save account again"))?,
    };
    for field in ["oauth:tokenCacheV2", "oauth:tokenCache"] {
        if let Some(encrypted) = auth[field].as_str() {
            if let Ok(cache) = decrypt_cache(encrypted, &local) {
                if let Some(c) = select_token(&cache, uuid) {
                    return Ok(c);
                }
            }
        }
    }
    Err(io::Error::other(
        "No usage credential for this account · open Claude and save it again",
    ))
}
struct Http(*mut c_void);
impl Drop for Http {
    fn drop(&mut self) {
        unsafe {
            if !self.0.is_null() {
                WinHttpCloseHandle(self.0);
            }
        }
    }
}
fn handle(raw: *mut c_void) -> io::Result<Http> {
    if raw.is_null() {
        Err(io::Error::other("Could not connect to Anthropic"))
    } else {
        Ok(Http(raw))
    }
}
fn request(token: &str) -> io::Result<Value> {
    unsafe {
        let session = handle(WinHttpOpen(
            wide("Muisland/0.4").as_ptr(),
            WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
            null(),
            null(),
            0,
        ))?;
        WinHttpSetTimeouts(session.0, 5000, 5000, 8000, 8000);
        let connection = handle(WinHttpConnect(
            session.0,
            wide("api.anthropic.com").as_ptr(),
            443,
            0,
        ))?;
        let req = handle(WinHttpOpenRequest(
            connection.0,
            wide("GET").as_ptr(),
            wide("/api/oauth/usage").as_ptr(),
            null(),
            null(),
            null(),
            WINHTTP_FLAG_SECURE,
        ))?;
        // Never forward a credential to a redirected origin; Windows validates TLS normally.
        let policy = WINHTTP_OPTION_REDIRECT_POLICY_NEVER;
        if WinHttpSetOption(
            req.0,
            WINHTTP_OPTION_REDIRECT_POLICY,
            (&policy as *const u32).cast(),
            4,
        ) == 0
        {
            return Err(io::Error::other("Could not restrict request redirects"));
        }
        let headers=wide(&format!("Authorization: Bearer {token}\r\nanthropic-beta: oauth-2025-04-20\r\nAccept: application/json\r\n"));
        if WinHttpSendRequest(
            req.0,
            headers.as_ptr(),
            (headers.len() - 1) as u32,
            null(),
            0,
            0,
            0,
        ) == 0
            || WinHttpReceiveResponse(req.0, null_mut()) == 0
        {
            return Err(io::Error::other(
                "Anthropic usage request failed · check your connection",
            ));
        }
        let mut status = 0u32;
        let mut size = 4;
        if WinHttpQueryHeaders(
            req.0,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            null(),
            (&mut status as *mut u32).cast(),
            &mut size,
            null_mut(),
        ) == 0
        {
            return Err(io::Error::other("Missing usage response status"));
        }
        match status {
            200 => {}
            401 => {
                return Err(io::Error::other(
                    "Session expired · open Claude and save account again",
                ))
            }
            403 => {
                return Err(io::Error::other(
                    "Usage access unavailable for this session",
                ))
            }
            429 => return Err(io::Error::other("Rate limited · wait before refreshing")),
            _ => {
                return Err(io::Error::other(format!(
                    "Usage request returned HTTP {status}"
                )))
            }
        }
        let mut body = Vec::new();
        let mut buffer = [0u8; 8192];
        loop {
            let mut count = 0;
            if WinHttpReadData(
                req.0,
                buffer.as_mut_ptr().cast(),
                buffer.len() as u32,
                &mut count,
            ) == 0
            {
                return Err(io::Error::other("Usage response interrupted"));
            }
            if count == 0 {
                break;
            }
            body.extend_from_slice(&buffer[..count as usize]);
            if body.len() > 1_048_576 {
                return Err(io::Error::other("Usage response too large"));
            }
        }
        serde_json::from_slice(&body).map_err(|_| io::Error::other("Unexpected usage response"))
    }
}
pub fn fetch(index: usize) -> io::Result<Usage> {
    let mut c = credential(index)?;
    let result = request(&c.token).and_then(|v| Usage::parse(&v, c.plan.clone()));
    c.token.clear();
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn quota_missing_is_not_zero_and_bad_fields_do_not_hide_weekly() {
        let u=Usage::parse(&json!({"five_hour":null,"seven_day":{"utilization":42,"resets_at":"2026-11-08T12:00:00Z"}}),"pro".into()).unwrap();
        assert!(u.session.is_none());
        assert_eq!(u.weekly.as_ref().unwrap().used, 42.);
        assert!(Usage::parse(
            &json!({"five_hour":{"utilization":101},"seven_day":{"utilization":true}}),
            String::new()
        )
        .is_err());
        assert_eq!(
            Usage::from_cache(&u.to_cache())
                .unwrap()
                .weekly
                .unwrap()
                .used,
            42.
        );
    }
    #[test]
    fn reset_times_support_offsets_and_elapsed_values_stay_unknown() {
        assert_eq!(
            parse_time("2026-11-08T12:00:00Z"),
            parse_time("2026-11-08T14:00:00.123+02:00")
        );
        assert!(parse_time("not-a-date").is_none());
        assert!(parse_time("2026-99-99T12:00:00Z").is_none());
        assert_eq!(
            Window {
                used: 80.,
                resets: Some(100)
            }
            .reset_label(200),
            "Reset due · refresh"
        );
    }
    #[test]
    fn credential_selection_cannot_cross_accounts_or_use_wrong_scopes() {
        let c = json!({"acct:other|x:https://api.anthropic.com:user:profile":{"token":"wrong"},"acct:mine|x:https://evil.test:user:profile":{"token":"wrong"},"acct:mine|x:https://api.anthropic.com:user:inference":{"token":"wrong"},"acct:mine|x:https://api.anthropic.com:user:profile":{"token":"right"}});
        assert_eq!(select_token(&c, "mine").unwrap().token, "right");
        assert!(select_token(&c, "missing").is_none());
    }

    #[test]
    fn windows_gcm_matches_known_aes256_vector_and_rejects_tampering() {
        let key = [0u8; 32];
        let mut blob = b"v10".to_vec();
        blob.extend_from_slice(&[0u8; 12]);
        blob.extend_from_slice(&[
            0xce, 0xa7, 0x40, 0x3d, 0x4d, 0x60, 0x6b, 0x6e, 0x07, 0x4e, 0xc5, 0xd3, 0xba, 0xf3,
            0x9d, 0x18,
        ]);
        blob.extend_from_slice(&[
            0xd0, 0xd1, 0xc8, 0xa7, 0x99, 0x99, 0x6b, 0xf0, 0x26, 0x5b, 0x98, 0xb5, 0xd4, 0x8a,
            0xb9, 0x19,
        ]);
        assert_eq!(decrypt_gcm(&key, &blob).unwrap(), [0u8; 16]);
        *blob.last_mut().unwrap() ^= 1;
        assert!(decrypt_gcm(&key, &blob).is_err());
    }
    #[test]
    fn base64_decoding_validates_input() {
        assert_eq!(decode64("aGVsbG8=").unwrap(), b"hello");
        assert!(decode64("a===x").is_err());
        assert!(decode64("?").is_err());
    }
}
