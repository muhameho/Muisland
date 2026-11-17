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
    pub slot: usize,
    pub draft: bool,
    pub name: String,
    pub usage: Option<Usage>,
}
#[derive(Clone)]
pub struct Settings {
    pub accounts: Vec<Account>,
    pub other_accounts: Vec<Account>,
    pub provider: usize,
    pub theme: usize,
    pub protect_local: bool,
    /// Copy Claude's Code-tab session list between accounts when switching.
    pub share_sessions: bool,
    /// Folder of the Cowork task being handed off between accounts.
    pub handoff_folder: Option<String>,
    pub menu_scale: f32,
    pub float_scale: f32,
    pub bubble_scale: f32,
    pub circle: bool,
    pub reduced_motion: bool,
    pub circle_numbers: bool,
    pub usage_alerts: bool,
    pub session_threshold: f32,
    pub weekly_threshold: f32,
    pub auto_switch: bool,
    pub pause_for_work: bool,
    /// 0 English, 1 Arabic.
    pub language: usize,
    /// True when no language was ever saved; the system language is used.
    pub language_unset: bool,
    /// Island percentage: 0 session, 1 weekly, 2 highest.
    pub circle_metric: usize,
    pub tray: bool,
    pub hotkey: bool,
    pub show_island: bool,
    /// Saved floating position (center x, top) in screen pixels.
    pub position: Option<(f32, i32)>,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            accounts: (0..2)
                .map(|i| Account {
                    slot: i + 1,
                    draft: false,
                    name: format!("Account {}", i + 1),
                    usage: None,
                })
                .collect(),
            other_accounts: Vec::new(),
            provider: 0,
            theme: crate::theme::DEFAULT,
            protect_local: false,
            share_sessions: true,
            handoff_folder: None,
            menu_scale: 1.0,
            float_scale: 1.0,
            bubble_scale: 1.0,
            circle: true,
            reduced_motion: false,
            circle_numbers: true,
            usage_alerts: false,
            session_threshold: 0.8,
            weekly_threshold: 0.8,
            auto_switch: false,
            pause_for_work: true,
            language: 0,
            language_unset: true,
            circle_metric: 0,
            tray: true,
            hotkey: true,
            show_island: true,
            position: None,
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
        if let Some(rows) = v["accounts"].as_array() {
            result.accounts.clear();
            for row in rows.iter().take(MAX_ACCOUNTS) {
                let preferred = row["slot"].as_u64().map(|n| n as usize).filter(|n| {
                    (1..=MAX_ACCOUNTS).contains(n) && !result.accounts.iter().any(|a| a.slot == *n)
                });
                let slot = preferred.unwrap_or_else(|| {
                    (1..=MAX_ACCOUNTS)
                        .find(|n| !result.accounts.iter().any(|a| a.slot == *n))
                        .unwrap()
                });
                let name = row["name"]
                    .as_str()
                    .and_then(|n| valid_name(n).ok())
                    .unwrap_or_else(|| format!("Account {}", result.accounts.len() + 1));
                result.accounts.push(Account {
                    slot,
                    draft: false,
                    name,
                    usage: Usage::from_cache(&row["usage"]),
                });
            }
        }
        if let Some(rows) = v["codex_accounts"].as_array() {
            let codex = Self::from_value(&json!({"accounts":rows}));
            result.other_accounts = codex.accounts;
        }
        result.provider = v["provider"].as_u64().unwrap_or(0).min(1) as usize;
        result.theme = v["theme"]
            .as_u64()
            .map(|t| t as usize)
            .unwrap_or(crate::theme::DEFAULT)
            .min(crate::theme::COUNT - 1);
        if result.provider == 1 {
            std::mem::swap(&mut result.accounts, &mut result.other_accounts);
        }
        result.protect_local = v["protect_local"].as_bool().unwrap_or(false);
        result.share_sessions = v["share_sessions"].as_bool().unwrap_or(true);
        result.handoff_folder = v["handoff_folder"]
            .as_str()
            .filter(|f| !f.is_empty() && f.len() < 1024)
            .map(String::from);
        let legacy = v["scale"].as_f64().filter(|n| n.is_finite()).unwrap_or(1.0);
        result.menu_scale = v["menu_scale"]
            .as_f64()
            .filter(|n| n.is_finite())
            .unwrap_or(legacy)
            .clamp(0.75, 1.6) as f32;
        result.float_scale = v["float_scale"]
            .as_f64()
            .filter(|n| n.is_finite())
            .unwrap_or(legacy)
            .clamp(0.60, 1.8) as f32;
        result.bubble_scale = v["bubble_scale"]
            .as_f64()
            .filter(|n| n.is_finite())
            .unwrap_or(1.0)
            .clamp(0.85, 1.6) as f32;
        result.circle = v["circle"].as_bool().unwrap_or(true);
        result.circle_numbers = v["circle_numbers"].as_bool().unwrap_or(true);
        // Version 3 introduced the percentage circle as the island; apply it once.
        if v["version"].as_u64().unwrap_or(0) < 3 {
            result.circle = true;
            result.circle_numbers = true;
        }
        if let Some(language) = v["language"].as_u64() {
            result.language = (language as usize).min(1);
            result.language_unset = false;
        }
        result.circle_metric = v["circle_metric"].as_u64().unwrap_or(0).min(2) as usize;
        result.tray = v["tray"].as_bool().unwrap_or(true);
        result.hotkey = v["hotkey"].as_bool().unwrap_or(true);
        result.show_island = v["show_island"].as_bool().unwrap_or(true) || !result.tray;
        // Version 4: an early minimize-to-tray build saved the island as hidden;
        // minimizing is now temporary, so bring the island back once.
        if v["version"].as_u64().unwrap_or(0) < 4 {
            result.show_island = true;
        }
        result.position = match (v["position"][0].as_f64(), v["position"][1].as_i64()) {
            (Some(x), Some(y)) if x.is_finite() && x.abs() < 1e6 && y.abs() < 1_000_000 => {
                Some((x as f32, y as i32))
            }
            _ => None,
        };
        result.usage_alerts = v["usage_alerts"].as_bool().unwrap_or(false);
        result.session_threshold = v["session_threshold"]
            .as_f64()
            .filter(|n| n.is_finite())
            .unwrap_or(0.8)
            .clamp(0.05, 1.0) as f32;
        result.weekly_threshold = v["weekly_threshold"]
            .as_f64()
            .filter(|n| n.is_finite())
            .unwrap_or(0.8)
            .clamp(0.05, 1.0) as f32;
        result.auto_switch = v["auto_switch"].as_bool().unwrap_or(false);
        result.pause_for_work = v["pause_for_work"].as_bool().unwrap_or(true);
        result.reduced_motion = v["reduced_motion"].as_bool().unwrap_or(false);
        result
    }
    pub fn add_account(&mut self) -> Option<usize> {
        if self.accounts.len() >= MAX_ACCOUNTS {
            return None;
        }
        let i = self.accounts.len();
        let slot = (1..=MAX_ACCOUNTS).find(|n| !self.accounts.iter().any(|a| a.slot == *n))?;
        self.accounts.push(Account {
            slot,
            draft: true,
            name: format!("Account {}", i + 1),
            usage: None,
        });
        Some(i)
    }
    fn to_value(&self) -> Value {
        let rows = |list: &[Account]| -> Vec<Value> {
            list.iter().filter(|a|!a.draft).map(|a|json!({"slot":a.slot,"name":a.name,"usage":a.usage.as_ref().map(Usage::to_cache)})).collect()
        };
        let (claude, codex) = if self.provider == 0 {
            (&self.accounts, &self.other_accounts)
        } else {
            (&self.other_accounts, &self.accounts)
        };
        json!({"version":4,"accounts":rows(claude),"codex_accounts":rows(codex),"provider":self.provider,"theme":self.theme,"protect_local":self.protect_local,"share_sessions":self.share_sessions,"handoff_folder":self.handoff_folder,"menu_scale":self.menu_scale,"float_scale":self.float_scale,"bubble_scale":self.bubble_scale,"circle":self.circle,"reduced_motion":self.reduced_motion,"circle_numbers":self.circle_numbers,"usage_alerts":self.usage_alerts,"session_threshold":self.session_threshold,"weekly_threshold":self.weekly_threshold,"auto_switch":self.auto_switch,"pause_for_work":self.pause_for_work,"language":self.language,"circle_metric":self.circle_metric,"tray":self.tray,"hotkey":self.hotkey,"show_island":self.show_island,"position":self.position.map(|(x,y)|json!([x,y]))})
    }
    pub fn select_provider(&mut self, provider: usize) {
        let provider = provider.min(1);
        if self.provider != provider {
            std::mem::swap(&mut self.accounts, &mut self.other_accounts);
            self.provider = provider;
        }
    }
    pub fn save(&self) -> io::Result<()> {
        let data = serde_json::to_vec(&self.to_value()).map_err(io::Error::other)?;
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
        assert_eq!(settings.menu_scale, 1.0);
        assert!(settings.circle, "the circle is the default island");
        assert!(
            settings.circle_numbers,
            "the circle shows a percentage by default"
        );
        assert!(!settings.protect_local);
        assert!(settings.accounts[0].usage.is_none());
    }
    #[test]
    fn dynamic_accounts_and_preferences_roundtrip() {
        let v = json!({"accounts":[{"name":"One"},{"name":"Two"},{"name":"Three"},{"name":"Four"}],"scale":9.0,"circle":true});
        let mut settings = Settings::from_value(&v);
        assert_eq!(settings.accounts.len(), 4);
        assert_eq!(settings.accounts[3].name, "Four");
        assert_eq!(settings.menu_scale, 1.6);
        assert_eq!(settings.float_scale, 1.8);
        assert_eq!(settings.bubble_scale, 1.0);
        assert!(settings.circle);
        assert_eq!(settings.add_account(), Some(4));
        assert_eq!(settings.accounts[4].name, "Account 5");
    }
    #[test]
    fn bubble_size_is_independent_and_old_preferences_default_to_readable_size() {
        let old = Settings::from_value(&json!({"float_scale":0.6,"scale":0.6}));
        assert_eq!(old.bubble_scale, 1.0);
        let custom = Settings::from_value(&json!({"float_scale":0.6,"bubble_scale":1.3}));
        assert_eq!(custom.float_scale, 0.6);
        assert_eq!(custom.bubble_scale, 1.3);
        assert_eq!(
            Settings::from_value(&json!({"bubble_scale":0.1})).bubble_scale,
            0.85
        );
        assert_eq!(
            Settings::from_value(&json!({"bubble_scale":20})).bubble_scale,
            1.6
        );
    }
    #[test]
    fn removal_keeps_other_storage_slots_and_empty_lists_stay_empty() {
        let mut settings = Settings::from_value(
            &json!({"accounts":[{"slot":1,"name":"A"},{"slot":3,"name":"C"}],"menu_scale":1.4,"float_scale":0.7}),
        );
        settings.accounts.remove(0);
        assert_eq!(settings.accounts[0].slot, 3);
        settings.add_account();
        assert_eq!(settings.accounts[0].slot, 3);
        assert_eq!(settings.accounts[1].slot, 1);
        assert_eq!(settings.menu_scale, 1.4);
        assert_eq!(settings.float_scale, 0.7);
        assert!(Settings::from_value(&json!({"accounts":[]}))
            .accounts
            .is_empty());
    }
    #[test]
    fn draft_accounts_never_enter_saved_settings_until_committed() {
        let mut settings = Settings::default();
        let i = settings.add_account().unwrap();
        settings.accounts[i].name = "A draft name".into();
        settings.menu_scale = 1.3;
        let decoded = Settings::from_value(&settings.to_value());
        assert_eq!(decoded.accounts.len(), 2);
        assert_eq!(decoded.menu_scale, 1.3);
        settings.accounts[i].draft = false;
        let decoded = Settings::from_value(&settings.to_value());
        assert_eq!(decoded.accounts.len(), 3);
        assert_eq!(decoded.accounts[2].name, "A draft name");
    }
    #[test]
    fn providers_have_separate_slots_and_survive_roundtrip() {
        let mut settings = Settings::default();
        settings.accounts[0].name = "Claude personal".into();
        settings.select_provider(1);
        assert!(settings.accounts.is_empty());
        let i = settings.add_account().unwrap();
        settings.accounts[i].draft = false;
        settings.accounts[i].name = "Codex work".into();
        settings.theme = 2;
        let mut loaded = Settings::from_value(&settings.to_value());
        assert_eq!(loaded.provider, 1);
        assert_eq!(loaded.accounts[0].name, "Codex work");
        assert_eq!(loaded.theme, 2);
        loaded.select_provider(0);
        assert_eq!(loaded.accounts[0].name, "Claude personal");
    }
    #[test]
    fn new_preferences_roundtrip_and_old_files_keep_working() {
        let mut settings = Settings::default();
        settings.language = 1;
        settings.language_unset = false;
        settings.circle_metric = 2;
        settings.tray = false;
        settings.show_island = false;
        settings.theme = 9;
        settings.position = Some((812.5, 40));
        let loaded = Settings::from_value(&settings.to_value());
        assert_eq!(loaded.language, 1);
        assert!(!loaded.language_unset);
        assert_eq!(loaded.circle_metric, 2);
        assert!(!loaded.tray);
        assert!(
            loaded.show_island,
            "the island cannot be hidden without a tray icon"
        );
        assert_eq!(loaded.theme, 9);
        assert_eq!(loaded.position, Some((812.5, 40)));
        let old = Settings::from_value(
            &json!({"version":2,"theme":3,"circle":false,"circle_numbers":false}),
        );
        assert_eq!(old.theme, 3);
        assert!(
            old.circle && old.circle_numbers,
            "upgrades adopt the percentage circle once"
        );
        let chosen =
            Settings::from_value(&json!({"version":3,"circle":false,"circle_numbers":false}));
        assert!(
            !chosen.circle && !chosen.circle_numbers,
            "later choices are respected"
        );
        assert!(old.language_unset);
        let minimized =
            Settings::from_value(&json!({"version":3,"tray":true,"show_island":false}));
        assert!(minimized.show_island, "the early tray build's hidden island returns once");
        let hidden = Settings::from_value(&json!({"version":4,"tray":true,"show_island":false}));
        assert!(!hidden.show_island, "a deliberate tray-only choice is kept");
        assert!(Settings::from_value(&json!({"theme":999})).theme < crate::theme::COUNT);
        assert!(Settings::from_value(&json!({"position":[1e30,5]}))
            .position
            .is_none());
    }
    #[test]
    fn names_support_unicode_and_reject_controls() {
        assert_eq!(valid_name("  شخصي 🚀 ").unwrap(), "شخصي 🚀");
        assert!(valid_name("a\nb").is_err());
        assert!(valid_name(&"a".repeat(33)).is_err());
    }
}
