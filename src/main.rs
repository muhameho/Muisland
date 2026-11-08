#![windows_subsystem = "windows"]
#[cfg(windows)]
mod accounts;
#[cfg(windows)]
mod backup;
#[cfg(windows)]
mod profiles;
#[cfg(windows)]
mod usage;
#[cfg(not(windows))]
fn main() {
    eprintln!("Muisland requires Windows");
}
#[cfg(windows)]
mod island {
    use crate::{
        accounts::{self, Settings},
        profiles,
        usage::{self, Usage, Window},
    };
    use std::{
        mem::zeroed,
        ptr::{null, null_mut},
        sync::mpsc::{self, Receiver},
        time::Instant,
    };
    use windows_sys::Win32::{
        Foundation::*, Graphics::Gdi::*, System::LibraryLoader::GetModuleHandleW,
        UI::WindowsAndMessaging::*,
    };
    #[link(name = "user32")]
    extern "system" {
        fn SetCapture(hwnd: HWND) -> HWND;
        fn ReleaseCapture() -> i32;
        fn SetProcessDPIAware() -> i32;
        fn GetKeyState(key: i32) -> i16;
        fn OpenClipboard(hwnd: HWND) -> i32;
        fn GetClipboardData(format: u32) -> HANDLE;
        fn CloseClipboard() -> i32;
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn GlobalLock(handle: HANDLE) -> *mut core::ffi::c_void;
        fn GlobalUnlock(handle: HANDLE) -> i32;
        fn GlobalSize(handle: HANDLE) -> usize;
    }
    const BG: u32 = 0x000A0C0B;
    const WHITE: u32 = 0x00F1F4F2;
    const MUTED: u32 = 0x00919992;
    const GREEN: u32 = 0x0095E2BB;
    const PANEL: u32 = 0x00171D19;
    const EW: f32 = 420.;
    const EH: f32 = 536.;
    const PILL_W: f32 = 228.;
    const PILL_H: f32 = 58.;
    const CIRCLE: f32 = 76.;
    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    enum Op {
        Save(usize),
        Switch(usize),
        Rollback,
        Recover,
    }
    #[derive(Clone, Debug)]
    enum View {
        Home,
        Details(usize),
        Preferences,
        Notice(String, String),
        Confirm(Op),
        SwitchConfirm(usize),
        Remove(usize, String),
    }
    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    enum Action {
        None,
        Toggle,
        Preferences,
        Back,
        Details(usize),
        Save(usize),
        Switch(usize),
        Add,
        Prev,
        Next,
        Protect,
        Smaller,
        FloatSmaller,
        FloatLarger,
        Remove,
        RemoveConfirm,
        Larger,
        Circle,
        Motion,
        Rollback,
        Recover,
        Quit,
        Confirm,
        Cancel,
        NameField,
        NameSave,
    }
    #[derive(Clone, Copy)]
    struct Button {
        action: Action,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
    }
    impl Button {
        fn contains(&self, x: i32, y: i32) -> bool {
            x >= self.x && x < self.x + self.w && y >= self.y && y < self.y + self.h
        }
    }
    enum Outcome {
        Done(Op, String),
        NeedsClose(Op),
        NeedsSwitch(usize),
        Already(usize),
    }
    struct SyncResult {
        generation: u64,
        active: Option<usize>,
        samples: Vec<(usize, String, Result<Usage, String>)>,
    }
    struct State {
        settings: Settings,
        saved: Vec<bool>,
        identities: Vec<Option<String>>,
        usage_errors: Vec<String>,
        active: Option<usize>,
        page: usize,
        open: bool,
        peek: bool,
        peek_until: f32,
        w: f32,
        h: f32,
        vw: f32,
        vh: f32,
        scale: f32,
        anchor: f32,
        top: i32,
        view: View,
        previous: Option<View>,
        transition: f32,
        page_fade: f32,
        reveal: f32,
        hover: Action,
        hover_t: f32,
        focus: Action,
        down: Option<(i32, i32, Action)>,
        drag: bool,
        origin: (i32, i32),
        edit: Vec<u16>,
        caret: usize,
        selected: bool,
        edit_focus: bool,
        status: String,
        status_until: f32,
        error: bool,
        pending: Option<Op>,
        worker: Option<Receiver<Result<Outcome, String>>>,
        sync: Option<Receiver<SyncResult>>,
        sync_next: f32,
        quota_next: f32,
        generation: u64,
        clock: Instant,
        last: Instant,
        last_second: u64,
        preview: bool,
        network_allowed: bool,
    }
    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }
    fn mix(a: u32, b: u32, t: f32) -> u32 {
        let mut result = 0;
        for shift in [0, 8, 16] {
            let x = ((a >> shift) & 255) as f32;
            let y = ((b >> shift) & 255) as f32;
            result |= ((x + (y - x) * t.clamp(0., 1.)) as u32) << shift;
        }
        result
    }
    fn point(lp: LPARAM, scale: f32) -> (i32, i32) {
        let x = (lp as u32 & 65535) as i16 as f32;
        let y = ((lp as u32 >> 16) & 65535) as i16 as f32;
        ((x / scale) as i32, (y / scale) as i32)
    }
    type Dc = *mut core::ffi::c_void;
    unsafe fn panel(dc: Dc, x: i32, y: i32, w: i32, h: i32, r: i32, color: u32) {
        let brush = CreateSolidBrush(color);
        let old = SelectObject(dc, brush);
        let pen = SelectObject(dc, GetStockObject(NULL_PEN));
        RoundRect(dc, x * 2, y * 2, (x + w) * 2, (y + h) * 2, r * 4, r * 4);
        SelectObject(dc, pen);
        SelectObject(dc, old);
        DeleteObject(brush);
    }
    unsafe fn text(
        dc: Dc,
        x: i32,
        y: i32,
        size: i32,
        weight: i32,
        label: &str,
        color: u32,
        width: i32,
    ) {
        let font = CreateFontW(
            -size * 2,
            0,
            0,
            0,
            weight,
            0,
            0,
            0,
            DEFAULT_CHARSET as u32,
            0,
            0,
            ANTIALIASED_QUALITY as u32,
            0,
            wide("Segoe UI Variable Display").as_ptr(),
        );
        let old = SelectObject(dc, font);
        SetBkMode(dc, TRANSPARENT as i32);
        SetTextColor(dc, color);
        let saved = SaveDC(dc);
        IntersectClipRect(dc, x * 2, y * 2, (x + width.max(0)) * 2, (y + size + 6) * 2);
        let chars: Vec<u16> = label.encode_utf16().collect();
        TextOutW(dc, x * 2, y * 2, chars.as_ptr(), chars.len() as i32);
        RestoreDC(dc, saved);
        SelectObject(dc, old);
        DeleteObject(font);
    }
    unsafe fn line(dc: Dc, pts: &[(i32, i32)], width: i32, color: u32) {
        let pen = CreatePen(PS_SOLID, width * 2, color);
        let old = SelectObject(dc, pen);
        let points: Vec<POINT> = pts
            .iter()
            .map(|(x, y)| POINT { x: x * 2, y: y * 2 })
            .collect();
        Polyline(dc, points.as_ptr(), points.len() as i32);
        SelectObject(dc, old);
        DeleteObject(pen);
    }
    unsafe fn mark(dc: Dc, x: i32, y: i32, size: i32, color: u32) {
        line(
            dc,
            &[
                (x, y + size),
                (x, y),
                (x + size / 2, y + size * 3 / 5),
                (x + size, y),
                (x + size, y + size),
            ],
            2,
            color,
        );
    }
    unsafe fn ring(dc: Dc, cx: f32, cy: f32, r: f32, used: Option<f64>) {
        // Analytic coverage avoids polygon steps in the small circular meter.
        GdiFlush();
        let object = GetCurrentObject(dc, OBJ_BITMAP as u32);
        let mut bitmap: BITMAP = zeroed();
        if GetObjectW(
            object,
            std::mem::size_of::<BITMAP>() as i32,
            (&mut bitmap as *mut BITMAP).cast(),
        ) == 0
            || bitmap.bmBits.is_null()
        {
            return;
        }
        let width = bitmap.bmWidth;
        let height = bitmap.bmHeight.abs();
        let pixels = std::slice::from_raw_parts_mut(
            bitmap.bmBits as *mut u8,
            (bitmap.bmWidthBytes * height) as usize,
        );
        let (cx, cy, r) = (cx * 2., cy * 2., r * 2.);
        let fraction = used.unwrap_or(0.).clamp(0., 100.) as f32 / 100.;
        let end = -std::f32::consts::FRAC_PI_2 + fraction * std::f32::consts::TAU;
        let endx = cx + end.cos() * r;
        let endy = cy + end.sin() * r;
        let accent = if used.unwrap_or(0.) >= 90. {
            0x009BA7F0
        } else if used.unwrap_or(0.) >= 70. {
            0x009FC5F2
        } else {
            GREEN
        };
        for y in ((cy - r - 5.) as i32).max(0)..((cy + r + 5.) as i32).min(height) {
            for x in ((cx - r - 5.) as i32).max(0)..((cx + r + 5.) as i32).min(width) {
                let dx = x as f32 + 0.5 - cx;
                let dy = y as f32 + 0.5 - cy;
                let coverage = (3.5 - (dx.hypot(dy) - r).abs()).clamp(0., 1.);
                let phase = dx.atan2(-dy).rem_euclid(std::f32::consts::TAU) / std::f32::consts::TAU;
                let cap = (3.5 - (x as f32 + 0.5 - endx).hypot(y as f32 + 0.5 - endy))
                    .clamp(0., 1.)
                    .max((3.5 - dx.hypot(dy + r)).clamp(0., 1.));
                let colored = if used.is_some() && fraction > 0. {
                    if phase <= fraction {
                        coverage
                    } else {
                        cap
                    }
                } else {
                    0.
                };
                let total = coverage.max(colored);
                if total <= 0. {
                    continue;
                }
                let color = mix(0x00313B34, accent, (colored / total).clamp(0., 1.));
                let offset = (y * bitmap.bmWidthBytes + x * 4) as usize;
                for channel in 0..3 {
                    let foreground = ((color >> (channel * 8)) & 255) as f32;
                    pixels[offset + channel] =
                        (pixels[offset + channel] as f32 * (1. - total) + foreground * total) as u8;
                }
            }
        }
    }

    fn buttons(s: &State) -> Vec<Button> {
        let mut b = Vec::new();
        let mut add = |action, x, y, w, h| b.push(Button { action, x, y, w, h });
        if !s.open {
            add(Action::Toggle, 0, 0, s.w as i32, s.h as i32);
            return b;
        }
        add(Action::Toggle, 0, 0, EW as i32, 58);
        add(Action::Preferences, EW as i32 - 88, 12, 32, 34);
        add(Action::Toggle, EW as i32 - 46, 12, 32, 34);
        if s.transition < 0.9 || s.reveal < 0.9 {
            return b;
        }
        match &s.view {
            View::Home => {
                for row in 0..2 {
                    let index = s.page * 2 + row;
                    if index >= s.settings.accounts.len() {
                        break;
                    }
                    let y = 126 + row as i32 * 124;
                    add(Action::Switch(index), 16, y, 388, 114);
                    add(Action::Details(index), 296, y + 14, 28, 32);
                    add(Action::Save(index), 329, y + 14, 60, 32);
                }
                if s.page > 0 {
                    add(Action::Prev, 24, 382, 36, 34)
                }
                if (s.page + 1) * 2 < s.settings.accounts.len() {
                    add(Action::Next, 68, 382, 36, 34)
                }
                add(Action::Add, 230, 382, 166, 34);
                add(Action::Protect, 24, 432, 372, 48);
            }
            View::Details(_) => {
                add(Action::NameField, 24, 126, 372, 44);
                add(Action::Back, 24, 410, 120, 42);
                add(Action::NameSave, 156, 410, 240, 42);
                add(Action::Remove, 24, 356, 372, 36);
            }
            View::Preferences => {
                add(Action::Smaller, 264, 126, 38, 36);
                add(Action::Larger, 352, 126, 38, 36);
                add(Action::FloatSmaller, 264, 178, 38, 36);
                add(Action::FloatLarger, 352, 178, 38, 36);
                add(Action::Circle, 24, 232, 372, 48);
                add(Action::Motion, 24, 290, 372, 48);
                add(Action::Rollback, 24, 350, 372, 36);
                add(Action::Recover, 24, 394, 372, 36);
                add(Action::Back, 24, 448, 170, 40);
                add(Action::Quit, 206, 448, 190, 40);
            }
            View::Remove(_, _) => {
                add(Action::Cancel, 24, 312, 120, 42);
                add(Action::RemoveConfirm, 156, 312, 240, 42);
            }
            View::Notice(_, _) => add(Action::Back, 24, 410, 372, 42),
            View::Confirm(_) | View::SwitchConfirm(_) => {
                add(Action::Cancel, 24, 312, 120, 42);
                add(Action::Confirm, 156, 312, 240, 42)
            }
        }
        b
    }
    fn hit(s: &State, x: i32, y: i32) -> Action {
        buttons(s)
            .iter()
            .rev()
            .find(|b| b.contains(x, y))
            .map(|b| b.action)
            .unwrap_or(Action::None)
    }
    fn hot(s: &State, a: Action) -> f32 {
        if s.focus == a {
            1.
        } else if s.hover == a {
            s.hover_t
        } else {
            0.
        }
    }
    unsafe fn button(
        dc: Dc,
        s: &State,
        a: Action,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        label: &str,
        primary: bool,
    ) {
        let pressed = s.down.is_some_and(|(_, _, down)| down == a) && s.hover == a;
        let d = if pressed { 1 } else { 0 };
        let danger = matches!(a, Action::Remove | Action::RemoveConfirm);
        let color = if danger {
            mix(0x00232834, 0x0031374C, hot(s, a))
        } else if primary {
            mix(GREEN, WHITE, hot(s, a) * 0.14)
        } else {
            mix(0x00232B26, 0x00364036, hot(s, a))
        };
        let radius = if w == h {
            h / 2
        } else if h <= 36 {
            10
        } else {
            12
        };
        panel(dc, x + d, y + d, w - 2 * d, h - 2 * d, radius, color);
        let size = if w <= 40 { 18 } else { 13 };
        let font = CreateFontW(
            -size * 2,
            0,
            0,
            0,
            600,
            0,
            0,
            0,
            DEFAULT_CHARSET as u32,
            0,
            0,
            ANTIALIASED_QUALITY as u32,
            0,
            wide("Segoe UI Variable Display").as_ptr(),
        );
        let old = SelectObject(dc, font);
        let chars: Vec<u16> = label.encode_utf16().collect();
        let mut extent: SIZE = zeroed();
        GetTextExtentPoint32W(dc, chars.as_ptr(), chars.len() as i32, &mut extent);
        SelectObject(dc, old);
        DeleteObject(font);
        let tx = x + ((w - extent.cx / 2) / 2).max(6);
        let ty = y + ((h - extent.cy / 2) / 2).max(0);
        text(
            dc,
            tx + d,
            ty + d,
            size,
            600,
            label,
            if danger {
                0x00B2B9F0
            } else if primary {
                BG
            } else {
                WHITE
            },
            w - 12,
        );
        if s.focus == a {
            panel(
                dc,
                x + 12,
                y + h - 4,
                w - 24,
                2,
                1,
                if primary && !danger { BG } else { GREEN },
            );
        }
    }

    unsafe fn toggle(dc: Dc, s: &State, a: Action, y: i32, label: &str, value: bool) {
        panel(dc, 24, y, 372, 48, 16, mix(PANEL, 0x002A342E, hot(s, a)));
        text(dc, 38, y + 16, 13, 500, label, WHITE, 280);
        panel(
            dc,
            340,
            y + 15,
            40,
            18,
            9,
            if value { GREEN } else { 0x00434B45 },
        );
        panel(
            dc,
            if value { 361 } else { 342 },
            y + 17,
            14,
            14,
            7,
            if value { BG } else { WHITE },
        );
    }
    unsafe fn quota(dc: Dc, x: i32, y: i32, width: i32, label: &str, window: Option<&Window>) {
        let now = accounts::now();
        let title = window
            .map(|w| format!("{label} · {:.0}% used", w.used))
            .unwrap_or_else(|| format!("{label} · —"));
        text(dc, x, y, 11, 600, &title, WHITE, width);
        panel(dc, x, y + 18, width, 3, 1, 0x00313B34);
        if let Some(w) = window {
            let color = if w.resets.is_some_and(|t| t <= now) {
                MUTED
            } else if w.used >= 90. {
                0x009BA7F0
            } else if w.used >= 70. {
                0x009FC5F2
            } else {
                GREEN
            };
            let fill = (width as f64 * w.used / 100.) as i32;
            if fill > 0 {
                panel(dc, x, y + 18, fill, 3, 1, color);
            }
        }
        text(
            dc,
            x,
            y + 27,
            10,
            400,
            &window
                .map(|w| w.reset_label(now))
                .unwrap_or_else(|| "Not available".into()),
            MUTED,
            width,
        );
    }
    unsafe fn paragraph(dc: Dc, x: i32, mut y: i32, label: &str, color: u32) {
        let mut current = String::new();
        for word in label.split_whitespace() {
            if current.chars().count() + word.chars().count() > 49 && !current.is_empty() {
                text(dc, x, y, 13, 400, &current, color, 365);
                y += 24;
                current.clear();
            }
            if !current.is_empty() {
                current.push(' ')
            }
            current.push_str(word);
        }
        if !current.is_empty() {
            text(dc, x, y, 13, 400, &current, color, 365);
        }
    }
    unsafe fn draw_view(dc: Dc, s: &State, view: &View) {
        match view {
            View::Home => {
                text(dc, 24, 76, 24, 600, "Your accounts.", WHITE, 370);
                text(
                    dc,
                    25,
                    108,
                    12,
                    400,
                    "Live limits. A seamless switch.",
                    MUTED,
                    370,
                );
                if s.settings.accounts.is_empty() {
                    panel(dc, 16, 126, 388, 238, 20, PANEL);
                    panel(dc, 185, 165, 50, 50, 25, 0x0026352D);
                    mark(dc, 201, 181, 18, GREEN);
                    text(dc, 49, 239, 21, 600, "Your island starts here.", WHITE, 335);
                    text(
                        dc,
                        36,
                        276,
                        12,
                        400,
                        "Add an account, then save your Claude sign-in.",
                        MUTED,
                        350,
                    );
                }
                for row in 0..2 {
                    let i = s.page * 2 + row;
                    let Some(account) = s.settings.accounts.get(i) else {
                        break;
                    };
                    let y = 126 + row as i32 * 124;
                    panel(
                        dc,
                        16,
                        y,
                        388,
                        114,
                        20,
                        mix(PANEL, 0x00242F27, hot(s, Action::Switch(i)) * 0.8),
                    );
                    panel(
                        dc,
                        28,
                        y + 14,
                        34,
                        34,
                        17,
                        if s.active == Some(i) {
                            0x0036483A
                        } else {
                            0x00382F2B
                        },
                    );
                    text(dc, 39, y + 20, 15, 600, &(i + 1).to_string(), GREEN, 25);
                    text(dc, 75, y + 12, 15, 600, &account.name, WHITE, 213);
                    let sub = if s.preview {
                        "Preview · sample data".into()
                    } else if !s.saved[i] {
                        "Not saved · Save to link".into()
                    } else if let Some(u) = &account.usage {
                        format!(
                            "{} · {}{}",
                            if s.active == Some(i) {
                                "Logged in"
                            } else {
                                "Saved"
                            },
                            accounts::age(accounts::now().saturating_sub(u.updated)),
                            if s.usage_errors[i].is_empty() {
                                ""
                            } else {
                                " · stale"
                            }
                        )
                    } else if !s.usage_errors[i].is_empty() {
                        "Usage unavailable · see details".into()
                    } else {
                        if s.network_allowed {
                            "Saved · reading usage…".into()
                        } else {
                            "Saved · usage not yet read".into()
                        }
                    };
                    text(dc, 75, y + 33, 10, 400, &sub, MUTED, 213);
                    button(dc, s, Action::Details(i), 296, y + 15, 28, 30, "···", false);
                    button(dc, s, Action::Save(i), 329, y + 15, 60, 30, "Save", false);
                    quota(
                        dc,
                        28,
                        y + 59,
                        164,
                        "5h",
                        account.usage.as_ref().and_then(|u| u.session.as_ref()),
                    );
                    quota(
                        dc,
                        216,
                        y + 59,
                        176,
                        "7d",
                        account.usage.as_ref().and_then(|u| u.weekly.as_ref()),
                    );
                }
                if s.page > 0 {
                    button(dc, s, Action::Prev, 24, 382, 36, 34, "‹", false)
                }
                if (s.page + 1) * 2 < s.settings.accounts.len() {
                    button(dc, s, Action::Next, 68, 382, 36, 34, "›", false)
                }
                text(
                    dc,
                    116,
                    392,
                    11,
                    400,
                    &if s.settings.accounts.is_empty() {
                        "No accounts".into()
                    } else {
                        format!("{} / {}", s.page + 1, s.settings.accounts.len().div_ceil(2))
                    },
                    MUTED,
                    100,
                );
                button(
                    dc,
                    s,
                    Action::Add,
                    230,
                    382,
                    166,
                    34,
                    "+ Add account",
                    false,
                );
                toggle(
                    dc,
                    s,
                    Action::Protect,
                    432,
                    "Back up local data before switch",
                    s.settings.protect_local,
                );
            }
            View::Details(i) => {
                if let Some(a) = s.settings.accounts.get(*i) {
                    text(dc, 24, 78, 24, 600, "Account details", WHITE, 370);
                    text(
                        dc,
                        25,
                        108,
                        12,
                        400,
                        "Name · up to 32 characters",
                        MUTED,
                        370,
                    );
                    panel(dc, 24, 126, 372, 44, 12, 0x00212723);
                    let label = if matches!(s.view,View::Details(j)if j==*i) {
                        String::from_utf16_lossy(&s.edit)
                    } else {
                        a.name.clone()
                    };
                    if s.selected && s.edit_focus {
                        panel(
                            dc,
                            34,
                            136,
                            ((label.chars().count() * 9) as i32).min(345),
                            24,
                            3,
                            0x004A604F,
                        );
                    }
                    text(dc, 35, 137, 17, 400, &label, WHITE, 345);
                    if s.edit_focus && ((s.clock.elapsed().as_millis() / 500) % 2 == 0) {
                        let cx = 35 + (s.caret as i32 * 8).min(339);
                        panel(dc, cx, 138, 1, 22, 0, GREEN);
                    }
                    text(
                        dc,
                        25,
                        191,
                        12,
                        600,
                        &format!(
                            "PLAN · {}",
                            a.usage
                                .as_ref()
                                .map(|u| u.plan.as_str())
                                .filter(|p| !p.is_empty())
                                .unwrap_or("Unavailable")
                                .to_uppercase()
                        ),
                        WHITE,
                        365,
                    );
                    quota(
                        dc,
                        25,
                        229,
                        165,
                        "5h",
                        a.usage.as_ref().and_then(|u| u.session.as_ref()),
                    );
                    quota(
                        dc,
                        216,
                        229,
                        176,
                        "7d",
                        a.usage.as_ref().and_then(|u| u.weekly.as_ref()),
                    );
                    text(
                        dc,
                        25,
                        293,
                        11,
                        400,
                        if s.preview {
                            "Preview · sample values"
                        } else {
                            "Updates automatically from your Claude session."
                        },
                        MUTED,
                        365,
                    );
                    if let Some(error) = s.usage_errors.get(*i).filter(|e| !e.is_empty()) {
                        text(dc, 25, 324, 11, 400, error, 0x009BA7F0, 365);
                    }
                    button(
                        dc,
                        s,
                        Action::Remove,
                        24,
                        356,
                        372,
                        36,
                        "Remove account",
                        false,
                    );
                    button(dc, s, Action::Back, 24, 410, 120, 42, "Back", false);
                    button(
                        dc,
                        s,
                        Action::NameSave,
                        156,
                        410,
                        240,
                        42,
                        "Save name",
                        true,
                    );
                }
            }
            View::Preferences => {
                text(dc, 24, 78, 24, 600, "Make it yours.", WHITE, 370);
                text(
                    dc,
                    25,
                    109,
                    12,
                    400,
                    "Independent sizes · saved on this device",
                    MUTED,
                    370,
                );
                text(dc, 26, 137, 13, 600, "Menu size", WHITE, 210);
                button(dc, s, Action::Smaller, 264, 126, 38, 36, "−", false);
                text(
                    dc,
                    308,
                    137,
                    12,
                    600,
                    &format!("{}%", (s.settings.menu_scale * 100.).round() as i32),
                    WHITE,
                    44,
                );
                button(dc, s, Action::Larger, 352, 126, 38, 36, "+", false);
                text(dc, 26, 189, 13, 600, "Floating island size", WHITE, 220);
                button(dc, s, Action::FloatSmaller, 264, 178, 38, 36, "−", false);
                text(
                    dc,
                    308,
                    189,
                    12,
                    600,
                    &format!("{}%", (s.settings.float_scale * 100.).round() as i32),
                    WHITE,
                    44,
                );
                button(dc, s, Action::FloatLarger, 352, 178, 38, 36, "+", false);
                toggle(
                    dc,
                    s,
                    Action::Circle,
                    232,
                    "Circle with usage ring",
                    s.settings.circle,
                );
                toggle(
                    dc,
                    s,
                    Action::Motion,
                    290,
                    "Reduce motion",
                    s.settings.reduced_motion,
                );
                button(
                    dc,
                    s,
                    Action::Rollback,
                    24,
                    350,
                    372,
                    36,
                    "Restore previous sign-in",
                    false,
                );
                button(
                    dc,
                    s,
                    Action::Recover,
                    24,
                    394,
                    372,
                    36,
                    "Recover local backup to folder",
                    false,
                );
                button(dc, s, Action::Back, 24, 448, 170, 40, "Back", false);
                button(
                    dc,
                    s,
                    Action::Quit,
                    206,
                    448,
                    190,
                    40,
                    "Quit Muisland",
                    false,
                );
            }
            View::Remove(_, name) => {
                text(dc, 24, 89, 24, 600, "Remove this account?", WHITE, 372);
                paragraph(dc,25,142,&format!("Remove {name} from Muisland? Claude will stay logged in. Your chats and files are unchanged. The saved sign-in is moved to an encrypted local archive."),MUTED);
                button(dc, s, Action::Cancel, 24, 312, 120, 42, "Cancel", false);
                button(
                    dc,
                    s,
                    Action::RemoveConfirm,
                    156,
                    312,
                    240,
                    42,
                    "Remove account",
                    false,
                );
            }
            View::Notice(title, body) => {
                panel(dc, 25, 80, 34, 34, 17, 0x00283530);
                text(dc, 37, 84, 20, 600, "i", GREEN, 20);
                text(dc, 24, 132, 23, 600, title, WHITE, 372);
                paragraph(dc, 25, 180, body, if s.error { 0x009BA7F0 } else { MUTED });
                button(dc, s, Action::Back, 24, 410, 372, 42, "Got it", true)
            }
            View::SwitchConfirm(index) => {
                text(dc, 24, 89, 24, 600, "Switch account?", WHITE, 372);
                text(
                    dc,
                    25,
                    136,
                    16,
                    600,
                    &s.settings.accounts[*index].name,
                    GREEN,
                    365,
                );
                paragraph(dc,25,180,"Confirm to open this saved account. If Claude is running, it will close first. Local tasks may stop and unsaved work may be lost.",MUTED);
                button(dc, s, Action::Cancel, 24, 312, 120, 42, "Cancel", false);
                button(
                    dc,
                    s,
                    Action::Confirm,
                    156,
                    312,
                    240,
                    42,
                    "Confirm switch",
                    true,
                );
            }
            View::Confirm(op) => {
                let saving = matches!(op, Op::Save(_));
                text(
                    dc,
                    24,
                    89,
                    24,
                    600,
                    if saving {
                        "Close Claude & save?"
                    } else {
                        "Close Claude & switch?"
                    },
                    WHITE,
                    372,
                );
                paragraph(dc,25,142,"Claude Desktop will close. Local tasks may stop and unsaved work may be lost. Nothing closes until you confirm.",MUTED);
                if let Op::Save(i) | Op::Switch(i) = op {
                    text(
                        dc,
                        25,
                        248,
                        12,
                        600,
                        &format!("Account: {}", s.settings.accounts[*i].name),
                        WHITE,
                        365,
                    );
                }
                button(dc, s, Action::Cancel, 24, 312, 120, 42, "Cancel", false);
                button(
                    dc,
                    s,
                    Action::Confirm,
                    156,
                    312,
                    240,
                    42,
                    if saving {
                        "Close & save"
                    } else {
                        "Close & switch"
                    },
                    true,
                )
            }
        }
    }
    struct Surface {
        dc: Dc,
        bmp: HBITMAP,
        old: HGDIOBJ,
        bits: *mut u8,
        w: i32,
        h: i32,
    }
    impl Surface {
        unsafe fn new(w: i32, h: i32) -> Self {
            let dc = CreateCompatibleDC(null_mut());
            let mut info: BITMAPINFO = zeroed();
            info.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
            info.bmiHeader.biWidth = w;
            info.bmiHeader.biHeight = -h;
            info.bmiHeader.biPlanes = 1;
            info.bmiHeader.biBitCount = 32;
            info.bmiHeader.biCompression = BI_RGB;
            let mut bits = null_mut();
            let bmp = CreateDIBSection(dc, &info, DIB_RGB_COLORS, &mut bits, null_mut(), 0);
            let old = SelectObject(dc, bmp);
            Self {
                dc,
                bmp,
                old,
                bits: bits.cast(),
                w,
                h,
            }
        }
        unsafe fn clear(&self) {
            let brush = CreateSolidBrush(BG);
            FillRect(
                self.dc,
                &RECT {
                    left: 0,
                    top: 0,
                    right: self.w,
                    bottom: self.h,
                },
                brush,
            );
            DeleteObject(brush);
        }
    }
    impl Drop for Surface {
        fn drop(&mut self) {
            unsafe {
                SelectObject(self.dc, self.old);
                DeleteObject(self.bmp);
                DeleteDC(self.dc);
            }
        }
    }
    unsafe fn render_surface(s: &State) -> Option<Surface> {
        let lw = s.w.round().max(1.) as i32;
        let lh = s.h.round().max(1.) as i32;
        let source = Surface::new(lw * 2, lh * 2);
        if source.bits.is_null() {
            return None;
        }
        source.clear();
        if !s.open && s.settings.circle {
            let p = ((s.w - CIRCLE) / (320. - CIRCLE)).clamp(0., 1.);
            let cx = 38. + p * 14.;
            let cy = 38. + p * 54.;
            let account = s.active.and_then(|i| s.settings.accounts.get(i));
            let u = account.and_then(|a| a.usage.as_ref());
            let used = u
                .and_then(|u| u.session.as_ref().or(u.weekly.as_ref()))
                .map(|w| w.used);
            ring(source.dc, cx, cy, 27., used);
            mark(source.dc, cx as i32 - 9, cy as i32 - 9, 18, WHITE);
            if p > 0.3 {
                let name = account
                    .map(|a| a.name.as_str())
                    .unwrap_or("No linked account");
                text(source.dc, 98, 31, 16, 600, name, WHITE, 205);
                quota(
                    source.dc,
                    98,
                    57,
                    194,
                    "5h",
                    u.and_then(|u| u.session.as_ref()),
                );
                quota(
                    source.dc,
                    98,
                    110,
                    194,
                    "7d",
                    u.and_then(|u| u.weekly.as_ref()),
                );
            }
        } else {
            panel(source.dc, 16, 14, 30, 30, 15, 0x0026352D);
            mark(source.dc, 24, 22, 14, GREEN);
            text(source.dc, 57, 17, 18, 600, "Muisland", WHITE, 170);
            let w = lw;
            button(
                source.dc,
                s,
                Action::Toggle,
                w - 43,
                14,
                29,
                29,
                if s.open { "−" } else { "+" },
                false,
            );
            if s.open {
                text(
                    source.dc,
                    w - 81,
                    15,
                    21,
                    600,
                    "···",
                    mix(MUTED, WHITE, hot(s, Action::Preferences)),
                    30,
                );
            }
            if s.reveal > 0.01 && lh > 70 {
                let layer = Surface::new(lw * 2, lh * 2);
                layer.clear();
                let motion = 1. - (1. - s.transition).powi(3);
                if let Some(previous) = &s.previous {
                    let saved = SaveDC(source.dc);
                    IntersectClipRect(source.dc, 0, 58 * 2, lw * 2, lh * 2);
                    SetViewportOrgEx(source.dc, 0, 0, null_mut());
                    draw_view(source.dc, s, previous);
                    RestoreDC(source.dc, saved);
                }
                let saved = SaveDC(layer.dc);
                SetViewportOrgEx(layer.dc, 0, ((1. - s.reveal) * 12.) as i32 * 2, null_mut());
                draw_view(layer.dc, s, &s.view);
                RestoreDC(layer.dc, saved);
                let page_opacity = if matches!(s.view, View::Home) {
                    1. - (1. - s.page_fade).powi(2)
                } else {
                    1.
                };
                let alpha = (motion * s.reveal * page_opacity * 255.).clamp(0., 255.) as u8;
                AlphaBlend(
                    source.dc,
                    0,
                    58 * 2,
                    lw * 2,
                    (lh - 58) * 2,
                    layer.dc,
                    0,
                    58 * 2,
                    lw * 2,
                    (lh - 58) * 2,
                    BLENDFUNCTION {
                        BlendOp: AC_SRC_OVER as u8,
                        BlendFlags: 0,
                        SourceConstantAlpha: alpha,
                        AlphaFormat: 0,
                    },
                );
                if lh > 500 {
                    let status = if !s.status.is_empty() {
                        s.status.as_str()
                    } else if s.worker.is_some() {
                        "Working on your account…"
                    } else if s.sync.is_some() {
                        "Syncing your account status…"
                    } else {
                        if !s.preview && !s.network_allowed {
                            "Usage paused in this test session"
                        } else {
                            "Usage updates automatically · (...) for settings"
                        }
                    };
                    text(
                        source.dc,
                        25,
                        506,
                        11,
                        400,
                        status,
                        if s.error { 0x009BA7F0 } else { MUTED },
                        370,
                    );
                }
            }
        }
        GdiFlush();
        let pw = (s.w * s.scale).round().max(1.) as i32;
        let ph = (s.h * s.scale).round().max(1.) as i32;
        let output = Surface::new(pw, ph);
        if output.bits.is_null() {
            return None;
        }
        let src = std::slice::from_raw_parts(source.bits, (source.w * source.h * 4) as usize);
        let dest = std::slice::from_raw_parts_mut(output.bits, (pw * ph * 4) as usize);
        let radius = if !s.open && s.settings.circle {
            let p = ((s.w - CIRCLE) / (320. - CIRCLE)).clamp(0., 1.);
            (38. - p * 6.) * s.scale
        } else {
            (29. + s.reveal * 5.) * s.scale
        };
        let hw = pw as f32 / 2.;
        let hh = ph as f32 / 2.;
        let radius = radius.min(hw).min(hh);
        for y in 0..ph {
            for x in 0..pw {
                let sx = ((x as f32 + 0.5) * source.w as f32 / pw as f32 - 0.5)
                    .clamp(0., source.w as f32 - 1.);
                let sy = ((y as f32 + 0.5) * source.h as f32 / ph as f32 - 0.5)
                    .clamp(0., source.h as f32 - 1.);
                let ix = sx as i32;
                let iy = sy as i32;
                let tx = sx - ix as f32;
                let ty = sy - iy as f32;
                let qx = ((x as f32 + 0.5 - hw).abs() - (hw - radius)).max(0.);
                let qy = ((y as f32 + 0.5 - hh).abs() - (hh - radius)).max(0.);
                let alpha = (0.5 - (qx.hypot(qy) - radius)).clamp(0., 1.);
                let d = ((y * pw + x) * 4) as usize;
                for c in 0..3 {
                    let at = |xx: i32, yy: i32| {
                        src[((yy.min(source.h - 1) * source.w + xx.min(source.w - 1)) * 4) as usize
                            + c] as f32
                    };
                    let top = at(ix, iy) * (1. - tx) + at(ix + 1, iy) * tx;
                    let bottom = at(ix, iy + 1) * (1. - tx) + at(ix + 1, iy + 1) * tx;
                    dest[d + c] = ((top * (1. - ty) + bottom * ty) * alpha) as u8;
                }
                dest[d + 3] = (alpha * 255.) as u8;
            }
        }
        Some(output)
    }
    unsafe fn paint(hwnd: HWND, s: &State) {
        let mut ps = zeroed();
        BeginPaint(hwnd, &mut ps);
        if let Some(output) = render_surface(s) {
            let position = POINT {
                x: (s.anchor - output.w as f32 / 2.).round() as i32,
                y: s.top,
            };
            let size = SIZE {
                cx: output.w,
                cy: output.h,
            };
            let origin = POINT { x: 0, y: 0 };
            UpdateLayeredWindow(
                hwnd,
                null_mut(),
                &position,
                &size,
                output.dc,
                &origin,
                0,
                &BLENDFUNCTION {
                    BlendOp: AC_SRC_OVER as u8,
                    BlendFlags: 0,
                    SourceConstantAlpha: 255,
                    AlphaFormat: AC_SRC_ALPHA as u8,
                },
                ULW_ALPHA,
            );
        }
        EndPaint(hwnd, &ps);
    }
    pub fn export_previews(folder: &std::path::Path) -> std::io::Result<()> {
        use std::io::Write;
        std::fs::create_dir_all(folder)?;
        let mut s = initial(true, 0., 0);
        s.open = true;
        s.w = EW;
        s.h = EH;
        s.reveal = 1.;
        s.transition = 1.;
        for name in [
            "accounts",
            "preferences",
            "already-logged-in",
            "account-name",
            "remove-account",
            "circle",
            "circle-hover",
            "three-accounts",
            "small-size",
        ] {
            s.open = true;
            s.w = EW;
            s.h = EH;
            s.view = View::Home;
            s.settings.circle = false;
            s.scale = 1.;
            s.previous = None;
            s.edit_focus = false;
            s.transition = 1.;
            s.reveal = 1.;
            match name{
                "preferences"=>s.view=View::Preferences,
                "remove-account"=>s.view=View::Remove(0,"Personal".into()),
                "account-name"=>{s.view=View::Details(0);s.edit="حساب الشغل".encode_utf16().collect();s.caret=s.edit.len();s.edit_focus=true;s.selected=true;},
                "already-logged-in"=>s.view=View::Notice("Already logged in".into(),"Personal is the account currently logged into Claude. No switch was needed and Claude was left untouched.".into()),
                "circle"=>{s.open=false;s.settings.circle=true;s.w=CIRCLE;s.h=CIRCLE;},
                "circle-hover"=>{s.open=false;s.settings.circle=true;s.w=320.;s.h=184.;s.peek=true;},
                "three-accounts"=>{s.settings.add_account();refresh_slots(&mut s);s.page=1;},
                "small-size"=>{s.page=0;s.scale=0.75;},
                _=>{},
            }
            let surface = unsafe { render_surface(&s) }
                .ok_or_else(|| std::io::Error::other("Preview render failed"))?;
            let bytes = unsafe {
                std::slice::from_raw_parts(surface.bits, (surface.w * surface.h * 4) as usize)
            };
            let mut pixels = bytes.to_vec();
            for pixel in pixels.chunks_exact_mut(4) {
                let alpha = pixel[3] as u32;
                for channel in pixel.iter_mut().take(3) {
                    *channel = (*channel as u32 + 42 * (255 - alpha) / 255).min(255) as u8;
                }
                pixel[3] = 255;
            }
            let mut file = std::fs::File::create(folder.join(format!("{name}.bmp")))?;
            file.write_all(b"BM")?;
            file.write_all(&(54 + pixels.len() as u32).to_le_bytes())?;
            file.write_all(&[0u8; 4])?;
            file.write_all(&54u32.to_le_bytes())?;
            file.write_all(&40u32.to_le_bytes())?;
            file.write_all(&surface.w.to_le_bytes())?;
            file.write_all(&(-surface.h).to_le_bytes())?;
            file.write_all(&1u16.to_le_bytes())?;
            file.write_all(&32u16.to_le_bytes())?;
            file.write_all(&[0u8; 24])?;
            file.write_all(&pixels)?;
        }
        Ok(())
    }

    fn status(s: &mut State, message: impl Into<String>, error: bool) {
        s.status = message.into();
        s.error = error;
        s.status_until = s.clock.elapsed().as_secs_f32() + 6.;
    }
    fn persist(s: &mut State) {
        if !s.preview {
            if let Err(_) = s.settings.save() {
                status(s, "Could not save your preferences", true);
            }
        }
    }
    fn navigate(s: &mut State, next: View, _backwards: bool) {
        s.previous = Some(s.view.clone());
        s.view = next;
        s.transition = if s.settings.reduced_motion { 1. } else { 0. };
        s.edit_focus = false;
        s.focus = Action::None;
        s.open = true;
        s.peek = false;
    }
    fn notice(s: &mut State, title: &str, body: impl Into<String>, error: bool) {
        s.error = error;
        s.status.clear();
        navigate(s, View::Notice(title.into(), body.into()), false);
    }
    fn refresh_slots(s: &mut State) {
        s.saved = (0..s.settings.accounts.len())
            .map(|i| {
                if s.preview {
                    true
                } else {
                    profiles::saved(s.settings.accounts[i].slot)
                }
            })
            .collect();
        s.identities = (0..s.settings.accounts.len())
            .map(|i| {
                if s.preview {
                    Some(format!("preview-{}", s.settings.accounts[i].slot))
                } else {
                    profiles::account_identity(s.settings.accounts[i].slot).ok()
                }
            })
            .collect();
        s.usage_errors
            .resize(s.settings.accounts.len(), String::new());
    }
    fn sync(s: &mut State) {
        if s.sync.is_some() || s.worker.is_some() {
            return;
        }
        let now = s.clock.elapsed().as_secs_f32();
        s.sync_next = now + 10.;
        let fetch = now >= s.quota_next && s.network_allowed;
        if fetch {
            s.quota_next = now + 300.;
        }
        let slots = s
            .identities
            .iter()
            .cloned()
            .zip(s.settings.accounts.iter().map(|a| a.slot))
            .collect::<Vec<_>>();
        let generation = s.generation;
        let preview = s.preview;
        let (tx, rx) = mpsc::channel();
        s.sync = Some(rx);
        std::thread::spawn(move || {
            let current = if preview {
                Some("preview-1".into())
            } else {
                usage::current_account_identity().ok()
            };
            let active = current.as_ref().and_then(|id| {
                slots
                    .iter()
                    .position(|(identity, _)| identity.as_ref() == Some(id))
            });
            let mut samples = Vec::new();
            if fetch && !preview {
                for (id, slot) in slots {
                    if let Some(id) = id {
                        samples.push((slot, id, usage::fetch(slot).map_err(|e| e.to_string())));
                    }
                }
            }
            let _ = tx.send(SyncResult {
                generation,
                active,
                samples,
            });
        });
    }
    fn begin(s: &mut State, op: Op, confirmed: bool) {
        if s.worker.is_some() {
            status(s, "Please wait for the current action", false);
            return;
        }
        s.generation += 1;
        s.edit_focus = false;
        let preview = s.preview;
        let active = s.active;
        let protect = s.settings.protect_local;
        let slot = match op {
            Op::Save(i) | Op::Switch(i) => s.settings.accounts[i].slot,
            _ => 0,
        };
        let (tx, rx) = mpsc::channel();
        s.worker = Some(rx);
        s.pending = Some(op);
        status(
            s,
            if confirmed {
                "Closing Claude, then continuing…"
            } else {
                "Checking your account…"
            },
            false,
        );
        std::thread::spawn(move || {
            let result = (|| -> Result<Outcome, String> {
                if let Op::Switch(i) = op {
                    if (preview && active == Some(i))
                        || (!preview && profiles::is_current(slot).unwrap_or(false))
                    {
                        return Ok(Outcome::Already(i));
                    }
                    if !confirmed {
                        return Ok(Outcome::NeedsSwitch(i));
                    }
                }
                if matches!(op, Op::Save(_) | Op::Switch(_))
                    && !confirmed
                    && (preview || profiles::claude_running())
                {
                    return Ok(Outcome::NeedsClose(op));
                }
                if preview {
                    return Ok(Outcome::Done(
                        op,
                        "Preview complete · no account changes".into(),
                    ));
                }
                if confirmed {
                    profiles::close_claude_for_save().map_err(|e| e.to_string())?;
                }
                match op {
                    Op::Save(_) => {
                        profiles::capture(slot).map_err(|e| e.to_string())?;
                        Ok(Outcome::Done(op, "Account saved".into()))
                    }
                    Op::Switch(_) => {
                        profiles::switch(slot, protect).map_err(|e| e.to_string())?;
                        profiles::open_claude();
                        Ok(Outcome::Done(op, "Switched successfully".into()))
                    }
                    Op::Rollback => {
                        profiles::restore_backup().map_err(|e| e.to_string())?;
                        Ok(Outcome::Done(op, "Previous sign-in restored".into()))
                    }
                    Op::Recover => {
                        let path = crate::backup::recover_latest().map_err(|e| e.to_string())?;
                        let _ = std::process::Command::new("explorer.exe").arg(path).spawn();
                        Ok(Outcome::Done(
                            op,
                            "Backup recovered to a separate folder".into(),
                        ))
                    }
                }
            })();
            let _ = tx.send(result);
        });
    }
    fn name_save(s: &mut State) {
        if let View::Details(i) = s.view {
            match accounts::valid_name(&String::from_utf16_lossy(&s.edit)) {
                Ok(name) => {
                    let old = s.settings.accounts[i].name.clone();
                    s.settings.accounts[i].name = name;
                    if !s.preview && s.settings.save().is_err() {
                        s.settings.accounts[i].name = old;
                        status(s, "Could not save the account name", true);
                    } else {
                        navigate(s, View::Home, true);
                        status(s, "Account name saved", false);
                    }
                }
                Err(e) => status(s, e.to_string(), true),
            }
        }
    }
    fn remove_account(s: &mut State, index: usize) {
        if s.worker.is_some() {
            status(
                s,
                "Wait for the current action before removing an account",
                false,
            );
            return;
        }
        if index >= s.settings.accounts.len() {
            return;
        }
        let slot = s.settings.accounts[index].slot;
        let old = s.settings.clone();
        let active_slot = s
            .active
            .and_then(|i| s.settings.accounts.get(i))
            .map(|a| a.slot);
        let archive = if s.preview {
            None
        } else {
            match profiles::archive_account(slot) {
                Ok(path) => path,
                Err(e) => {
                    notice(s, "Could not remove account", e.to_string(), true);
                    return;
                }
            }
        };
        s.settings.accounts.remove(index);
        if !s.preview && s.settings.save().is_err() {
            s.settings = old;
            if let Some(path) = archive {
                let _ = profiles::restore_archived_account(slot, &path);
            }
            notice(
                s,
                "Could not remove account",
                "Preferences could not be saved. Your account entry has been kept.",
                true,
            );
            return;
        }
        if index < s.usage_errors.len() {
            s.usage_errors.remove(index);
        }
        s.generation += 1;
        s.sync = None;
        s.sync_next = 0.;
        s.quota_next = 0.;
        refresh_slots(s);
        s.active =
            active_slot.and_then(|slot| s.settings.accounts.iter().position(|a| a.slot == slot));
        s.page = s
            .page
            .min(s.settings.accounts.len().div_ceil(2).saturating_sub(1));
        navigate(s, View::Home, true);
        status(s, "Account removed from Muisland", false);
    }
    fn change_page(s: &mut State, next: usize) {
        s.page = next;
        s.previous = None;
        s.transition = 1.;
        s.page_fade = if s.settings.reduced_motion { 1. } else { 0.2 };
        s.view = View::Home;
        s.edit_focus = false;
        s.focus = Action::None;
    }
    unsafe fn activate(hwnd: HWND, s: &mut State, a: Action) {
        match a {
            Action::None => {}
            Action::Toggle => {
                s.open = !s.open;
                s.peek = false;
                s.edit_focus = false;
                if !s.open {
                    s.view = View::Home;
                    s.previous = None;
                    s.transition = 1.;
                }
            }
            Action::Preferences => navigate(s, View::Preferences, false),
            Action::Back | Action::Cancel => navigate(s, View::Home, true),
            Action::Details(i) => {
                s.edit = s.settings.accounts[i].name.encode_utf16().collect();
                s.caret = s.edit.len();
                s.selected = true;
                navigate(s, View::Details(i), false);
                s.edit_focus = true;
            }
            Action::NameField => {
                s.edit_focus = true;
                s.caret = s.edit.len();
                s.selected = true;
            }
            Action::NameSave => name_save(s),
            Action::Remove => {
                if let View::Details(i) = s.view {
                    if s.worker.is_some() {
                        status(s, "Wait for the current action", false);
                    } else {
                        navigate(
                            s,
                            View::Remove(i, s.settings.accounts[i].name.clone()),
                            false,
                        );
                        s.focus = Action::Cancel;
                    }
                }
            }
            Action::RemoveConfirm => {
                if let View::Remove(i, _) = s.view {
                    remove_account(s, i);
                }
            }
            Action::Switch(i) => begin(s, Op::Switch(i), false),
            Action::Save(i) => begin(s, Op::Save(i), false),
            Action::Confirm => {
                let operation = match s.view {
                    View::Confirm(op) => Some(op),
                    View::SwitchConfirm(i) => Some(Op::Switch(i)),
                    _ => None,
                };
                if let Some(op) = operation {
                    navigate(s, View::Home, true);
                    begin(s, op, true);
                }
            }
            Action::Add => {
                if let Some(i) = s.settings.add_account() {
                    if !s.preview && s.settings.save().is_err() {
                        s.settings.accounts.pop();
                        notice(
                            s,
                            "Could not add account",
                            "Preferences could not be saved. No account slot was added.",
                            true,
                        );
                        return;
                    }
                    refresh_slots(s);
                    s.page = i / 2;
                    s.edit = s.settings.accounts[i].name.encode_utf16().collect();
                    s.caret = s.edit.len();
                    s.selected = true;
                    navigate(s, View::Details(i), false);
                    s.edit_focus = true;
                } else {
                    notice(
                        s,
                        "Account slots full",
                        format!(
                            "You can save up to {} accounts on this device.",
                            accounts::MAX_ACCOUNTS
                        ),
                        false,
                    );
                }
            }
            Action::Prev => {
                if s.page > 0 {
                    change_page(s, s.page - 1);
                }
            }
            Action::Next => {
                if (s.page + 1) * 2 < s.settings.accounts.len() {
                    change_page(s, s.page + 1);
                }
            }
            Action::Protect => {
                s.settings.protect_local = !s.settings.protect_local;
                persist(s)
            }
            Action::Smaller => {
                s.settings.menu_scale = (s.settings.menu_scale - 0.1).max(0.75);
                persist(s)
            }
            Action::Larger => {
                s.settings.menu_scale = (s.settings.menu_scale + 0.1).min(1.6);
                persist(s)
            }
            Action::FloatSmaller => {
                s.settings.float_scale = (s.settings.float_scale - 0.1).max(0.6);
                persist(s)
            }
            Action::FloatLarger => {
                s.settings.float_scale = (s.settings.float_scale + 0.1).min(1.8);
                persist(s)
            }
            Action::Circle => {
                s.settings.circle = !s.settings.circle;
                persist(s)
            }
            Action::Motion => {
                s.settings.reduced_motion = !s.settings.reduced_motion;
                persist(s)
            }
            Action::Rollback => begin(s, Op::Rollback, false),
            Action::Recover => begin(s, Op::Recover, false),
            Action::Quit => {
                if s.worker.is_some() {
                    status(s, "Wait for the current action before quitting", false)
                } else {
                    DestroyWindow(hwnd);
                    return;
                }
            }
        }
        InvalidateRect(hwnd, null_mut(), 0);
    }
    fn replace_selection(s: &mut State) {
        if s.selected {
            s.edit.clear();
            s.caret = 0;
            s.selected = false;
        }
    }
    unsafe fn paste(hwnd: HWND, s: &mut State) {
        if OpenClipboard(hwnd) == 0 {
            return;
        }
        let h = GetClipboardData(13);
        if !h.is_null() {
            let ptr = GlobalLock(h) as *const u16;
            if !ptr.is_null() {
                let max = (GlobalSize(h) / 2).min(4096);
                let slice = std::slice::from_raw_parts(ptr, max);
                let count = slice.iter().position(|x| *x == 0).unwrap_or(max);
                let content = String::from_utf16_lossy(&slice[..count]);
                replace_selection(s);
                for c in content.chars().filter(|c| !c.is_control()).take(32) {
                    let mut buf = [0; 2];
                    for u in c.encode_utf16(&mut buf) {
                        if s.edit.len() < 64 {
                            s.edit.insert(s.caret, *u);
                            s.caret += 1;
                        }
                    }
                }
                GlobalUnlock(h);
            }
        }
        CloseClipboard();
    }
    unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
        let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut State;
        if ptr.is_null() {
            return DefWindowProcW(hwnd, msg, wp, lp);
        }
        if msg == WM_DESTROY {
            KillTimer(hwnd, 17);
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            drop(Box::from_raw(ptr));
            PostQuitMessage(0);
            return 0;
        }
        let s = &mut *ptr;
        match msg {
            WM_ERASEBKGND => 1,
            WM_PAINT => {
                paint(hwnd, s);
                0
            }
            WM_CLOSE => {
                activate(hwnd, s, Action::Quit);
                0
            }
            WM_SETCURSOR => {
                let mut p = zeroed();
                GetCursorPos(&mut p);
                ScreenToClient(hwnd, &mut p);
                let action = hit(
                    s,
                    (p.x as f32 / s.scale) as i32,
                    (p.y as f32 / s.scale) as i32,
                );
                SetCursor(LoadCursorW(
                    null_mut(),
                    if action == Action::NameField {
                        IDC_IBEAM
                    } else if action != Action::None {
                        IDC_HAND
                    } else {
                        IDC_ARROW
                    },
                ));
                1
            }
            WM_TIMER => {
                let now = Instant::now();
                let dt = now.duration_since(s.last).as_secs_f32().min(0.05);
                s.last = now;
                let t = s.clock.elapsed().as_secs_f32();
                let before = (
                    s.w,
                    s.h,
                    s.scale,
                    s.reveal,
                    s.transition,
                    s.hover,
                    s.hover_t,
                    s.peek,
                );
                let mut cursor = zeroed();
                GetCursorPos(&mut cursor);
                ScreenToClient(hwnd, &mut cursor);
                let x = cursor.x as f32 / s.scale;
                let y = cursor.y as f32 / s.scale;
                let inside = x >= 0. && x < s.w && y >= 0. && y < s.h;
                if !s.open && s.settings.circle {
                    if inside && (s.peek || ((x - 38.).powi(2) + (y - 38.).powi(2) < 38f32.powi(2)))
                    {
                        s.peek = true;
                        s.peek_until = t + 0.3;
                    } else if t > s.peek_until {
                        s.peek = false;
                    }
                }
                let (tw, th) = if s.open {
                    (EW, EH)
                } else if s.settings.circle {
                    if s.peek {
                        (320., 184.)
                    } else {
                        (CIRCLE, CIRCLE)
                    }
                } else {
                    (PILL_W, PILL_H)
                };
                let target_scale = if s.open {
                    s.settings.menu_scale
                } else {
                    s.settings.float_scale
                };
                let old_page_fade = s.page_fade;
                s.page_fade = if s.settings.reduced_motion {
                    1.
                } else {
                    (s.page_fade + dt * 10.).min(1.)
                };
                if s.settings.reduced_motion {
                    s.w = tw;
                    s.h = th;
                    s.vw = 0.;
                    s.vh = 0.;
                    s.scale = target_scale;
                    s.reveal = if s.open { 1. } else { 0. };
                    s.transition = 1.;
                } else {
                    let steps = (dt / 0.004).ceil().max(1.) as usize;
                    let step = dt / steps as f32;
                    for _ in 0..steps {
                        s.vw += ((tw - s.w) * 360. - s.vw * 32.) * step;
                        s.vh += ((th - s.h) * 340. - s.vh * 31.) * step;
                        s.w += s.vw * step;
                        s.h += s.vh * step;
                    }
                    if (tw - s.w).abs() < 0.04 && s.vw.abs() < 0.3 {
                        s.w = tw;
                        s.vw = 0.;
                    }
                    if (th - s.h).abs() < 0.04 && s.vh.abs() < 0.3 {
                        s.h = th;
                        s.vh = 0.;
                    }
                    s.scale += (target_scale - s.scale) * (dt * 14.).min(1.);
                    if (s.scale - target_scale).abs() < 0.001 {
                        s.scale = target_scale;
                    }
                    let reveal = if s.open {
                        ((s.h - 58.) / (EH - 58.) * 1.35 - 0.25).clamp(0., 1.)
                    } else {
                        0.
                    };
                    s.reveal += (reveal - s.reveal) * (dt * 18.).min(1.);
                    if (reveal - s.reveal).abs() < 0.002 {
                        s.reveal = reveal;
                    }
                    s.transition = (s.transition + dt * 7.).min(1.);
                }
                if s.transition >= 1. {
                    s.previous = None;
                }
                let hover = hit(s, x as i32, y as i32);
                if hover != s.hover {
                    s.hover = hover;
                    s.hover_t = 0.;
                }
                s.hover_t += (1. - s.hover_t) * (dt * 16.).min(1.);
                if s.hover_t > 0.998 {
                    s.hover_t = 1.;
                }
                let mut changed = old_page_fade != s.page_fade;
                if let Some(result) = s.sync.as_ref().and_then(|r| r.try_recv().ok()) {
                    s.sync = None;
                    if result.generation == s.generation {
                        s.active = result.active;
                    }
                    let mut measurements = false;
                    for (slot, id, result) in result.samples {
                        let Some(i) = s.settings.accounts.iter().position(|a| a.slot == slot)
                        else {
                            continue;
                        };
                        if s.identities.get(i).and_then(|i| i.as_ref()) != Some(&id) {
                            continue;
                        }
                        match result {
                            Ok(u) => {
                                s.settings.accounts[i].usage = Some(u);
                                s.usage_errors[i].clear();
                                measurements = true
                            }
                            Err(e) => s.usage_errors[i] = e,
                        }
                    }
                    if measurements {
                        persist(s)
                    }
                    changed = true;
                }
                if let Some(result) = s.worker.as_ref().and_then(|r| r.try_recv().ok()) {
                    s.worker = None;
                    s.pending = None;
                    match result {
                        Ok(Outcome::Already(i)) => {
                            s.active = Some(i);
                            notice(s,"Already logged in",format!("{} is the account currently logged into Claude. No switch was needed and Claude was left untouched.",s.settings.accounts[i].name),false)
                        }
                        Ok(Outcome::NeedsSwitch(i)) => {
                            s.status.clear();
                            navigate(s, View::SwitchConfirm(i), false);
                            s.focus = Action::Cancel;
                        }
                        Ok(Outcome::NeedsClose(op)) => {
                            s.status.clear();
                            navigate(s, View::Confirm(op), false);
                            s.focus = Action::Cancel;
                        }
                        Ok(Outcome::Done(op, label)) => {
                            if let Op::Save(i) = op {
                                s.settings.accounts[i].usage = None;
                                s.usage_errors[i].clear();
                                persist(s);
                            }
                            if let Op::Switch(i) = op {
                                s.active = Some(i);
                            }
                            refresh_slots(s);
                            s.quota_next = 0.;
                            s.sync_next = 0.;
                            navigate(s, View::Home, true);
                            status(s, label, false);
                        }
                        Err(e) => notice(s, "Couldn't finish this action", e, true),
                    }
                    changed = true;
                }
                if t >= s.sync_next {
                    sync(s)
                }
                if !s.status.is_empty() && t > s.status_until {
                    s.status.clear();
                    s.error = false;
                    changed = true;
                }
                let second = accounts::now();
                if second != s.last_second {
                    s.last_second = second;
                    changed = true;
                }
                if before
                    != (
                        s.w,
                        s.h,
                        s.scale,
                        s.reveal,
                        s.transition,
                        s.hover,
                        s.hover_t,
                        s.peek,
                    )
                    || changed
                    || s.edit_focus
                {
                    InvalidateRect(hwnd, null_mut(), 0);
                }
                0
            }
            WM_LBUTTONDOWN => {
                let (x, y) = point(lp, s.scale);
                let action = hit(s, x, y);
                s.down = Some((x, y, action));
                s.drag = false;
                s.focus = Action::None;
                if y < 58 && x < if s.open { EW as i32 - 100 } else { 120 } {
                    let mut p = zeroed();
                    GetCursorPos(&mut p);
                    s.origin = (p.x - (s.anchor - s.w * s.scale / 2.) as i32, p.y - s.top);
                    SetCapture(hwnd);
                }
                InvalidateRect(hwnd, null_mut(), 0);
                0
            }
            WM_MOUSEMOVE => {
                if let Some((dx, dy, _)) = s.down {
                    let (x, y) = point(lp, s.scale);
                    if dy < 58
                        && dx < if s.open { EW as i32 - 100 } else { 120 }
                        && ((x - dx).abs() > 4 || (y - dy).abs() > 4 || s.drag)
                    {
                        s.drag = true;
                        let mut p = zeroed();
                        GetCursorPos(&mut p);
                        s.anchor = (p.x - s.origin.0) as f32 + s.w * s.scale / 2.;
                        s.top = p.y - s.origin.1;
                        InvalidateRect(hwnd, null_mut(), 0);
                    }
                }
                0
            }
            WM_LBUTTONUP => {
                let down = s.down.take();
                let drag = s.drag;
                ReleaseCapture();
                let (x, y) = point(lp, s.scale);
                if let Some((dx, dy, action)) = down {
                    if !drag && (x - dx).abs() < 8 && (y - dy).abs() < 8 && hit(s, x, y) == action {
                        activate(hwnd, s, action);
                    }
                }
                s.drag = false;
                0
            }
            WM_CAPTURECHANGED => {
                s.down = None;
                s.drag = false;
                0
            }
            WM_RBUTTONUP => {
                navigate(s, View::Preferences, false);
                InvalidateRect(hwnd, null_mut(), 0);
                0
            }
            WM_CHAR => {
                if s.edit_focus {
                    let c = wp as u16;
                    if c == 8 {
                        replace_selection(s);
                        if s.caret > 0 {
                            s.caret -= 1;
                            let low = s.edit.remove(s.caret);
                            if (0xDC00..=0xDFFF).contains(&low) && s.caret > 0 {
                                s.caret -= 1;
                                s.edit.remove(s.caret);
                            }
                        }
                    } else if c >= 32 && c != 127 && s.edit.len() < 64 {
                        replace_selection(s);
                        s.edit.insert(s.caret, c);
                        s.caret += 1;
                    }
                    InvalidateRect(hwnd, null_mut(), 0);
                }
                0
            }
            WM_KEYDOWN => {
                let control = GetKeyState(0x11) < 0;
                let key = wp as u32;
                if s.edit_focus && control && key == 0x41 {
                    s.selected = true;
                } else if s.edit_focus && control && key == 0x56 {
                    paste(hwnd, s);
                } else if key == 0x1B {
                    if s.open && !matches!(s.view, View::Home) {
                        navigate(s, View::Home, true)
                    } else {
                        s.open = false;
                        s.peek = false;
                        s.edit_focus = false;
                    }
                } else if s.edit_focus && key == 0x0D {
                    name_save(s)
                } else if s.edit_focus && key == 0x24 {
                    s.caret = 0;
                    s.selected = false;
                } else if s.edit_focus && key == 0x23 {
                    s.caret = s.edit.len();
                    s.selected = false;
                } else if s.edit_focus && key == 0x25 {
                    s.caret = s.caret.saturating_sub(1);
                    s.selected = false;
                } else if s.edit_focus && key == 0x27 {
                    s.caret = (s.caret + 1).min(s.edit.len());
                    s.selected = false;
                } else if s.edit_focus && key == 0x2E {
                    if s.selected {
                        replace_selection(s)
                    } else if s.caret < s.edit.len() {
                        s.edit.remove(s.caret);
                    }
                } else if key == 0x09 {
                    s.edit_focus = false;
                    let b = buttons(s);
                    let current = b.iter().position(|b| b.action == s.focus);
                    if !b.is_empty() {
                        s.focus = b[current.map(|i| (i + 1) % b.len()).unwrap_or(0)].action;
                    }
                } else if (key == 0x0D || key == 0x20) && !s.edit_focus {
                    let a = if s.focus == Action::None {
                        if matches!(
                            s.view,
                            View::Confirm(_) | View::SwitchConfirm(_) | View::Remove(_, _)
                        ) {
                            Action::Cancel
                        } else {
                            Action::Toggle
                        }
                    } else {
                        s.focus
                    };
                    activate(hwnd, s, a);
                }
                InvalidateRect(hwnd, null_mut(), 0);
                0
            }
            _ => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }
    fn initial(preview: bool, anchor: f32, top: i32) -> State {
        let now = Instant::now();
        let mut settings = if preview {
            Settings::default()
        } else {
            Settings::load().unwrap_or_default()
        };
        if preview {
            settings.accounts[0].name = "Personal".into();
            settings.accounts[1].name = "Work".into();
            for i in 0..2 {
                settings.accounts[i].usage = Some(Usage {
                    session: Some(Window {
                        used: if i == 0 { 28. } else { 82. },
                        resets: Some(accounts::now() + 7200),
                    }),
                    weekly: Some(Window {
                        used: 46.,
                        resets: Some(accounts::now() + 86400 * 3),
                    }),
                    updated: accounts::now(),
                    plan: "Demo".into(),
                });
            }
        }
        let circle = settings.circle;
        let scale = if preview {
            settings.menu_scale
        } else {
            settings.float_scale
        };
        let mut s = State {
            settings,
            saved: Vec::new(),
            identities: Vec::new(),
            usage_errors: Vec::new(),
            active: if preview { Some(0) } else { None },
            page: 0,
            open: preview,
            peek: false,
            peek_until: 0.,
            w: if circle { CIRCLE } else { PILL_W },
            h: if circle { CIRCLE } else { PILL_H },
            vw: 0.,
            vh: 0.,
            scale,
            anchor,
            top,
            view: View::Home,
            previous: None,
            transition: 1.,
            page_fade: 1.,
            reveal: 0.,
            hover: Action::None,
            hover_t: 0.,
            focus: Action::None,
            down: None,
            drag: false,
            origin: (0, 0),
            edit: Vec::new(),
            caret: 0,
            selected: false,
            edit_focus: false,
            status: String::new(),
            status_until: 0.,
            error: false,
            pending: None,
            worker: None,
            sync: None,
            sync_next: 0.,
            quota_next: 0.,
            generation: 0,
            clock: now,
            last: now,
            last_second: accounts::now(),
            preview,
            network_allowed: !preview && !std::env::args().any(|a| a == "--no-network"),
        };
        refresh_slots(&mut s);
        s
    }
    pub fn run() {
        unsafe {
            SetProcessDPIAware();
            let class = wide("MuislandWindow");
            let existing = FindWindowW(class.as_ptr(), null());
            if !existing.is_null() {
                SetForegroundWindow(existing);
                return;
            }
            let instance = GetModuleHandleW(null());
            let wc = WNDCLASSW {
                lpfnWndProc: Some(wndproc),
                hInstance: instance,
                hCursor: LoadCursorW(null_mut(), IDC_ARROW),
                lpszClassName: class.as_ptr(),
                ..zeroed()
            };
            if RegisterClassW(&wc) == 0 {
                return;
            }
            let mut area: RECT = zeroed();
            SystemParametersInfoW(SPI_GETWORKAREA, 0, (&mut area as *mut RECT).cast(), 0);
            let preview = std::env::args().any(|a| a == "--preview");
            let s = Box::new(initial(
                preview,
                (area.left + area.right) as f32 / 2.,
                area.top + 20,
            ));
            let hwnd = CreateWindowExW(
                WS_EX_TOPMOST
                    | WS_EX_LAYERED
                    | if preview {
                        WS_EX_APPWINDOW
                    } else {
                        WS_EX_TOOLWINDOW
                    },
                class.as_ptr(),
                wide("Muisland").as_ptr(),
                WS_POPUP,
                (s.anchor - s.w * s.scale / 2.) as i32,
                s.top,
                (s.w * s.scale) as i32,
                (s.h * s.scale) as i32,
                null_mut(),
                null_mut(),
                instance,
                null(),
            );
            if hwnd.is_null() {
                return;
            }
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(s) as isize);
            ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            SetTimer(hwnd, 17, 16, None);
            InvalidateRect(hwnd, null_mut(), 0);
            let mut message: MSG = zeroed();
            while GetMessageW(&mut message, null_mut(), 0, 0) > 0 {
                TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
    }
    #[cfg(test)]
    mod tests {
        use super::*;
        #[test]
        fn account_hit_targets_are_not_limited_to_two() {
            let mut s = initial(true, 0., 0);
            for _ in 0..4 {
                s.settings.add_account();
            }
            refresh_slots(&mut s);
            s.page = 2;
            s.open = true;
            s.transition = 1.;
            s.reveal = 1.;
            assert_eq!(hit(&s, 80, 150), Action::Switch(4));
            assert_eq!(hit(&s, 350, 150), Action::Save(4));
            assert_eq!(hit(&s, 307, 274), Action::Details(5));
        }
        #[test]
        fn rounded_window_uses_transparent_antialiased_edges_without_a_stroke() {
            let mut s = initial(true, 0., 0);
            s.w = EW;
            s.h = EH;
            s.open = true;
            s.reveal = 1.;
            let surface = unsafe { render_surface(&s) }.unwrap();
            let bytes = unsafe {
                std::slice::from_raw_parts(surface.bits, (surface.w * surface.h * 4) as usize)
            };
            assert_eq!(bytes[3], 0);
            assert_eq!(
                bytes[((surface.h / 2 * surface.w + surface.w / 2) * 4 + 3) as usize],
                255
            );
            assert!(bytes.chunks_exact(4).any(|p| p[3] > 0 && p[3] < 255));
        }
        #[test]
        fn remove_middle_account_keeps_remaining_slot_ids() {
            let mut s = initial(true, 0., 0);
            s.settings.add_account();
            refresh_slots(&mut s);
            remove_account(&mut s, 1);
            assert_eq!(
                s.settings
                    .accounts
                    .iter()
                    .map(|a| a.slot)
                    .collect::<Vec<_>>(),
                vec![1, 3]
            );
            s.settings.add_account();
            assert_eq!(s.settings.accounts[2].slot, 2);
        }
        #[test]
        fn removing_last_account_leaves_add_available() {
            let mut s = initial(true, 0., 0);
            remove_account(&mut s, 1);
            remove_account(&mut s, 0);
            s.reveal = 1.;
            s.transition = 1.;
            assert!(s.settings.accounts.is_empty());
            assert!(s.active.is_none());
            assert_eq!(s.page, 0);
            assert!(buttons(&s).iter().any(|b| b.action == Action::Add));
        }
        #[test]
        fn preference_size_controls_are_independent() {
            let mut s = initial(true, 0., 0);
            s.view = View::Preferences;
            s.reveal = 1.;
            s.transition = 1.;
            assert_eq!(hit(&s, 371, 144), Action::Larger);
            assert_eq!(hit(&s, 371, 196), Action::FloatLarger);
            s.settings.menu_scale = 1.4;
            assert_eq!(s.settings.float_scale, 1.0);
        }
        #[test]
        fn page_navigation_uses_only_the_short_fade() {
            let mut s = initial(true, 0., 0);
            change_page(&mut s, 1);
            assert_eq!(s.transition, 1.);
            assert!(s.previous.is_none());
            assert!(s.page_fade < 1.);
        }
        #[test]
        fn selecting_current_account_never_requests_a_close_or_switch() {
            let mut s = initial(true, 0., 0);
            begin(&mut s, Op::Switch(0), false);
            let result = s
                .worker
                .take()
                .unwrap()
                .recv_timeout(std::time::Duration::from_secs(2))
                .unwrap()
                .unwrap();
            assert!(matches!(result, Outcome::Already(0)));
        }
        #[test]
        fn all_header_areas_collapse_except_the_settings_control() {
            let mut s = initial(true, 0., 0);
            s.reveal = 1.;
            s.transition = 1.;
            for (x, y) in [(18, 22), (105, 28), (220, 35), (300, 12), (413, 50)] {
                assert_eq!(hit(&s, x, y), Action::Toggle);
            }
            assert_eq!(hit(&s, 347, 29), Action::Preferences);
        }
        #[test]
        fn switch_only_proceeds_after_confirmation_in_preview() {
            let mut s = initial(true, 0., 0);
            begin(&mut s, Op::Switch(1), true);
            let result = s
                .worker
                .take()
                .unwrap()
                .recv_timeout(std::time::Duration::from_secs(2))
                .unwrap()
                .unwrap();
            assert!(matches!(result, Outcome::Done(Op::Switch(1), _)));
        }
        #[test]
        fn another_account_requires_close_confirmation_before_changes() {
            let mut s = initial(true, 0., 0);
            begin(&mut s, Op::Switch(1), false);
            let result = s
                .worker
                .take()
                .unwrap()
                .recv_timeout(std::time::Duration::from_secs(2))
                .unwrap()
                .unwrap();
            assert!(matches!(result, Outcome::NeedsSwitch(1)));
        }
    }
}
#[cfg(windows)]
fn main() {
    if std::env::args().any(|a| a == "--check-usage") {
        match usage::fetch_current() {
            Ok(u) => println!("{}", u.to_cache()),
            Err(e) => {
                eprintln!("Usage check: {e}");
                std::process::exit(1)
            }
        }
        return;
    }
    if std::env::args().any(|a| a == "--render-previews") {
        let folder = std::env::args().nth(2).unwrap_or_else(|| "preview".into());
        if let Err(e) = island::export_previews(std::path::Path::new(&folder)) {
            eprintln!("Preview render: {e}");
            std::process::exit(1)
        }
        return;
    }
    island::run();
}
