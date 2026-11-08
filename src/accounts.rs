use crate::{profiles, usage::Usage};
use serde_json::{json, Value};
use std::{
    fs, io,
    time::{SystemTime, UNIX_EPOCH},
};

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
#[derive(Clone)]
pub struct Account {
    pub name: String,
    pub usage: Option<Usage>,
}
#[derive(Clone)]
pub struct Settings {
    pub accounts: [Account; 2],
    pub protect_local: bool,
    pub live_usage: bool,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            accounts: std::array::from_fn(|i| Account {
                name: format!("Account {}", i + 1),
                usage: None,
            }),
            protect_local: false,
            live_usage: false,
        }
    }
}
impl Settings {
    pub fn load() -> io::Result<Self> {
        let p = profiles::base()?.join("muisland-settings.bin");
        if !p.exists() {
            return Ok(Self::default());
        }
        let value: Value = serde_json::from_slice(&profiles::dpapi(&fs::read(p)?, false)?)
            .map_err(io::Error::other)?;
        Ok(Self::from_value(&value))
    }
    fn from_value(v: &Value) -> Self {
        let mut result = Self::default();
        for i in 0..2 {
            if let Some(name) = v["accounts"][i]["name"].as_str() {
                if let Ok(name) = valid_name(name) {
                    result.accounts[i].name = name;
                }
            }
            result.accounts[i].usage = Usage::from_cache(&v["accounts"][i]["usage"]);
        }
        result.protect_local = v["protect_local"].as_bool().unwrap_or(false);
        result.live_usage = v["live_usage"].as_bool().unwrap_or(false);
        result
    }
    pub fn save(&self) -> io::Result<()> {
        let accounts: Vec<Value> = self
            .accounts
            .iter()
            .map(|a| json!({"name": a.name, "usage": a.usage.as_ref().map(Usage::to_cache)}))
            .collect();
        let data = serde_json::to_vec(&json!({"version":1,"accounts":accounts,"protect_local":self.protect_local,"live_usage":self.live_usage})).map_err(io::Error::other)?;
        profiles::write_atomic(
            &profiles::base()?.join("muisland-settings.bin"),
            &profiles::dpapi(&data, true)?,
        )
    }
}
pub fn valid_name(input: &str) -> io::Result<String> {
    let input = input.trim();
    if input.is_empty() || input.chars().count() > 32 || input.chars().any(char::is_control) {
        return Err(io::Error::other("Use a name of 1–32 characters."));
    }
    Ok(input.into())
}
pub fn age(seconds: u64) -> String {
    if seconds < 60 {
        "just now".into()
    } else if seconds < 3600 {
        format!("{}m ago", seconds / 60)
    } else if seconds < 86400 {
        format!("{}h ago", seconds / 3600)
    } else {
        format!("{}d ago", seconds / 86400)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn old_and_damaged_settings_have_safe_defaults() {
        let settings = Settings::from_value(
            &json!({"accounts":[{"name":"  شغل  ","usage":{"session":{"used":500}}},{"name":"\n"}],"live_usage":"yes"}),
        );
        assert_eq!(settings.accounts[0].name, "شغل");
        assert_eq!(settings.accounts[1].name, "Account 2");
        assert!(!settings.live_usage);
        assert!(!settings.protect_local);
        assert!(settings.accounts[0].usage.is_none());
    }
    #[test]
    fn names_support_unicode_and_reject_controls() {
        assert_eq!(valid_name("  شخصي 🚀 ").unwrap(), "شخصي 🚀");
        assert!(valid_name("a\nb").is_err());
        assert!(valid_name(&"a".repeat(33)).is_err());
    }
}
