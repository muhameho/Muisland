use crate::{profiles, usage::Usage};
use serde_json::{json, Value};
use std::{
    fs, io,
    time::{SystemTime, UNIX_EPOCH},
};

pub const MAX_ACCOUNTS: usize = 64;
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
    pub accounts: Vec<Account>,
    pub protect_local: bool,
    pub scale: f32,
    pub circle: bool,
    pub reduced_motion: bool,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            accounts: (0..2)
                .map(|i| Account {
                    name: format!("Account {}", i + 1),
                    usage: None,
                })
                .collect(),
            protect_local: false,
            scale: 1.0,
            circle: false,
            reduced_motion: false,
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
        let count = v["accounts"]
            .as_array()
            .map(|a| a.len())
            .unwrap_or(2)
            .clamp(2, MAX_ACCOUNTS);
        while result.accounts.len() < count {
            result.add_account();
        }
        for i in 0..count {
            if let Some(name) = v["accounts"][i]["name"].as_str() {
                if let Ok(name) = valid_name(name) {
                    result.accounts[i].name = name;
                }
            }
            result.accounts[i].usage = Usage::from_cache(&v["accounts"][i]["usage"]);
        }
        result.protect_local = v["protect_local"].as_bool().unwrap_or(false);
        result.scale = v["scale"]
            .as_f64()
            .filter(|n| n.is_finite())
            .unwrap_or(1.0)
            .clamp(0.75, 1.6) as f32;
        result.circle = v["circle"].as_bool().unwrap_or(false);
        result.reduced_motion = v["reduced_motion"].as_bool().unwrap_or(false);
        result
    }
    pub fn add_account(&mut self) -> Option<usize> {
        if self.accounts.len() >= MAX_ACCOUNTS {
            return None;
        }
        let i = self.accounts.len();
        self.accounts.push(Account {
            name: format!("Account {}", i + 1),
            usage: None,
        });
        Some(i)
    }
    pub fn save(&self) -> io::Result<()> {
        let accounts: Vec<Value> = self
            .accounts
            .iter()
            .map(|a| json!({"name": a.name, "usage": a.usage.as_ref().map(Usage::to_cache)}))
            .collect();
        let data = serde_json::to_vec(&json!({"version":1,"accounts":accounts,"protect_local":self.protect_local,"scale":self.scale,"circle":self.circle,"reduced_motion":self.reduced_motion})).map_err(io::Error::other)?;
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
        assert_eq!(settings.scale, 1.0);
        assert!(!settings.circle);
        assert!(!settings.protect_local);
        assert!(settings.accounts[0].usage.is_none());
    }
    #[test]
    fn dynamic_accounts_and_preferences_roundtrip() {
        let v = json!({"accounts":[{"name":"One"},{"name":"Two"},{"name":"Three"},{"name":"Four"}],"scale":9.0,"circle":true});
        let mut settings = Settings::from_value(&v);
        assert_eq!(settings.accounts.len(), 4);
        assert_eq!(settings.accounts[3].name, "Four");
        assert_eq!(settings.scale, 1.6);
        assert!(settings.circle);
        assert_eq!(settings.add_account(), Some(4));
        assert_eq!(settings.accounts[4].name, "Account 5");
    }
    #[test]
    fn names_support_unicode_and_reject_controls() {
        assert_eq!(valid_name("  شخصي 🚀 ").unwrap(), "شخصي 🚀");
        assert!(valid_name("a\nb").is_err());
        assert!(valid_name(&"a".repeat(33)).is_err());
    }
}
