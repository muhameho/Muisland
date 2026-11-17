//! Colour themes. Index order is persisted in settings and must stay stable.
use crate::gfx::{hex, Color};

#[derive(Clone, Copy)]
pub struct Theme {
    pub name: &'static str,
    pub arabic: &'static str,
    pub dark: bool,
    /// Window / panel background.
    pub bg: Color,
    /// Raised cards.
    pub surface: Color,
    /// Controls on cards (buttons, tracks).
    pub raised: Color,
    pub line: Color,
    pub text: Color,
    pub muted: Color,
    pub accent: Color,
    pub accent2: Color,
    /// Text drawn on top of the accent gradient.
    pub on_accent: Color,
    pub warn: Color,
    pub danger: Color,
    pub good: Color,
    pub shadow: Color,
}

const fn dark(
    name: &'static str,
    arabic: &'static str,
    bg: u32,
    surface: u32,
    raised: u32,
    line: u32,
    muted: u32,
    accent: u32,
    accent2: u32,
) -> Theme {
    Theme {
        name,
        arabic,
        dark: true,
        bg: hex(bg),
        surface: hex(surface),
        raised: hex(raised),
        line: hex(line),
        text: hex(0xF4F5FA),
        muted: hex(muted),
        accent: hex(accent),
        accent2: hex(accent2),
        on_accent: hex(0x0B0C12),
        warn: hex(0xFFB547),
        danger: hex(0xFF6170),
        good: hex(0x4ADE9B),
        shadow: hex(0x000000),
    }
}
const fn light(
    name: &'static str,
    arabic: &'static str,
    bg: u32,
    surface: u32,
    raised: u32,
    line: u32,
    text: u32,
    muted: u32,
    accent: u32,
    accent2: u32,
) -> Theme {
    Theme {
        name,
        arabic,
        dark: false,
        bg: hex(bg),
        surface: hex(surface),
        raised: hex(raised),
        line: hex(line),
        text: hex(text),
        muted: hex(muted),
        accent: hex(accent),
        accent2: hex(accent2),
        on_accent: hex(0xFFFFFF),
        warn: hex(0xD98200),
        danger: hex(0xE0344A),
        good: hex(0x15A36A),
        shadow: hex(0x1B1F3A),
    }
}

/// Persisted indices 0–3 keep the original theme names.
pub const THEMES: [Theme; 10] = [
    dark(
        "Lavender",
        "لافندر",
        0x121019,
        0x1B1824,
        0x262233,
        0x2F2A3E,
        0x9E97B3,
        0xC4B2FF,
        0xFF9ECF,
    ),
    dark(
        "Ocean", "محيط", 0x0A1219, 0x111D27, 0x182836, 0x213343, 0x8CA1B3, 0x38BDF8, 0x34E2C1,
    ),
    dark(
        "Sunset", "غروب", 0x150F10, 0x201718, 0x2C2021, 0x3A2A2B, 0xB39C97, 0xFF8A5B, 0xFF5E8A,
    ),
    dark(
        "Graphite",
        "جرافيت",
        0x111112,
        0x1A1A1C,
        0x252527,
        0x2F2F32,
        0x9C9CA3,
        0xF2F2F2,
        0xA5A5AE,
    ),
    dark(
        "Midnight",
        "منتصف الليل",
        0x0C0D14,
        0x151722,
        0x1E2130,
        0x282C3F,
        0x8F94AE,
        0x8B7CFF,
        0x4CC9FF,
    ),
    dark(
        "Forest", "غابة", 0x0B120E, 0x121C16, 0x1A281F, 0x22352A, 0x8FA898, 0x4ADE80, 0xBEF264,
    ),
    dark(
        "Sakura",
        "ساكورا",
        0x151013,
        0x20171D,
        0x2C2028,
        0x3A2A34,
        0xB39AA9,
        0xFF9EC4,
        0xFFC9A3,
    ),
    dark(
        "Aurora", "شفق", 0x090F12, 0x10191D, 0x172328, 0x1F2F35, 0x8AA3A8, 0x5EEAD4, 0xA78BFA,
    ),
    light(
        "Daylight", "نهار", 0xF4F5FA, 0xFFFFFF, 0xECEEF5, 0xDFE2EC, 0x15161E, 0x6A6F84, 0x5B5BF0,
        0x00A3FF,
    ),
    light(
        "Latte",
        "لاتيه",
        0xF6F1EA,
        0xFFFCF8,
        0xEFE7DC,
        0xE2D7C8,
        0x2A211A,
        0x7D6E60,
        0xC2703D,
        0xE0A458,
    ),
];
/// Pseudo-theme that follows Windows' app light/dark preference.
pub const AUTO: usize = THEMES.len();
pub const DEFAULT: usize = 4;
pub const COUNT: usize = THEMES.len() + 1;

pub fn resolve(index: usize, system_dark: bool) -> &'static Theme {
    if index >= THEMES.len() {
        &THEMES[if system_dark { DEFAULT } else { 8 }]
    } else {
        &THEMES[index]
    }
}

#[cfg(windows)]
pub fn system_prefers_dark() -> bool {
    use windows_sys::Win32::System::Registry::*;
    let key: Vec<u16> = "Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize\0"
        .encode_utf16()
        .collect();
    let value: Vec<u16> = "AppsUseLightTheme\0".encode_utf16().collect();
    let mut data = 1u32;
    let mut size = 4u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_DWORD,
            std::ptr::null_mut(),
            (&mut data as *mut u32).cast(),
            &mut size,
        )
    };
    status != 0 || data == 0
}

impl Theme {
    /// Usage colour: accent below 70%, warning toward 90%, danger above.
    pub fn usage(&self, used: f64) -> (Color, Color) {
        let u = used as f32;
        let warn = ((u - 62.) / 13.).clamp(0., 1.);
        let danger = ((u - 84.) / 8.).clamp(0., 1.);
        let a = self.accent.mix(self.warn, warn).mix(self.danger, danger);
        let b = self.accent2.mix(self.warn, warn).mix(self.danger, danger);
        (a, b)
    }
}

/// Deterministic avatar hue for an account name.
pub fn avatar(name: &str, t: &Theme) -> (Color, Color) {
    let mut h: u32 = 2166136261;
    for b in name.bytes() {
        h = (h ^ b as u32).wrapping_mul(16777619);
    }
    let hue = (h % 360) as f32;
    let (s, l) = if t.dark { (0.62, 0.62) } else { (0.62, 0.52) };
    (hsl(hue, s, l), hsl((hue + 38.) % 360., s, l - 0.08))
}
pub fn hsl(h: f32, s: f32, l: f32) -> Color {
    let c = (1. - (2. * l - 1.).abs()) * s;
    let x = c * (1. - ((h / 60.) % 2. - 1.).abs());
    let m = l - c / 2.;
    let (r, g, b) = match (h / 60.) as i32 {
        0 => (c, x, 0.),
        1 => (x, c, 0.),
        2 => (0., c, x),
        3 => (0., x, c),
        4 => (x, 0., c),
        _ => (c, 0., x),
    };
    Color {
        r: r + m,
        g: g + m,
        b: b + m,
        a: 1.,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn legacy_indices_keep_their_names_and_auto_resolves() {
        assert_eq!(THEMES[0].name, "Lavender");
        assert_eq!(THEMES[3].name, "Graphite");
        assert!(resolve(AUTO, true).dark);
        assert!(!resolve(AUTO, false).dark);
    }
    #[test]
    fn text_contrasts_with_background_in_every_theme() {
        for t in THEMES {
            let d = (t.text.luminance() - t.bg.luminance()).abs();
            assert!(d > 0.6, "{} contrast {d}", t.name);
        }
    }
    #[test]
    fn usage_colour_escalates() {
        let t = &THEMES[4];
        assert_eq!(t.usage(10.).0, t.accent);
        assert_eq!(t.usage(99.).0, t.danger);
    }
}
