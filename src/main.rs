#![cfg_attr(windows, windows_subsystem = "windows")]
#[cfg(windows)]
mod accounts;
#[cfg(windows)]
mod backup;
#[cfg(windows)]
mod codex;
#[cfg(windows)]
mod polling;
#[cfg(windows)]
mod profiles;
#[cfg(windows)]
mod usage;
#[cfg(target_os = "linux")]
fn main() {
    use std::process::{Command, Stdio};
    let bootstrap = r#"import sys,json,types
p=json.load(sys.stdin)
backend=types.ModuleType('backend')
sys.modules['backend']=backend
MUISLAND_HTML=p['html']
MUISLAND_VERSION=p['version']
try:
    exec(p['backend'],backend.__dict__)
    exec(p['app'])
except ImportError:
    print('Muisland needs system Python 3, PyGObject/Cairo, GTK 3, WebKitGTK and cryptography. Install the Linux runtime dependencies in docs/LINUX.md.', file=sys.stderr)
    sys.exit(1)
"#;
    let mut child = match Command::new("/usr/bin/python3")
        .arg("-c")
        .arg(bootstrap)
        .args(std::env::args().skip(1))
        .stdin(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(_) => {
            eprintln!("Install Python 3 and the Linux dependencies listed in README.");
            std::process::exit(1)
        }
    };
    let payload = serde_json::json!({"version":env!("CARGO_PKG_VERSION"),"backend":include_str!("../linux/backend.py"),"html":include_str!("../linux/island.html"),"app":include_str!("../linux/app.py")});
    let result = serde_json::to_writer(child.stdin.take().unwrap(), &payload);
    if result.is_err() {
        let _ = child.kill();
        let _ = child.wait();
        eprintln!("Could not initialize the Linux UI runtime.");
        std::process::exit(1)
    }
    std::process::exit(child.wait().ok().and_then(|s| s.code()).unwrap_or(1));
}
#[cfg(not(any(windows, target_os = "linux")))]
fn main() {
    eprintln!("Muisland supports Windows and Linux.");
    std::process::exit(1);
}

#[cfg(windows)]
mod island {
    use crate::{
        accounts::{self, Settings},
        codex,
        polling::{self, PollState},
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
        fn GetAsyncKeyState(key: i32) -> i16;
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
    thread_local! {static ACTIVE_THEME:std::cell::Cell<usize>=const{std::cell::Cell::new(0)};}
    fn colors() -> [u32; 5] {
        match ACTIVE_THEME.with(|v| v.get()) {
            1 => [0x001A120D, 0x00FAF2ED, 0x00B6A396, 0x00FFC67F, 0x002D2117],
            2 => [0x0014131B, 0x00F0F2FC, 0x00AAA1BB, 0x0096B3FF, 0x00221F30],
            3 => [0x00161616, 0x00F5F5F5, 0x00AAAAAA, 0x00EAE3E1, 0x00272727],
            _ => [0x0018100F, 0x00F9F2F1, 0x00B39F9B, 0x00FFB2C3, 0x002C1E1C],
        }
    }
    fn bg() -> u32 {
        colors()[0]
    }
    fn white() -> u32 {
        colors()[1]
    }
    fn muted() -> u32 {
        colors()[2]
    }
    fn accent() -> u32 {
        colors()[3]
    }
    fn panel_color() -> u32 {
        colors()[4]
    }
    fn theme_name(index: usize) -> &'static str {
        ["Lavender", "Ocean", "Sunset", "Graphite"][index.min(3)]
    }
    const EW: f32 = 420.;
    const EH: f32 = 628.;
    const BODY_OFFSET: i32 = 44;
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
        AutoSwitch(usize),
    }
    #[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
    enum Action {
        None,
        Toggle,
        Preferences,
        ClaudeTab,
        CodexTab,
        Theme(usize),
        MenuSlider,
        FloatSlider,
        BubbleSlider,
        CircleNumbers,
        UsageAlerts,
        SessionThreshold,
        WeeklyThreshold,
        AutoSwitch,
        PauseForWork,
        Back,
        Details(usize),
        Save(usize),
        Switch(usize),
        Add,
        Prev,
        Next,
        Protect,
        Remove,
        RemoveConfirm,
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
        /// Claude reopened signed out locally; the payload is the slot that kept its account.
        SignIn(usize),
        LoginRequired,
    }
    struct SyncResult {
        generation: u64,
        active: Option<usize>,
        samples: Vec<(usize, String, Result<Usage, usage::Failure>)>,
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
        bubble: HWND,
        bubble_alpha: f32,
        w: f32,
        h: f32,
        vw: f32,
        vh: f32,
        scale: f32,
        anchor: f32,
        top: i32,
        float_position: (f32, i32),
        view: View,
        previous: Option<View>,
        transition: f32,
        page_fade: f32,
        reveal: f32,
        hover: Action,
        hover_t: f32,
        hover_levels: std::collections::HashMap<Action, f32>,
        focus: Action,
        down: Option<(i32, i32, Action)>,
        drag: bool,
        slider_drag: Option<Action>,
        slider_preview: Option<f32>,
        prefs_scroll: i32,
        pulse_until: f32,
        toast_until: f32,
        ring_session: Option<f64>,
        ring_weekly: Option<f64>,
        auto_due: Option<f32>,
        alerted: std::collections::HashSet<(usize, usize)>,
        mouse_was_down: bool,
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
        polls: std::collections::HashMap<usize, PollState>,
        generation: u64,
        clock: Instant,
        last: Instant,
        last_second: u64,
        preview: bool,
        network_allowed: bool,
        work_areas: Vec<RECT>,
    }
    unsafe fn available_work_areas() -> Vec<RECT> {
        unsafe extern "system" fn collect(
            monitor: HMONITOR,
            _dc: HDC,
            _rect: *mut RECT,
            data: LPARAM,
        ) -> BOOL {
            let areas = &mut *(data as *mut Vec<RECT>);
            let mut info: MONITORINFO = zeroed();
            info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
            if GetMonitorInfoW(monitor, &mut info) != 0
                && info.rcWork.right > info.rcWork.left
                && info.rcWork.bottom > info.rcWork.top
            {
                areas.push(info.rcWork);
            }
            1
        }
        let mut areas = Vec::new();
        EnumDisplayMonitors(
            null_mut(),
            null(),
            Some(collect),
            (&mut areas as *mut Vec<RECT>) as LPARAM,
        );
        if areas.is_empty() {
            let mut rect: RECT = zeroed();
            if SystemParametersInfoW(SPI_GETWORKAREA, 0, (&mut rect as *mut RECT).cast(), 0) != 0 {
                areas.push(rect);
            }
        }
        areas
    }
    fn bounded_geometry(
        areas: &[RECT],
        anchor: f32,
        top: i32,
        w: f32,
        h: f32,
        scale: f32,
    ) -> (f32, i32, f32) {
        let cy = top as f32 + h * scale / 2.;
        let distance = |r: &RECT| {
            let dx = (r.left as f32 - anchor)
                .max(0.)
                .max(anchor - r.right as f32);
            let dy = (r.top as f32 - cy).max(0.).max(cy - r.bottom as f32);
            dx * dx + dy * dy
        };
        let Some(area) = areas
            .iter()
            .min_by(|a, b| distance(a).total_cmp(&distance(b)))
        else {
            return (anchor, top, scale);
        };
        let margin = 8
            .min((area.right - area.left).max(0) / 4)
            .min((area.bottom - area.top).max(0) / 4);
        let left = area.left + margin;
        let right = area.right - margin;
        let upper = area.top + margin;
        let bottom = area.bottom - margin;
        let fit = scale
            .min((right - left).max(1) as f32 / w.max(1.))
            .min((bottom - upper).max(1) as f32 / h.max(1.));
        let pw = (w * fit).round().max(1.) as i32;
        let ph = (h * fit).round().max(1.) as i32;
        let x = (anchor - pw as f32 / 2.)
            .round()
            .clamp(left as f32, (right - pw).max(left) as f32);
        let y = top.clamp(upper, (bottom - ph).max(upper));
        (x + pw as f32 / 2., y, fit)
    }
    fn constrain(s: &mut State) {
        let (fw, fh) = if s.settings.circle {
            (CIRCLE, CIRCLE)
        } else {
            (PILL_W, PILL_H)
        };
        let (fa, ft, _) = bounded_geometry(
            &s.work_areas,
            s.float_position.0,
            s.float_position.1,
            fw,
            fh,
            s.settings.float_scale,
        );
        s.float_position = (fa, ft);
        // Menu fitting must never overwrite the preferred floating location.
        let (desired_x, desired_y) = if s.open {
            (s.anchor, s.top)
        } else {
            s.float_position
        };
        let (anchor, top, scale) =
            bounded_geometry(&s.work_areas, desired_x, desired_y, s.w, s.h, s.scale);
        s.anchor = anchor;
        s.top = top;
        s.scale = scale;
    }
    fn bubble_motion(progress: f32) -> (f32, f32, f32) {
        let eased = 1. - (1. - progress.clamp(0., 1.)).powi(3);
        (eased, 0.94 + 0.06 * eased, 8. * (1. - eased))
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
    unsafe fn status_paragraph(dc: Dc, label: &str, color: u32) {
        let font = CreateFontW(
            -24,
            0,
            0,
            0,
            400,
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
        let mut bounds = RECT {
            left: 40 * 2,
            top: 570 * 2,
            right: 380 * 2,
            bottom: 606 * 2,
        };
        let mut chars: Vec<u16> = label.encode_utf16().collect();
        DrawTextW(
            dc,
            chars.as_mut_ptr(),
            chars.len() as i32,
            &mut bounds,
            DT_WORDBREAK | DT_NOPREFIX | DT_END_ELLIPSIS,
        );
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
    unsafe fn gear(dc: Dc, cx: f32, cy: f32, angle: f32, color: u32) {
        let points: Vec<POINT> = (0..=32)
            .map(|i| {
                let a = angle + i as f32 * std::f32::consts::TAU / 32.;
                let r = if i % 4 == 1 || i % 4 == 2 { 9. } else { 7. };
                POINT {
                    x: ((cx + r * a.cos()) * 2.).round() as i32,
                    y: ((cy + r * a.sin()) * 2.).round() as i32,
                }
            })
            .collect();
        let pen = CreatePen(PS_SOLID, 2, color);
        let old = SelectObject(dc, pen);
        Polyline(dc, points.as_ptr(), points.len() as i32);
        let brush = SelectObject(dc, GetStockObject(HOLLOW_BRUSH));
        Ellipse(
            dc,
            ((cx - 3.) * 2.) as i32,
            ((cy - 3.) * 2.) as i32,
            ((cx + 3.) * 2.) as i32,
            ((cy + 3.) * 2.) as i32,
        );
        SelectObject(dc, brush);
        SelectObject(dc, old);
        DeleteObject(pen);
    }
    unsafe fn adjustments(dc: Dc, x: i32, y: i32, color: u32) {
        for (row, knob) in [(0, 4), (5, 10), (10, 6)] {
            line(dc, &[(x, y + row), (x + 14, y + row)], 1, color);
            panel(dc, x + knob - 2, y + row - 2, 4, 4, 2, color);
        }
    }
    fn options_motion(previous: Option<&View>, next: &View, progress: f32) -> (i32, i32, f32) {
        let eased = 1. - (1. - progress.clamp(0., 1.)).powi(3);
        let from = previous.is_some_and(|v| matches!(v, View::Preferences));
        let to = matches!(next, View::Preferences);
        let direction = if to && !from {
            1.
        } else if from && !to {
            -1.
        } else {
            0.
        };
        let gear_progress = if to {
            eased
        } else if from {
            1. - eased
        } else {
            0.
        };
        (
            (-12. * eased * direction).round() as i32,
            (20. * (1. - eased) * direction).round() as i32,
            gear_progress,
        )
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
    unsafe fn ring_color(dc: Dc, cx: f32, cy: f32, r: f32, used: Option<f64>, base_color: u32) {
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
            base_color
        };
        for y in ((cy - r - 5.) as i32).max(0)..((cy + r + 5.) as i32).min(height) {
            for x in ((cx - r - 5.) as i32).max(0)..((cx + r + 5.) as i32).min(width) {
                let dx = x as f32 + 0.5 - cx;
                let dy = y as f32 + 0.5 - cy;
                let coverage = (2.5 - (dx.hypot(dy) - r).abs()).clamp(0., 1.);
                let phase = dx.atan2(-dy).rem_euclid(std::f32::consts::TAU) / std::f32::consts::TAU;
                let cap = (2.5 - (x as f32 + 0.5 - endx).hypot(y as f32 + 0.5 - endy))
                    .clamp(0., 1.)
                    .max((2.5 - dx.hypot(dy + r)).clamp(0., 1.));
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
                let color = mix(
                    mix(panel_color(), white(), 0.10),
                    accent,
                    (colored / total).clamp(0., 1.),
                );
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
        add(Action::ClaudeTab, 24, 64, 182, 32);
        add(Action::CodexTab, 214, 64, 182, 32);
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
                if s.settings.provider == 0 {
                    add(Action::Protect, 24, 432, 372, 48);
                }
            }
            View::Details(_) => {
                add(Action::NameField, 24, 126, 372, 44);
                add(Action::Back, 24, 410, 120, 42);
                add(Action::NameSave, 156, 410, 240, 42);
                add(Action::Remove, 24, 356, 372, 36);
            }
            View::Preferences => {
                for (index, y) in [160, 200].into_iter().enumerate() {
                    add(Action::Theme(index * 2), 24, y, 182, 34);
                    add(Action::Theme(index * 2 + 1), 214, y, 182, 34);
                }
                add(Action::MenuSlider, 154, 254, 140, 30);
                add(Action::FloatSlider, 154, 300, 140, 30);
                add(Action::BubbleSlider, 154, 346, 140, 30);
                add(Action::Circle, 24, 396, 372, 56);
                add(Action::Motion, 24, 462, 372, 56);
                add(Action::Rollback, 24, 528, 372, 34);
                if s.settings.provider == 0 {
                    add(Action::Recover, 24, 570, 372, 34);
                }
                add(Action::CircleNumbers, 24, 620, 372, 48);
                add(Action::UsageAlerts, 24, 750, 372, 48);
                add(Action::SessionThreshold, 154, 814, 140, 30);
                add(Action::WeeklyThreshold, 154, 860, 140, 30);
                add(Action::AutoSwitch, 24, 914, 372, 48);
                add(Action::PauseForWork, 24, 972, 372, 48);
                add(Action::Back, 24, 1116, 178, 34);
                add(Action::Quit, 210, 1116, 186, 34);
            }
            View::Remove(_, _) => {
                add(Action::Cancel, 24, 312, 120, 42);
                add(Action::RemoveConfirm, 156, 312, 240, 42);
            }
            View::Notice(_, _) => add(Action::Back, 24, 410, 372, 42),
            View::Confirm(_) | View::SwitchConfirm(_) | View::AutoSwitch(_) => {
                add(Action::Cancel, 24, 312, 120, 42);
                add(Action::Confirm, 156, 312, 240, 42)
            }
        }
        for button in b.iter_mut().skip(5) {
            button.y += BODY_OFFSET
                - if matches!(s.view, View::Preferences) {
                    s.prefs_scroll
                } else {
                    0
                };
            if matches!(s.view, View::Preferences) {
                let bottom = (button.y + button.h).min(584);
                button.y = button.y.max(112);
                button.h = (bottom - button.y).max(0);
            }
        }
        b.retain(|button| button.h > 0);
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
        } else {
            s.hover_levels.get(&a).copied().unwrap_or(0.)
        }
    }
    fn animate_hover(s: &mut State, dt: f32) -> bool {
        if s.hover != Action::None {
            s.hover_levels.entry(s.hover).or_insert(0.);
        }
        let mut changed = false;
        for (action, level) in &mut s.hover_levels {
            let target = if *action == s.hover { 1. } else { 0. };
            let old = *level;
            *level += (target - *level) * (dt * 15.).min(1.);
            if (*level - target).abs() < 0.005 {
                *level = target;
            }
            changed |= old != *level;
        }
        s.hover_levels.retain(|_, level| *level > 0.);
        changed
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
        let y = y - (hot(s, a) * 1.5).round() as i32;
        let pressed = s.down.is_some_and(|(_, _, down)| down == a) && s.hover == a;
        let d = if pressed { 1 } else { 0 };
        let danger = matches!(a, Action::Remove | Action::RemoveConfirm);
        let color = if danger {
            mix(0x00232834, 0x0031374C, hot(s, a))
        } else if primary {
            mix(accent(), white(), hot(s, a) * 0.14)
        } else {
            mix(
                mix(panel_color(), white(), 0.04),
                mix(panel_color(), white(), 0.13),
                hot(s, a),
            )
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
                bg()
            } else {
                white()
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
                if primary && !danger { bg() } else { accent() },
            );
        }
    }

    unsafe fn toggle(dc: Dc, s: &State, a: Action, y: i32, label: &str, value: bool) {
        let tall = matches!(a, Action::Circle | Action::Motion);
        let h = if tall { 56 } else { 48 };
        panel(
            dc,
            24,
            y,
            372,
            h,
            17,
            mix(panel_color(), button_color(), hot(s, a) * 0.5),
        );
        text(
            dc,
            39,
            y + (h - 16) / 2,
            if tall { 12 } else { 13 },
            400,
            label,
            white(),
            284,
        );
        panel(
            dc,
            344,
            y + (h - 20) / 2,
            36,
            20,
            10,
            if value { accent() } else { button_color() },
        );
        panel(
            dc,
            if value { 363 } else { 347 },
            y + (h - 14) / 2,
            14,
            14,
            7,
            if value { bg() } else { white() },
        );
    }
    fn button_color() -> u32 {
        match ACTIVE_THEME.with(|v| v.get()) {
            1 => 0x004B3727,
            2 => 0x00393049,
            3 => 0x00383838,
            _ => 0x003B2B29,
        }
    }
    fn slider_bounds(action: Action) -> Option<(f32, f32)> {
        match action {
            Action::MenuSlider => Some((0.75, 1.6)),
            Action::FloatSlider => Some((0.6, 1.8)),
            Action::BubbleSlider => Some((0.85, 1.6)),
            Action::SessionThreshold | Action::WeeklyThreshold => Some((0.05, 1.0)),
            _ => None,
        }
    }
    fn slider_value(s: &State, action: Action) -> f32 {
        if s.slider_drag == Some(action) {
            if let Some(value) = s.slider_preview {
                return value;
            }
        }
        match action {
            Action::MenuSlider => s.settings.menu_scale,
            Action::FloatSlider => s.settings.float_scale,
            Action::BubbleSlider => s.settings.bubble_scale,
            Action::SessionThreshold => s.settings.session_threshold,
            Action::WeeklyThreshold => s.settings.weekly_threshold,
            _ => 1.,
        }
    }
    fn slider_from_x(action: Action, x: i32) -> f32 {
        let (lo, hi) = slider_bounds(action).unwrap();
        let value = lo + ((x - 164) as f32 / 118.).clamp(0., 1.) * (hi - lo);
        ((value / 0.05).round() * 0.05).clamp(lo, hi)
    }
    fn set_slider(s: &mut State, action: Action, value: f32) {
        let Some((lo, hi)) = slider_bounds(action) else {
            return;
        };
        let value = value.clamp(lo, hi);
        match action {
            Action::MenuSlider => s.settings.menu_scale = value,
            Action::FloatSlider => s.settings.float_scale = value,
            Action::BubbleSlider => s.settings.bubble_scale = value,
            Action::SessionThreshold => {
                s.settings.session_threshold = value;
                s.alerted.clear();
            }
            Action::WeeklyThreshold => {
                s.settings.weekly_threshold = value;
                s.alerted.clear();
            }
            _ => {}
        }
    }
    unsafe fn size_slider(dc: Dc, s: &State, action: Action, cy: i32, label: &str) {
        let (lo, hi) = slider_bounds(action).unwrap();
        let value = slider_value(s, action);
        let position = 164 + (118. * ((value - lo) / (hi - lo)).clamp(0., 1.)).round() as i32;
        text(dc, 24, cy - 7, 12, 400, label, white(), 135);
        panel(dc, 164, cy - 3, 118, 6, 3, button_color());
        panel(dc, 164, cy - 3, (position - 164).max(1), 6, 3, accent());
        let radius = 9 + hot(s, action).round() as i32;
        panel(
            dc,
            position - radius,
            cy - radius,
            radius * 2,
            radius * 2,
            radius,
            mix(0x00D7D4DC, white(), hot(s, action) * 0.5),
        );
        text(
            dc,
            360,
            cy - 6,
            10,
            400,
            &format!("{}%", (value * 100.).round() as i32),
            accent(),
            40,
        );
    }
    unsafe fn theme_card(dc: Dc, s: &State, index: usize, x: i32, y: i32) {
        let selected = s.settings.theme == index;
        let fill = mix(button_color(), white(), hot(s, Action::Theme(index)) * 0.06);
        panel(
            dc,
            x,
            y,
            182,
            34,
            12,
            if selected {
                mix(fill, accent(), 0.75)
            } else {
                fill
            },
        );
        if selected {
            panel(dc, x + 1, y + 1, 180, 32, 11, fill);
        }
        let dot = [0x00FFB2C3, 0x00FFC67F, 0x0096B3FF, 0x00EAE3E1][index];
        panel(dc, x + 9, y + 13, 9, 9, 4, dot);
        text(dc, x + 26, y + 10, 11, 500, theme_name(index), white(), 146);
    }
    unsafe fn quota(dc: Dc, x: i32, y: i32, width: i32, label: &str, window: Option<&Window>) {
        let now = accounts::now();
        let title = window
            .map(|w| format!("{label} · {:.0}% used", w.used))
            .unwrap_or_else(|| format!("{label} · —"));
        text(dc, x, y, 11, 600, &title, white(), width);
        panel(
            dc,
            x,
            y + 18,
            width,
            3,
            1,
            mix(panel_color(), white(), 0.10),
        );
        if let Some(w) = window {
            let color = if w.resets.is_some_and(|t| t <= now) {
                muted()
            } else if w.used >= 90. {
                0x009BA7F0
            } else if w.used >= 70. {
                0x009FC5F2
            } else {
                accent()
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
            muted(),
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
                text(dc, 24, 76, 24, 600, "Your accounts.", white(), 370);
                text(
                    dc,
                    25,
                    108,
                    12,
                    400,
                    "Live limits. A seamless switch.",
                    muted(),
                    370,
                );
                if s.settings.accounts.is_empty() {
                    panel(dc, 16, 126, 388, 238, 20, panel_color());
                    panel(dc, 185, 165, 50, 50, 25, mix(panel_color(), accent(), 0.18));
                    mark(dc, 201, 181, 18, accent());
                    text(
                        dc,
                        49,
                        239,
                        21,
                        600,
                        "Your island starts here.",
                        white(),
                        335,
                    );
                    text(
                        dc,
                        36,
                        276,
                        12,
                        400,
                        "Add an account, then save your Claude sign-in.",
                        muted(),
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
                        mix(panel_color(), 0x00433330, hot(s, Action::Switch(i)) * 0.8),
                    );
                    panel(
                        dc,
                        28,
                        y + 14,
                        34,
                        34,
                        17,
                        if s.active == Some(i) {
                            mix(panel_color(), accent(), 0.25)
                        } else {
                            mix(panel_color(), white(), 0.08)
                        },
                    );
                    text(dc, 39, y + 20, 15, 600, &(i + 1).to_string(), accent(), 25);
                    text(dc, 75, y + 12, 15, 600, &account.name, white(), 213);
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
                    text(dc, 75, y + 33, 10, 400, &sub, muted(), 213);
                    button(dc, s, Action::Details(i), 296, y + 15, 28, 30, "", false);
                    adjustments(
                        dc,
                        303,
                        y + 25,
                        mix(muted(), white(), hot(s, Action::Details(i))),
                    );
                    button(dc, s, Action::Save(i), 329, y + 15, 60, 30, "Save", false);
                    quota(
                        dc,
                        28,
                        y + 59,
                        164,
                        if s.settings.provider == 1 {
                            "Session"
                        } else {
                            "5h"
                        },
                        account.usage.as_ref().and_then(|u| u.session.as_ref()),
                    );
                    quota(
                        dc,
                        216,
                        y + 59,
                        176,
                        if s.settings.provider == 1 {
                            "Extended"
                        } else {
                            "7d"
                        },
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
                    muted(),
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
                if s.settings.provider == 0 {
                    toggle(
                        dc,
                        s,
                        Action::Protect,
                        432,
                        "Back up local data before switch",
                        s.settings.protect_local,
                    );
                } else {
                    text(
                        dc,
                        24,
                        450,
                        11,
                        400,
                        "Codex switches sign-ins; open sessions keep their work.",
                        muted(),
                        372,
                    );
                }
            }
            View::Details(i) => {
                if let Some(a) = s.settings.accounts.get(*i) {
                    text(dc, 24, 78, 24, 600, "Account details", white(), 370);
                    text(
                        dc,
                        25,
                        108,
                        12,
                        400,
                        "Name · up to 32 characters",
                        muted(),
                        370,
                    );
                    panel(dc, 24, 126, 372, 44, 12, 0x00352A27);
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
                            0x008E7068,
                        );
                    }
                    text(dc, 35, 137, 17, 400, &label, white(), 345);
                    if s.edit_focus && ((s.clock.elapsed().as_millis() / 500) % 2 == 0) {
                        let cx = 35 + (s.caret as i32 * 8).min(339);
                        panel(dc, cx, 138, 1, 22, 0, accent());
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
                        white(),
                        365,
                    );
                    quota(
                        dc,
                        25,
                        229,
                        165,
                        if s.settings.provider == 1 {
                            "Session"
                        } else {
                            "5h"
                        },
                        a.usage.as_ref().and_then(|u| u.session.as_ref()),
                    );
                    quota(
                        dc,
                        216,
                        229,
                        176,
                        if s.settings.provider == 1 {
                            "Extended"
                        } else {
                            "7d"
                        },
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
                            if s.settings.provider == 1 {
                                "Updates through Codex app-server using file credentials."
                            } else {
                                "Updates automatically from your Claude session."
                            }
                        },
                        muted(),
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
                        if a.draft {
                            "Discard draft"
                        } else {
                            "Remove account"
                        },
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
                        if a.draft { "Save account" } else { "Save name" },
                        true,
                    );
                }
            }
            View::Preferences => {
                text(dc, 24, 88, 26, 600, "Make it yours.", white(), 372);
                text(
                    dc,
                    24,
                    123,
                    12,
                    400,
                    "Independent sizes · scroll for usage alerts",
                    muted(),
                    372,
                );
                for index in 0..4 {
                    theme_card(
                        dc,
                        s,
                        index,
                        24 + (index % 2) as i32 * 190,
                        160 + (index / 2) as i32 * 40,
                    );
                }
                size_slider(dc, s, Action::MenuSlider, 269, "Menu size");
                size_slider(dc, s, Action::FloatSlider, 315, "Floating size");
                size_slider(dc, s, Action::BubbleSlider, 361, "Hover bubble size");
                toggle(
                    dc,
                    s,
                    Action::Circle,
                    396,
                    "Circle with two usage rings",
                    s.settings.circle,
                );
                toggle(
                    dc,
                    s,
                    Action::Motion,
                    462,
                    "Reduce motion",
                    s.settings.reduced_motion,
                );
                button(
                    dc,
                    s,
                    Action::Rollback,
                    24,
                    528,
                    372,
                    34,
                    "Restore previous sign-in",
                    false,
                );
                if s.settings.provider == 0 {
                    button(
                        dc,
                        s,
                        Action::Recover,
                        24,
                        570,
                        372,
                        34,
                        "Recover local backup to folder",
                        false,
                    );
                }

                toggle(
                    dc,
                    s,
                    Action::CircleNumbers,
                    620,
                    "Show percentage instead of M",
                    s.settings.circle_numbers,
                );
                text(dc, 40, 692, 18, 600, "Alerts & auto-switch", white(), 372);
                text(
                    dc,
                    40,
                    720,
                    11,
                    400,
                    "Choose a limit for each usage window.",
                    muted(),
                    340,
                );
                toggle(
                    dc,
                    s,
                    Action::UsageAlerts,
                    750,
                    "Notify at usage thresholds",
                    s.settings.usage_alerts,
                );
                size_slider(dc, s, Action::SessionThreshold, 829, "Session limit");
                size_slider(dc, s, Action::WeeklyThreshold, 875, "Weekly limit");
                toggle(
                    dc,
                    s,
                    Action::AutoSwitch,
                    914,
                    "Auto-switch at these limits",
                    s.settings.auto_switch,
                );
                toggle(
                    dc,
                    s,
                    Action::PauseForWork,
                    972,
                    "Ask first when work may be open",
                    s.settings.pause_for_work,
                );
                paragraph(dc,25,1034,"Auto-switch waits 15 seconds. Cancel or close the Island to stop it. Open apps may have running tasks; idle detection is not guaranteed.",muted());
                button(dc, s, Action::Back, 24, 1116, 178, 34, "Back", false);
                button(dc, s, Action::Quit, 210, 1116, 186, 34, "Quit", false);
            }
            View::AutoSwitch(i) => {
                text(dc, 24, 89, 24, 600, "Usage limit reached", white(), 372);
                let label = s
                    .settings
                    .accounts
                    .get(*i)
                    .map(|a| a.name.as_str())
                    .unwrap_or("another account");
                paragraph(dc,25,142,&format!("Switch to {label}? Claude may close and running projects or unsaved work may stop. Muisland cannot reliably tell whether a task is idle. Codex sessions need a restart to use the new sign-in."),muted());
                text(
                    dc,
                    25,
                    270,
                    14,
                    600,
                    &s.auto_due
                        .map(|due| {
                            format!(
                                "Switching in {} seconds",
                                (due - s.clock.elapsed().as_secs_f32()).ceil().max(0.) as i32
                            )
                        })
                        .unwrap_or_else(|| "App may be working · confirmation required".into()),
                    accent(),
                    372,
                );
                button(dc, s, Action::Cancel, 24, 312, 120, 42, "Cancel", false);
                button(
                    dc,
                    s,
                    Action::Confirm,
                    156,
                    312,
                    240,
                    42,
                    "Switch now",
                    true,
                );
            }
            View::Remove(_, name) => {
                text(dc, 24, 89, 24, 600, "Remove this account?", white(), 372);
                paragraph(dc,25,142,&format!("Remove {name} from Muisland? The current sign-in, chats and files remain unchanged. The saved sign-in is moved to an encrypted local archive."),muted());
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
                panel(dc, 25, 80, 34, 34, 17, 0x004A3734);
                text(dc, 37, 84, 20, 600, "i", accent(), 20);
                text(dc, 24, 132, 23, 600, title, white(), 372);
                paragraph(
                    dc,
                    25,
                    180,
                    body,
                    if s.error { 0x009BA7F0 } else { muted() },
                );
                button(dc, s, Action::Back, 24, 410, 372, 42, "Got it", true)
            }
            View::SwitchConfirm(index) => {
                text(dc, 24, 89, 24, 600, "Switch account?", white(), 372);
                text(
                    dc,
                    25,
                    136,
                    16,
                    600,
                    &s.settings.accounts[*index].name,
                    accent(),
                    365,
                );
                paragraph(
                    dc,
                    25,
                    180,
                    if s.settings.provider == 1 {
                        "Switch the Codex file sign-in? New CLI sessions use this account. Restart open sessions to reload. The Desktop host may keep its own login."
                    } else {
                        "Confirm to open this saved account. If Claude is running, it will close first. Local tasks may stop and unsaved work may be lost."
                    },
                    muted(),
                );
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
                // Another slot already holds the signed-in account: this Save adds a new one.
                let adding = s.settings.provider == 0
                    && matches!(op, Op::Save(i) if s.active.is_some_and(|active| {
                        active != *i && s.identities.get(active) != s.identities.get(*i)
                    }));
                text(
                    dc,
                    24,
                    89,
                    24,
                    600,
                    if adding {
                        "Add another account?"
                    } else if saving {
                        "Close Claude & save?"
                    } else {
                        "Close Claude & switch?"
                    },
                    white(),
                    372,
                );
                paragraph(
                    dc,
                    25,
                    142,
                    if s.settings.provider == 1 {
                        "Switch the Codex sign-in cache? New Codex CLI sessions use this account. Restart existing sessions to reload it; the Desktop app may retain its own host login."
                    } else if adding {
                        "Claude's current account is already saved. Claude will close and reopen at its sign-in screen so you can add another one. The saved account is not signed out."
                    } else {
                        "Claude Desktop will close. Local tasks may stop and unsaved work may be lost. Nothing closes until you confirm."
                    },
                    muted(),
                );
                if let Op::Save(i) | Op::Switch(i) = op {
                    text(
                        dc,
                        25,
                        248,
                        12,
                        600,
                        &format!("Account: {}", s.settings.accounts[*i].name),
                        white(),
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
                    if adding {
                        "Close & sign in"
                    } else if saving {
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
            let brush = CreateSolidBrush(bg());
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
        ACTIVE_THEME.with(|v| v.set(s.settings.theme));
        let lw = s.w.round().max(1.) as i32;
        let lh = s.h.round().max(1.) as i32;
        let source = Surface::new(lw * 2, lh * 2);
        if source.bits.is_null() {
            return None;
        }
        source.clear();
        if !s.open && s.settings.circle {
            let cx = s.w / 2.;
            let cy = s.h / 2.;
            let account = s.active.and_then(|i| s.settings.accounts.get(i));
            let u = account.and_then(|a| a.usage.as_ref());
            let used = u.and_then(|u| u.session.as_ref()).map(|w| w.used);
            let weekly = s
                .active
                .and_then(|i| s.settings.accounts.get(i))
                .and_then(|a| a.usage.as_ref())
                .and_then(|u| u.weekly.as_ref())
                .map(|w| w.used);
            let pulse =
                if !s.settings.reduced_motion && s.pulse_until > s.clock.elapsed().as_secs_f32() {
                    (s.clock.elapsed().as_secs_f32() * 8.).sin().abs() * 0.4
                } else {
                    0.
                };
            ring_color(
                source.dc,
                cx,
                cy,
                30.,
                weekly.map(|v| s.ring_weekly.unwrap_or(v)),
                mix(0x00E8C17F, white(), pulse),
            );
            ring_color(
                source.dc,
                cx,
                cy,
                23.,
                used.map(|v| s.ring_session.unwrap_or(v)),
                mix(accent(), white(), pulse),
            );
            if s.settings.circle_numbers {
                let label = used
                    .map(|v| format!("{v:.0}%"))
                    .unwrap_or_else(|| "—".into());
                text(
                    source.dc,
                    cx as i32 - 16,
                    cy as i32 - 8,
                    13,
                    600,
                    &label,
                    white(),
                    37,
                );
            } else {
                mark(source.dc, cx as i32 - 8, cy as i32 - 8, 16, white());
            }
        } else {
            panel(
                source.dc,
                16,
                14,
                30,
                30,
                15,
                mix(panel_color(), accent(), 0.18),
            );
            mark(source.dc, 24, 22, 14, accent());
            text(source.dc, 57, 17, 18, 600, "Muisland", white(), 170);
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
                let (_, _, rotation) = options_motion(s.previous.as_ref(), &s.view, s.transition);
                panel(
                    source.dc,
                    w - 88,
                    12,
                    32,
                    34,
                    12,
                    mix(panel_color(), 0x004B3938, hot(s, Action::Preferences) * 0.8),
                );
                gear(
                    source.dc,
                    (w - 72) as f32,
                    29.,
                    rotation * std::f32::consts::FRAC_PI_4,
                    mix(muted(), accent(), rotation.max(hot(s, Action::Preferences))),
                );
            }
            if s.open && lh > 100 {
                button(
                    source.dc,
                    s,
                    Action::ClaudeTab,
                    24,
                    64,
                    182,
                    32,
                    "Claude",
                    s.settings.provider == 0,
                );
                button(
                    source.dc,
                    s,
                    Action::CodexTab,
                    214,
                    64,
                    182,
                    32,
                    "Codex",
                    s.settings.provider == 1,
                );
            }
            if s.reveal > 0.01 && lh > 70 {
                let layer = Surface::new(lw * 2, lh * 2);
                layer.clear();
                let motion = 1. - (1. - s.transition).powi(3);
                let (outgoing, incoming, _) =
                    options_motion(s.previous.as_ref(), &s.view, s.transition);
                if let Some(previous) = &s.previous {
                    let saved = SaveDC(source.dc);
                    IntersectClipRect(source.dc, 0, (58 + BODY_OFFSET) * 2, lw * 2, lh * 2);
                    if matches!(previous, View::Preferences) {
                        IntersectClipRect(source.dc, 0, 112 * 2, lw * 2, 584 * 2);
                    }
                    SetViewportOrgEx(
                        source.dc,
                        outgoing * 2,
                        (BODY_OFFSET
                            - if matches!(previous, View::Preferences) {
                                s.prefs_scroll
                            } else {
                                0
                            })
                            * 2,
                        null_mut(),
                    );
                    draw_view(source.dc, s, previous);
                    RestoreDC(source.dc, saved);
                }
                let saved = SaveDC(layer.dc);
                if matches!(s.view, View::Preferences) {
                    IntersectClipRect(layer.dc, 0, 112 * 2, lw * 2, 584 * 2);
                }
                SetViewportOrgEx(
                    layer.dc,
                    incoming * 2,
                    (BODY_OFFSET
                        - if matches!(s.view, View::Preferences) {
                            s.prefs_scroll
                        } else {
                            0
                        })
                        * 2
                        + ((1. - s.reveal) * 12.) as i32 * 2,
                    null_mut(),
                );
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
                    (58 + BODY_OFFSET) * 2,
                    lw * 2,
                    (lh - 58 - BODY_OFFSET) * 2,
                    layer.dc,
                    0,
                    (58 + BODY_OFFSET) * 2,
                    lw * 2,
                    (lh - 58 - BODY_OFFSET) * 2,
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
                            if s.settings.provider == 1 {
                                "Codex · shared file login · 30s usage polling"
                            } else {
                                "Claude · 30s active / 2m other accounts"
                            }
                        }
                    };
                    if matches!(s.view, View::Home) {
                        panel(
                            source.dc,
                            24,
                            534,
                            372,
                            78,
                            16,
                            mix(bg(), panel_color(), 0.65),
                        );
                        let foreground = if s.error { 0x009BA7F0 } else { white() };
                        let provider = if s.settings.provider == 1 {
                            "Codex"
                        } else {
                            "Claude"
                        };
                        let title = if s.error {
                            "Needs attention"
                        } else if s.worker.is_some() {
                            "Updating your account…"
                        } else if s.sync.is_some() {
                            "Checking account status…"
                        } else if !s.status.is_empty() {
                            "Account status"
                        } else if s.preview {
                            "Preview · sample data"
                        } else if !s.network_allowed {
                            "Usage updates paused"
                        } else {
                            "Automatic usage updates"
                        };
                        text(source.dc, 40, 546, 13, 600, title, foreground, 276);
                        text(
                            source.dc,
                            336,
                            548,
                            11,
                            400,
                            &format!("v{}", env!("CARGO_PKG_VERSION")),
                            muted(),
                            48,
                        );
                        if !s.status.is_empty() || s.worker.is_some() || s.sync.is_some() {
                            status_paragraph(
                                source.dc,
                                status,
                                if s.error {
                                    foreground
                                } else {
                                    mix(muted(), white(), 0.30)
                                },
                            );
                        } else if s.preview || !s.network_allowed {
                            status_paragraph(
                                source.dc,
                                if s.preview {
                                    "Illustrative values only. No accounts are connected."
                                } else {
                                    "Network access is disabled for this test session."
                                },
                                mix(muted(), white(), 0.30),
                            );
                        } else {
                            text(
                                source.dc,
                                40,
                                570,
                                12,
                                400,
                                &format!("{} · active account every 30 seconds", provider),
                                mix(muted(), white(), 0.30),
                                340,
                            );
                            text(
                                source.dc,
                                40,
                                589,
                                12,
                                400,
                                "Other saved accounts every 2 minutes",
                                mix(muted(), white(), 0.30),
                                340,
                            );
                        }
                    } else {
                        text(
                            source.dc,
                            25,
                            EH as i32 - 30,
                            11,
                            400,
                            &format!(
                                "Muisland {} · {}",
                                env!("CARGO_PKG_VERSION"),
                                if matches!(s.view, View::Preferences) && s.status.is_empty() {
                                    if s.preview {
                                        "Preview · sample data"
                                    } else if s.settings.provider == 1 {
                                        "Codex · automatic usage"
                                    } else {
                                        "Claude · automatic usage"
                                    }
                                } else {
                                    status
                                }
                            ),
                            if s.error { 0x009BA7F0 } else { muted() },
                            370,
                        );
                    }
                }
            }
        }
        let radius = if !s.open && s.settings.circle {
            s.w.min(s.h) / 2.
        } else {
            29. + s.reveal * 5.
        };
        compose(&source, s.w, s.h, s.scale, |x, y| {
            rounded_alpha(x, y, 0., 0., s.w, s.h, radius)
        })
    }
    fn rounded_alpha(x: f32, y: f32, left: f32, top: f32, w: f32, h: f32, r: f32) -> f32 {
        let qx = (x - left - w / 2.).abs() - (w / 2. - r);
        let qy = (y - top - h / 2.).abs() - (h / 2. - r);
        (0.5 - (qx.max(0.).hypot(qy.max(0.)) + qx.max(qy).min(0.) - r)).clamp(0., 1.)
    }
    unsafe fn compose(
        source: &Surface,
        w: f32,
        h: f32,
        scale: f32,
        mask: impl Fn(f32, f32) -> f32,
    ) -> Option<Surface> {
        GdiFlush();
        let pw = (w * scale).round().max(1.) as i32;
        let ph = (h * scale).round().max(1.) as i32;
        let output = Surface::new(pw, ph);
        if output.bits.is_null() {
            return None;
        }
        let src = std::slice::from_raw_parts(source.bits, (source.w * source.h * 4) as usize);
        let dest = std::slice::from_raw_parts_mut(output.bits, (pw * ph * 4) as usize);
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
                let alpha = mask((x as f32 + 0.5) / scale, (y as f32 + 0.5) / scale);
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
    const BUBBLE_OPEN: u32 = WM_APP + 42;
    const BUBBLE_W: f32 = 306.;
    const BUBBLE_H: f32 = 188.;
    #[derive(Debug)]
    struct BubblePlacement {
        x: i32,
        y: i32,
        scale: f32,
        right: bool,
        tail: f32,
    }
    fn bubble_placement(
        areas: &[RECT],
        cx: f32,
        top: i32,
        diameter: f32,
        requested: f32,
    ) -> Option<BubblePlacement> {
        let cy = top as f32 + diameter / 2.;
        let area = areas.iter().min_by(|a, b| {
            let dist = |r: &RECT| {
                (cx - cx.clamp(r.left as f32, r.right as f32)).powi(2)
                    + (cy - cy.clamp(r.top as f32, r.bottom as f32)).powi(2)
            };
            dist(a).total_cmp(&dist(b))
        })?;
        let left = area.left as f32 + 8.;
        let right = area.right as f32 - 8.;
        let upper = area.top as f32 + 8.;
        let bottom = area.bottom as f32 - 8.;
        let gap = 8.;
        let room_right = (right - cx - diameter / 2. - gap).max(1.);
        let room_left = (cx - diameter / 2. - gap - left).max(1.);
        let preferred_right = cx < (left + right) / 2.;
        let right_side = if preferred_right {
            room_right >= BUBBLE_W * requested || room_right >= room_left
        } else {
            !(room_left >= BUBBLE_W * requested || room_left >= room_right)
        };
        let scale = requested
            .min(if right_side { room_right } else { room_left } / BUBBLE_W)
            .min((bottom - upper).max(1.) / BUBBLE_H)
            .max(0.01);
        let w = (BUBBLE_W * scale).round();
        let h = (BUBBLE_H * scale).round();
        let x = if right_side {
            cx + diameter / 2. + gap
        } else {
            cx - diameter / 2. - gap - w
        };
        let y = (cy - h / 2.).clamp(upper, (bottom - h).max(upper));
        Some(BubblePlacement {
            x: x.round() as i32,
            y: y.round() as i32,
            scale,
            right: right_side,
            tail: ((cy - y) / scale).clamp(30., BUBBLE_H - 30.),
        })
    }
    unsafe fn render_bubble(s: &State, scale: f32, right: bool, tail: f32) -> Option<Surface> {
        ACTIVE_THEME.with(|v| v.set(s.settings.theme));
        let height = BUBBLE_H;
        let source = Surface::new(BUBBLE_W as i32 * 2, height as i32 * 2);
        if source.bits.is_null() {
            return None;
        }
        let brush = CreateSolidBrush(panel_color());
        FillRect(
            source.dc,
            &RECT {
                left: 0,
                top: 0,
                right: source.w,
                bottom: source.h,
            },
            brush,
        );
        DeleteObject(brush);
        let inset = if right { 10 } else { 0 };
        let account = s.active.and_then(|i| s.settings.accounts.get(i));
        let u = account.and_then(|a| a.usage.as_ref());
        text(
            source.dc,
            inset + 20,
            17,
            17,
            600,
            account
                .map(|a| a.name.as_str())
                .unwrap_or("No linked account"),
            white(),
            252,
        );
        let caption = u
            .map(|u| {
                format!(
                    "{} · {} · {}",
                    if s.settings.provider == 1 {
                        "Codex"
                    } else {
                        "Claude"
                    },
                    u.plan,
                    if s.preview {
                        "Sample usage"
                    } else {
                        "Live usage"
                    }
                )
            })
            .unwrap_or_else(|| "Usage not available yet".into());
        text(source.dc, inset + 20, 43, 10, 400, &caption, muted(), 252);
        line(
            source.dc,
            &[(inset + 20, 65), (inset + 276, 65)],
            1,
            0x00433732,
        );
        quota(
            source.dc,
            inset + 20,
            77,
            256,
            if s.settings.provider == 1 {
                "Session"
            } else {
                "5h"
            },
            u.and_then(|u| u.session.as_ref()),
        );
        quota(
            source.dc,
            inset + 20,
            131,
            256,
            if s.settings.provider == 1 {
                "Extended"
            } else {
                "7d"
            },
            u.and_then(|u| u.weekly.as_ref()),
        );
        compose(&source, BUBBLE_W, height, scale, |x, y| {
            let body = rounded_alpha(x, y, inset as f32, 0., 296., height, 22.);
            let depth = if right { x } else { BUBBLE_W - x };
            // Symmetric triangle overlaps the body to avoid a hairline seam.
            let triangle = ((depth - (y - tail.min(height - 22.)).abs()) / 2f32.sqrt() + 0.5)
                .clamp(0., 1.)
                * (12.5 - depth).clamp(0., 1.);
            body.max(triangle)
        })
    }
    unsafe fn paint_bubble(s: &State) {
        if s.bubble.is_null() {
            return;
        }
        if s.open || s.drag || s.down.is_some() || !s.settings.circle || s.bubble_alpha <= 0.01 {
            ShowWindow(s.bubble, SW_HIDE);
            return;
        }
        let Some(p) = bubble_placement(
            &s.work_areas,
            s.anchor,
            s.top,
            s.w * s.scale,
            s.settings.bubble_scale,
        ) else {
            return;
        };
        let (opacity, zoom, slide) = bubble_motion(s.bubble_alpha);
        if let Some(output) = render_bubble(s, p.scale * zoom, p.right, p.tail) {
            let slide = (slide * p.scale).round() as i32;
            let x = if p.right {
                p.x + slide
            } else {
                p.x + (BUBBLE_W * p.scale).round() as i32 - output.w - slide
            };
            let y = p.y + ((BUBBLE_H * p.scale).round() as i32 - output.h) / 2;
            UpdateLayeredWindow(
                s.bubble,
                null_mut(),
                &POINT { x, y },
                &SIZE {
                    cx: output.w,
                    cy: output.h,
                },
                output.dc,
                &POINT { x: 0, y: 0 },
                0,
                &BLENDFUNCTION {
                    BlendOp: AC_SRC_OVER as u8,
                    BlendFlags: 0,
                    SourceConstantAlpha: (opacity * 255.) as u8,
                    AlphaFormat: AC_SRC_ALPHA as u8,
                },
                ULW_ALPHA,
            );
            ShowWindow(s.bubble, SW_SHOWNOACTIVATE);
        }
    }
    unsafe extern "system" fn bubble_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
        match msg {
            WM_MOUSEACTIVATE => MA_NOACTIVATE as LRESULT,
            WM_LBUTTONUP => {
                let owner = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as HWND;
                if !owner.is_null() {
                    PostMessageW(owner, BUBBLE_OPEN, 0, 0);
                }
                0
            }
            WM_SETCURSOR => {
                SetCursor(LoadCursorW(null_mut(), IDC_HAND));
                1
            }
            WM_ERASEBKGND => 1,
            _ => DefWindowProcW(hwnd, msg, wp, lp),
        }
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
        paint_bubble(s);
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
            "circle-hover-left",
            "three-accounts",
            "small-size",
            "usage-settings",
            "usage-updates",
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
                "usage-settings"=>{s.view=View::Preferences;s.prefs_scroll=550;},
                "remove-account"=>s.view=View::Remove(0,"Personal".into()),
                "account-name"=>{s.view=View::Details(0);s.edit="حساب الشغل".encode_utf16().collect();s.caret=s.edit.len();s.edit_focus=true;s.selected=true;},
                "already-logged-in"=>s.view=View::Notice("Already logged in".into(),"Personal is the account currently logged into Claude. No switch was needed and Claude was left untouched.".into()),
                "circle"=>{s.open=false;s.settings.circle=true;s.w=CIRCLE;s.h=CIRCLE;},
                "circle-hover" | "circle-hover-left"=>{s.open=false;s.settings.circle=true;s.w=CIRCLE;s.h=CIRCLE;s.peek=true;},
                "three-accounts"=>{s.settings.add_account();refresh_slots(&mut s);s.page=1;},
                "small-size"=>{s.page=0;s.scale=0.75;},
                // Export is synchronous: these flags only select the painted state,
                // and do not start timers, read credentials, or request usage.
                "usage-updates"=>{s.page=0;s.preview=false;s.network_allowed=true;},
                _=>{},
            }
            let surface = unsafe {
                if name.starts_with("circle-hover") {
                    let right = name == "circle-hover";
                    let circle = render_surface(&s).unwrap();
                    let card = render_bubble(&s, 1., right, 94.).unwrap();
                    let board = Surface::new(390, 188);
                    std::ptr::write_bytes(board.bits, 0, (board.w * board.h * 4) as usize);
                    for (image, x, y) in [
                        (&circle, if right { 0 } else { 314 }, 56),
                        (&card, if right { 84 } else { 0 }, 0),
                    ] {
                        for row in 0..image.h {
                            std::ptr::copy_nonoverlapping(
                                image.bits.add((row * image.w * 4) as usize),
                                board.bits.add(((row + y) * board.w * 4 + x * 4) as usize),
                                (image.w * 4) as usize,
                            );
                        }
                    }
                    Some(board)
                } else {
                    render_surface(&s)
                }
            }
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
    fn discard_drafts(s: &mut State) {
        // Worker outcomes carry account indices. Keep them stable until completion.
        if s.worker.is_some() || !s.settings.accounts.iter().any(|a| a.draft) {
            return;
        }
        let active_slot = s
            .active
            .and_then(|i| s.settings.accounts.get(i))
            .map(|a| a.slot);
        s.settings.accounts.retain(|a| !a.draft);
        s.active =
            active_slot.and_then(|slot| s.settings.accounts.iter().position(|a| a.slot == slot));
        s.generation += 1;
        s.sync = None;
        s.sync_next = 0.;
        s.polls
            .retain(|slot, _| s.settings.accounts.iter().any(|a| a.slot == *slot));
        refresh_slots(s);
        s.page = s
            .page
            .min(s.settings.accounts.len().div_ceil(2).saturating_sub(1));
        s.view = View::Home;
        s.previous = None;
        s.edit_focus = false;
        s.focus = Action::None;
    }
    fn collapse(s: &mut State) {
        s.auto_due = None;
        s.open = false;
        discard_drafts(s);
        s.peek = false;
        s.peek_until = 0.;
        s.edit_focus = false;
        s.view = View::Home;
        s.previous = None;
        s.transition = 1.;
        s.focus = Action::None;
    }
    fn drag_zone(s: &State, x: i32, y: i32) -> bool {
        !s.open || (y < 58 && x < EW as i32 - 100)
    }
    fn navigate(s: &mut State, next: View, _backwards: bool) {
        if !matches!(next, View::AutoSwitch(_)) {
            s.auto_due = None;
        }
        if matches!(next, View::Preferences) && !matches!(s.view, View::Preferences) {
            s.prefs_scroll = 0;
        }
        s.slider_drag = None;
        s.slider_preview = None;
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
                if s.settings.accounts[i].draft {
                    false
                } else if s.preview {
                    true
                } else {
                    if s.settings.provider == 1 {
                        codex::saved(s.settings.accounts[i].slot)
                    } else {
                        profiles::saved(s.settings.accounts[i].slot)
                    }
                }
            })
            .collect();
        s.identities = (0..s.settings.accounts.len())
            .map(|i| {
                if s.settings.accounts[i].draft {
                    None
                } else if s.preview {
                    Some(format!("preview-{}", s.settings.accounts[i].slot))
                } else {
                    if s.settings.provider == 1 {
                        codex::account_identity(s.settings.accounts[i].slot).ok()
                    } else {
                        profiles::account_identity(s.settings.accounts[i].slot).ok()
                    }
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
        s.sync_next = now + polling::IDENTITY_SECONDS;
        let network_allowed = s.network_allowed;
        let provider = s.settings.provider;
        let polls = s.polls.clone();
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
                if provider == 1 {
                    codex::current_identity().ok()
                } else {
                    usage::current_account_identity().ok()
                }
            };
            let active = current.as_ref().and_then(|id| {
                slots
                    .iter()
                    .position(|(identity, _)| identity.as_ref() == Some(id))
            });
            let mut samples = Vec::new();
            if network_allowed && !preview {
                let active_slot = active.and_then(|i| slots.get(i).map(|(_, slot)| *slot));
                for (id, slot) in slots {
                    if let Some(id) = id {
                        if polls
                            .get(&slot)
                            .cloned()
                            .unwrap_or_default()
                            .due(now, active_slot == Some(slot))
                        {
                            samples.push((
                                slot,
                                id,
                                (if provider == 1 {
                                    codex::fetch(slot)
                                } else {
                                    usage::fetch(slot)
                                })
                                .map_err(usage::Failure::from),
                            ));
                        }
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
        let expected_source = if matches!(s.view, View::AutoSwitch(_)) {
            s.active
                .and_then(|i| s.identities.get(i))
                .and_then(|id| id.clone())
        } else {
            None
        };
        let protect = s.settings.protect_local;
        let provider = s.settings.provider;
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
                if let Some(expected) = expected_source.as_ref() {
                    let current = if provider == 1 {
                        codex::current_identity().ok()
                    } else {
                        usage::current_account_identity().ok()
                    };
                    if !preview && current.as_ref() != Some(expected) {
                        return Err("Signed-in account changed. Auto-switch cancelled.".into());
                    }
                }
                if let Op::Switch(i) = op {
                    if (preview && active == Some(i))
                        || (!preview
                            && if provider == 1 {
                                codex::is_current(slot)
                            } else {
                                profiles::is_current(slot).unwrap_or(false)
                            })
                    {
                        return Ok(Outcome::Already(i));
                    }
                    if !confirmed {
                        return Ok(Outcome::NeedsSwitch(i));
                    }
                }
                if provider == 0
                    && !confirmed
                    && (matches!(op, Op::Rollback)
                        || (matches!(op, Op::Save(_) | Op::Switch(_))
                            && (preview || profiles::claude_running())))
                {
                    return Ok(Outcome::NeedsClose(op));
                }
                if preview {
                    return Ok(Outcome::Done(
                        op,
                        "Preview complete · no account changes".into(),
                    ));
                }
                if provider == 1 {
                    match op {
                        Op::Save(_)=>codex::save(slot),Op::Switch(_)=>codex::switch(slot),Op::Rollback=>codex::rollback(),
                        Op::Recover=>Err(std::io::Error::other("Codex switches only its sign-in cache. Chats and working folders are not moved or restored."))
                    }.map_err(|e|e.to_string())?;
                    return Ok(Outcome::Done(
                        op,
                        if matches!(op, Op::Switch(_)) {
                            "Codex sign-in switched · restart existing Codex sessions to reload"
                        } else {
                            "Codex sign-in saved"
                        }
                        .into(),
                    ));
                }
                let launch = profiles::claude_launch_target();
                let expected = if matches!(op, Op::Save(_)) {
                    Some(match usage::current_account_identity() {
                        Ok(identity) => identity,
                        Err(_) => {
                            launch.open().map_err(|_| {
                                "Open Claude Desktop, sign in, then choose Save again.".to_string()
                            })?;
                            return Ok(Outcome::LoginRequired);
                        }
                    })
                } else {
                    None
                };
                if confirmed {
                    profiles::close_claude_for_save().map_err(|e| e.to_string())?;
                }
                // Saving into another slot while this account is already saved means
                // the user wants to add a different account, not a duplicate.
                // Re-saving a slot that already holds this account stays a plain save.
                if let Some(other) = expected
                    .as_deref()
                    .filter(|identity| {
                        profiles::account_identity(slot).ok().as_deref() != Some(*identity)
                    })
                    .and_then(|identity| profiles::slot_holding(identity, slot))
                {
                    let prepared =
                        profiles::sign_in_another(other, expected.as_deref().unwrap_or(""));
                    let opened = launch.open();
                    prepared.map_err(|e| e.to_string())?;
                    opened.map_err(|_| {
                        "Claude is ready for another sign-in but could not reopen. Open Claude manually."
                            .to_string()
                    })?;
                    return Ok(Outcome::SignIn(other));
                }
                match op {
                    Op::Save(_) => {
                        profiles::save_and_reopen(slot, expected.as_deref().unwrap_or(""), &launch)
                            .map_err(|e| e.to_string())?;
                        Ok(Outcome::Done(op, "Account saved · Claude reopened".into()))
                    }
                    Op::Switch(_) => {
                        let switched = profiles::switch(slot, protect);
                        let reopened = launch.open();
                        switched.map_err(|e| e.to_string())?;
                        reopened.map_err(|_|"Account switched, but Claude could not reopen. Open Claude manually.".to_string())?;
                        Ok(Outcome::Done(op, "Switched successfully".into()))
                    }
                    Op::Rollback => {
                        let restored = profiles::restore_backup();
                        let reopened = launch.open();
                        restored.map_err(|e| e.to_string())?;
                        reopened.map_err(|_| "Previous sign-in restored, but Claude could not reopen. Open Claude manually.".to_string())?;
                        Ok(Outcome::Done(
                            op,
                            "Previous sign-in restored · Claude reopened".into(),
                        ))
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
    fn threshold_hit(s: &State, i: usize) -> bool {
        s.settings
            .accounts
            .get(i)
            .and_then(|a| a.usage.as_ref())
            .is_some_and(|u| {
                accounts::now().saturating_sub(u.updated) <= 90
                    && (u.session.as_ref().is_some_and(|w| {
                        w.resets.is_none_or(|r| r > accounts::now())
                            && w.used >= (s.settings.session_threshold * 100.).round() as f64
                    }) || u.weekly.as_ref().is_some_and(|w| {
                        w.resets.is_none_or(|r| r > accounts::now())
                            && w.used >= (s.settings.weekly_threshold * 100.).round() as f64
                    }))
            })
    }
    fn auto_target(s: &State) -> Option<usize> {
        let active = s.active?;
        if !threshold_hit(s, active) || !s.usage_errors.get(active).is_some_and(|e| e.is_empty()) {
            return None;
        }
        s.settings
            .accounts
            .iter()
            .enumerate()
            .filter(|(i, a)| {
                *i != active
                    && !a.draft
                    && s.saved.get(*i) == Some(&true)
                    && s.usage_errors.get(*i).is_some_and(|e| e.is_empty())
                    && s.identities
                        .get(*i)
                        .and_then(|v| v.as_ref())
                        .is_some_and(|id| {
                            Some(id) != s.identities.get(active).and_then(|v| v.as_ref())
                        })
            })
            .filter_map(|(i, a)| {
                let u = a.usage.as_ref()?;
                let x = u.session.as_ref()?;
                let y = u.weekly.as_ref()?;
                if accounts::now().saturating_sub(u.updated) > 300
                    || x.resets.is_some_and(|r| r <= accounts::now())
                    || y.resets.is_some_and(|r| r <= accounts::now())
                    || x.used >= (s.settings.session_threshold * 100.).round() as f64
                    || y.used >= (s.settings.weekly_threshold * 100.).round() as f64
                {
                    return None;
                }
                Some((i, x.used.max(y.used)))
            })
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|v| v.0)
    }
    fn evaluate_limits(s: &mut State) {
        if (!s.settings.usage_alerts && !s.settings.auto_switch)
            || s.worker.is_some()
            || !matches!(s.view, View::Home)
        {
            return;
        }
        let Some(i) = s.active else {
            return;
        };
        let key = (s.settings.provider, s.settings.accounts[i].slot);
        if !threshold_hit(s, i) {
            s.alerted.remove(&key);
            return;
        }
        if !s.usage_errors.get(i).is_some_and(|e| e.is_empty()) || !s.alerted.insert(key) {
            return;
        }
        s.pulse_until = s.clock.elapsed().as_secs_f32() + 2.;
        if s.settings.auto_switch {
            if let Some(target) = auto_target(s) {
                let may_work =
                    s.settings.provider == 1 || (!s.preview && profiles::claude_running());
                s.auto_due = if s.settings.pause_for_work && may_work {
                    None
                } else {
                    Some(s.clock.elapsed().as_secs_f32() + 15.)
                };
                navigate(s, View::AutoSwitch(target), false);
                s.focus = Action::Cancel;
                return;
            }
        }
        notice(
            s,
            "Usage alert",
            format!(
                "{} reached a configured usage threshold. {}",
                s.settings.accounts[i].name,
                if s.settings.auto_switch {
                    "No saved account has fresh, lower usage in both windows. No switch was made."
                } else {
                    "Open accounts to review usage or switch."
                }
            ),
            false,
        );
    }
    fn name_save(s: &mut State) {
        if let View::Details(i) = s.view {
            match accounts::valid_name(&String::from_utf16_lossy(&s.edit)) {
                Ok(name) => {
                    if s.settings.accounts[i].draft {
                        s.settings.accounts[i].name = name;
                        navigate(s, View::Home, true);
                        begin(s, Op::Save(i), false);
                        return;
                    }
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
        let archive = if s.preview || s.settings.accounts[index].draft {
            None
        } else {
            match if s.settings.provider == 1 {
                codex::archive(slot)
            } else {
                profiles::archive_account(slot)
            } {
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
                let _ = if s.settings.provider == 1 {
                    codex::restore_archive(slot, &path)
                } else {
                    profiles::restore_archived_account(slot, &path)
                };
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
        s.polls
            .retain(|slot, _| s.settings.accounts.iter().any(|a| a.slot == *slot));
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
        if !matches!(a, Action::Confirm) {
            s.auto_due = None;
        }
        match a {
            Action::None => {}
            Action::ClaudeTab | Action::CodexTab => {
                if s.worker.is_some() {
                    status(
                        s,
                        "Wait for the current operation before changing apps",
                        false,
                    );
                } else {
                    discard_drafts(s);
                    let provider = if a == Action::CodexTab { 1 } else { 0 };
                    s.settings.select_provider(provider);
                    s.generation += 1;
                    s.sync = None;
                    s.sync_next = 0.;
                    s.polls.clear();
                    s.active = None;
                    s.page = 0;
                    refresh_slots(s);
                    persist(s);
                    navigate(s, View::Home, false);
                    // A prior confirmation may refer to another provider's slot.
                    s.previous = None;
                    s.hover_levels.clear();
                }
            }
            Action::MenuSlider
            | Action::FloatSlider
            | Action::BubbleSlider
            | Action::SessionThreshold
            | Action::WeeklyThreshold => {}
            Action::CircleNumbers => {
                s.settings.circle_numbers = !s.settings.circle_numbers;
                persist(s);
            }
            Action::UsageAlerts => {
                s.settings.usage_alerts = !s.settings.usage_alerts;
                s.alerted.clear();
                persist(s);
            }
            Action::AutoSwitch => {
                s.settings.auto_switch = !s.settings.auto_switch;
                s.alerted.clear();
                persist(s);
            }
            Action::PauseForWork => {
                s.settings.pause_for_work = !s.settings.pause_for_work;
                s.auto_due = None;
                persist(s);
            }

            Action::Theme(index) => {
                s.settings.theme = index.min(3);
                persist(s);
            }

            Action::Toggle => {
                if s.open {
                    collapse(s)
                } else {
                    s.open = true;
                    s.peek = false;
                    s.edit_focus = false;
                }
            }
            Action::Preferences => {
                if matches!(s.view, View::Preferences) {
                    navigate(s, View::Home, true)
                } else {
                    navigate(s, View::Preferences, false)
                }
            }
            Action::Back | Action::Cancel => {
                discard_drafts(s);
                navigate(s, View::Home, true)
            }
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
                    View::SwitchConfirm(i) | View::AutoSwitch(i) => Some(Op::Switch(i)),
                    _ => None,
                };
                if let Some(op) = operation {
                    begin(s, op, true);
                    navigate(s, View::Home, true);
                }
            }
            Action::Add => {
                if let Some(i) = s.settings.add_account() {
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
            if !(*ptr).bubble.is_null() {
                DestroyWindow((*ptr).bubble);
            }
            drop(Box::from_raw(ptr));
            PostQuitMessage(0);
            return 0;
        }
        let s = &mut *ptr;
        match msg {
            BUBBLE_OPEN => {
                activate(hwnd, s, Action::Toggle);
                SetForegroundWindow(hwnd);
                InvalidateRect(hwnd, null(), 0);
                0
            }
            WM_ACTIVATE if (wp as u32 & 0xFFFF) == WA_INACTIVE => {
                if s.open || s.peek {
                    collapse(s);
                    InvalidateRect(hwnd, null_mut(), 0);
                }
                0
            }
            WM_DISPLAYCHANGE | WM_SETTINGCHANGE => {
                s.work_areas = available_work_areas();
                constrain(s);
                InvalidateRect(hwnd, null_mut(), 0);
                0
            }
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
                if let (Some(due), View::AutoSwitch(i)) = (s.auto_due, s.view.clone()) {
                    if t >= due && s.open && s.worker.is_none() {
                        s.auto_due = None;
                        let valid = auto_target(s) == Some(i);
                        if valid {
                            begin(s, Op::Switch(i), true);
                            navigate(s, View::Home, true);
                        } else {
                            notice(s,"Auto-switch paused","Usage or account state changed. Review the accounts and switch manually.",false);
                        }
                    }
                }
                let before_position = (s.anchor, s.top);
                if !s.open && GetAsyncKeyState(0x01) < 0 {
                    if let Some((dx, dy, _)) = s.down {
                        if drag_zone(s, dx, dy) {
                            let mut pointer = zeroed();
                            GetCursorPos(&mut pointer);
                            let anchor = (pointer.x - s.origin.0) as f32 + s.w * s.scale / 2.;
                            let top = pointer.y - s.origin.1;
                            if s.drag || (anchor - s.anchor).abs() > 4. || (top - s.top).abs() > 4 {
                                s.drag = true;
                                s.anchor = anchor;
                                s.top = top;
                                s.float_position = (anchor, top);
                                constrain(s);
                            }
                        }
                    }
                }
                let before = (
                    s.w,
                    s.h,
                    s.scale,
                    s.reveal,
                    s.transition,
                    s.hover,
                    s.hover_t,
                    s.peek,
                    s.bubble_alpha,
                );
                let mut cursor = zeroed();
                GetCursorPos(&mut cursor);
                let on_bubble = !s.bubble.is_null() && WindowFromPoint(cursor) == s.bubble;
                let mouse_down = [0x01, 0x02, 0x04]
                    .iter()
                    .any(|key| GetAsyncKeyState(*key) < 0);
                if mouse_down
                    && !s.mouse_was_down
                    && s.down.is_none()
                    && s.slider_drag.is_none()
                    && !s.drag
                    && (s.open || s.peek)
                {
                    let target = WindowFromPoint(cursor);
                    if target != hwnd && target != s.bubble && IsChild(hwnd, target) == 0 {
                        collapse(s);
                        InvalidateRect(hwnd, null_mut(), 0);
                    }
                }
                s.mouse_was_down = mouse_down;
                ScreenToClient(hwnd, &mut cursor);
                let x = cursor.x as f32 / s.scale;
                let y = cursor.y as f32 / s.scale;
                let inside = x >= 0. && x < s.w && y >= 0. && y < s.h;
                if !s.open && s.settings.circle && s.down.is_none() && !s.drag {
                    if on_bubble
                        || (inside && ((x - 38.).powi(2) + (y - 38.).powi(2) < 38f32.powi(2)))
                    {
                        if !s.peek {}
                        s.peek = true;
                        s.peek_until = t + 0.3;
                    } else if t > s.peek_until {
                        s.peek = false;
                    }
                }
                let (tw, th) = if s.open {
                    (EW, EH)
                } else if s.settings.circle {
                    (CIRCLE, CIRCLE)
                } else {
                    (PILL_W, PILL_H)
                };
                if s.open || !s.settings.circle || s.drag || s.down.is_some() {
                    s.peek = false;
                }
                let old_rings = (s.ring_session, s.ring_weekly);
                let usage = s
                    .active
                    .and_then(|i| s.settings.accounts.get(i))
                    .and_then(|a| a.usage.as_ref());
                for (display, target) in [
                    (
                        &mut s.ring_session,
                        usage.and_then(|u| u.session.as_ref()).map(|w| w.used),
                    ),
                    (
                        &mut s.ring_weekly,
                        usage.and_then(|u| u.weekly.as_ref()).map(|w| w.used),
                    ),
                ] {
                    *display = target.map(|target| {
                        if s.settings.reduced_motion {
                            target
                        } else {
                            let old = display.unwrap_or(target);
                            if (target - old).abs() < 0.05 {
                                target
                            } else {
                                old + (target - old) * (dt as f64 * 8.).min(1.)
                            }
                        }
                    });
                }
                let bubble_target = if (s.peek || s.toast_until > t)
                    && (s.w - CIRCLE).abs() < 0.1
                    && (s.h - CIRCLE).abs() < 0.1
                {
                    1.
                } else {
                    0.
                };
                s.bubble_alpha = if s.settings.reduced_motion {
                    bubble_target
                } else {
                    if bubble_target > s.bubble_alpha {
                        (s.bubble_alpha + dt / 0.24).min(1.)
                    } else {
                        (s.bubble_alpha - dt / 0.18).max(0.)
                    }
                };
                if (s.bubble_alpha - bubble_target).abs() < 0.005 {
                    s.bubble_alpha = bubble_target;
                }
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
                    s.transition = (s.transition + dt * 5.5).min(1.);
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
                let hover_changed = animate_hover(s, dt);
                constrain(s);
                let mut changed = old_rings != (s.ring_session, s.ring_weekly)
                    || s.toast_until > t
                    || s.pulse_until > t
                    || s.peek
                    || matches!(s.view, View::AutoSwitch(_))
                    || hover_changed
                    || old_page_fade != s.page_fade
                    || before_position != (s.anchor, s.top);
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
                                s.polls
                                    .entry(slot)
                                    .or_default()
                                    .success(t, s.active == Some(i));
                                s.settings.accounts[i].usage = Some(u);
                                s.usage_errors[i].clear();
                                measurements = true
                            }
                            Err(e) => {
                                s.polls.entry(slot).or_default().failure(
                                    t,
                                    s.active == Some(i),
                                    e.retry_after,
                                    e.auth_required,
                                );
                                s.usage_errors[i] = e.message;
                            }
                        }
                    }
                    if measurements {
                        persist(s);
                        if result.generation == s.generation {
                            evaluate_limits(s);
                        }
                    }
                    changed = true;
                }
                if let Some(result) = s.worker.as_ref().and_then(|r| r.try_recv().ok()) {
                    s.worker = None;
                    s.pending = None;
                    match result {
                        Ok(Outcome::Already(i)) => {
                            s.active = Some(i);
                            let visible = s.open;
                            notice(
                                s,
                                "Already logged in",
                                format!(
                                    "{} is already logged into {}. No sign-in was changed.",
                                    s.settings.accounts[i].name,
                                    if s.settings.provider == 1 {
                                        "Codex"
                                    } else {
                                        "Claude"
                                    }
                                ),
                                false,
                            );
                            if !visible {
                                s.open = false;
                                s.previous = None;
                                s.transition = 1.;
                            }
                        }
                        Ok(Outcome::NeedsSwitch(i)) => {
                            s.status.clear();
                            if s.open {
                                navigate(s, View::SwitchConfirm(i), false);
                                s.focus = Action::Cancel;
                            }
                        }
                        Ok(Outcome::NeedsClose(op)) => {
                            s.status.clear();
                            if s.open {
                                navigate(s, View::Confirm(op), false);
                                s.focus = Action::Cancel;
                            }
                        }
                        Ok(Outcome::LoginRequired) => {
                            notice(s, "Sign in to Claude", "Claude opened. Complete sign-in there, then choose Save again. Your existing saved accounts were kept.", false);
                        }
                        Ok(Outcome::SignIn(slot)) => {
                            let name = s
                                .settings
                                .accounts
                                .iter()
                                .find(|a| a.slot == slot)
                                .map(|a| a.name.clone())
                                .unwrap_or_else(|| "Your account".into());
                            refresh_slots(s);
                            s.sync_next = 0.;
                            let visible = s.open;
                            notice(
                                s,
                                "Sign in to the other account",
                                format!("{name} is already saved and was not signed out. Claude reopened at its sign-in screen: sign in to the other account there, then choose Save again. Avoid Claude's own Log out; it ends a saved session."),
                                false,
                            );
                            if !visible {
                                s.open = false;
                                s.previous = None;
                                s.transition = 1.;
                            }
                        }
                        Ok(Outcome::Done(op, label)) => {
                            if matches!(op, Op::Switch(_)) {
                                s.toast_until = t + 2.;
                            }
                            let visible = s.open;
                            if let Op::Save(i) = op {
                                s.settings.accounts[i].draft = false;
                                s.settings.accounts[i].usage = None;
                                s.usage_errors[i].clear();
                                persist(s);
                            }
                            if let Op::Switch(i) = op {
                                s.active = Some(i);
                            }
                            refresh_slots(s);
                            if let Op::Save(i) | Op::Switch(i) = op {
                                s.polls.remove(&s.settings.accounts[i].slot);
                            }
                            s.sync_next = 0.;
                            navigate(s, View::Home, true);
                            if !visible {
                                s.open = false;
                                s.previous = None;
                                s.transition = 1.;
                            }
                            status(s, label, false);
                        }
                        Err(e) => {
                            let visible = s.open;
                            notice(s, "Couldn't finish this action", e, true);
                            if !visible {
                                s.open = false;
                                s.previous = None;
                                s.transition = 1.;
                            }
                        }
                    }
                    if !s.open {
                        discard_drafts(s);
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
                        s.bubble_alpha,
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
                if slider_bounds(action).is_some() {
                    s.slider_drag = Some(action);
                    s.slider_preview = Some(slider_from_x(action, x));
                    s.focus = action;
                    SetCapture(hwnd);
                    InvalidateRect(hwnd, null_mut(), 0);
                    return 0;
                }
                s.down = Some((x, y, action));
                s.drag = false;
                s.focus = Action::None;
                if drag_zone(s, x, y) {
                    let mut p = zeroed();
                    GetCursorPos(&mut p);
                    s.origin = (p.x - (s.anchor - s.w * s.scale / 2.) as i32, p.y - s.top);
                    SetCapture(hwnd);
                }
                InvalidateRect(hwnd, null_mut(), 0);
                0
            }
            WM_MOUSEMOVE => {
                if let Some(action) = s.slider_drag {
                    let (x, _) = point(lp, s.scale);
                    s.slider_preview = Some(slider_from_x(action, x));
                    InvalidateRect(hwnd, null_mut(), 0);
                    return 0;
                }
                if let Some((dx, dy, _)) = s.down {
                    let (x, y) = point(lp, s.scale);
                    if drag_zone(s, dx, dy) && ((x - dx).abs() > 4 || (y - dy).abs() > 4 || s.drag)
                    {
                        s.drag = true;
                        let mut p = zeroed();
                        GetCursorPos(&mut p);
                        s.anchor = (p.x - s.origin.0) as f32 + s.w * s.scale / 2.;
                        s.top = p.y - s.origin.1;
                        s.float_position = (s.anchor, s.top);
                        constrain(s);
                        InvalidateRect(hwnd, null_mut(), 0);
                    }
                }
                0
            }
            WM_LBUTTONUP => {
                if let Some(action) = s.slider_drag.take() {
                    let (x, _) = point(lp, s.scale);
                    set_slider(s, action, slider_from_x(action, x));
                    s.slider_preview = None;
                    ReleaseCapture();
                    persist(s);
                    InvalidateRect(hwnd, null_mut(), 0);
                    return 0;
                }
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
                s.slider_drag = None;
                s.slider_preview = None;
                s.down = None;
                s.drag = false;
                0
            }
            WM_MOUSEWHEEL => {
                if s.open && matches!(s.view, View::Preferences) && s.slider_drag.is_none() {
                    let delta = ((wp >> 16) as u16) as i16;
                    s.prefs_scroll =
                        (s.prefs_scroll - (delta as f32 / 120. * 40.).round() as i32).clamp(0, 610);
                    InvalidateRect(hwnd, null_mut(), 0);
                }
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
                if matches!(s.view, View::Preferences) && [0x21, 0x22].contains(&key) {
                    s.prefs_scroll =
                        (s.prefs_scroll + if key == 0x22 { 100 } else { -100 }).clamp(0, 610);
                } else if matches!(s.view, View::Preferences)
                    && slider_bounds(s.focus).is_some()
                    && [0x25, 0x27, 0x24, 0x23].contains(&key)
                {
                    let action = s.focus;
                    let (lo, hi) = slider_bounds(action).unwrap();
                    let value = match key {
                        0x24 => lo,
                        0x23 => hi,
                        0x25 => slider_value(s, action) - 0.05,
                        _ => slider_value(s, action) + 0.05,
                    };
                    set_slider(s, action, value);
                    persist(s);
                } else if s.edit_focus && control && key == 0x41 {
                    s.selected = true;
                } else if s.edit_focus && control && key == 0x56 {
                    paste(hwnd, s);
                } else if key == 0x1B {
                    if s.open && !matches!(s.view, View::Home) {
                        discard_drafts(s);
                        navigate(s, View::Home, true)
                    } else {
                        collapse(s);
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
            settings.other_accounts = settings.accounts.clone();
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
        if preview {
            settings.other_accounts = settings.accounts.clone();
            for (i, a) in settings.other_accounts.iter_mut().enumerate() {
                a.name = if i == 0 {
                    "Codex Personal"
                } else {
                    "Development"
                }
                .into();
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
            bubble: null_mut(),
            bubble_alpha: 0.,
            w: if circle { CIRCLE } else { PILL_W },
            h: if circle { CIRCLE } else { PILL_H },
            vw: 0.,
            vh: 0.,
            scale,
            anchor,
            top,
            float_position: (anchor, top),
            view: View::Home,
            previous: None,
            transition: 1.,
            page_fade: 1.,
            reveal: 0.,
            hover: Action::None,
            hover_t: 0.,
            hover_levels: std::collections::HashMap::new(),
            focus: Action::None,
            down: None,
            drag: false,
            slider_drag: None,
            slider_preview: None,
            prefs_scroll: 0,
            pulse_until: 0.,
            toast_until: 0.,
            ring_session: None,
            ring_weekly: None,
            auto_due: None,
            alerted: Default::default(),
            mouse_was_down: false,
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
            polls: std::collections::HashMap::new(),
            generation: 0,
            clock: now,
            last: now,
            last_second: accounts::now(),
            preview,
            network_allowed: !preview && !std::env::args().any(|a| a == "--no-network"),
            work_areas: Vec::new(),
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
            let mut s = Box::new(initial(
                preview,
                (area.left + area.right) as f32 / 2.,
                area.top + 20,
            ));
            s.work_areas = available_work_areas();
            constrain(&mut s);
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
            let bubble_class = wide("MuislandHoverWindow");
            RegisterClassW(&WNDCLASSW {
                lpfnWndProc: Some(bubble_proc),
                hInstance: instance,
                hCursor: LoadCursorW(null_mut(), IDC_HAND),
                lpszClassName: bubble_class.as_ptr(),
                ..zeroed()
            });
            s.bubble = CreateWindowExW(
                WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_LAYERED | WS_EX_NOACTIVATE,
                bubble_class.as_ptr(),
                wide("Muisland account usage").as_ptr(),
                WS_POPUP,
                0,
                0,
                1,
                1,
                hwnd,
                null_mut(),
                instance,
                null(),
            );
            SetWindowLongPtrW(s.bubble, GWLP_USERDATA, hwnd as isize);
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
        fn opening_and_closing_menu_preserves_floating_position_at_edges() {
            let mut s = initial(true, 46., 8);
            s.settings.circle = true;
            s.settings.float_scale = 1.;
            s.work_areas = vec![RECT {
                left: 0,
                top: 0,
                right: 1920,
                bottom: 1040,
            }];
            for home in [(46., 8), (1874., 8), (46., 956), (1874., 956)] {
                s.float_position = home;
                s.anchor = home.0;
                s.top = home.1;
                s.scale = 1.;
                s.open = true;
                s.w = EW;
                s.h = EH;
                constrain(&mut s);
                assert_eq!(s.float_position, home);
                assert_ne!((s.anchor, s.top), home);
                collapse(&mut s);
                // Clamping while the window is still large must not lose home.
                constrain(&mut s);
                assert_eq!(s.float_position, home);
                s.w = CIRCLE;
                s.h = CIRCLE;
                constrain(&mut s);
                assert_eq!((s.anchor, s.top), home);
                // Repeated cycles must not accumulate a position shift.
                s.open = true;
                s.w = EW;
                s.h = EH;
                constrain(&mut s);
                collapse(&mut s);
                s.w = CIRCLE;
                s.h = CIRCLE;
                constrain(&mut s);
                assert_eq!((s.anchor, s.top), home);
            }
        }
        #[test]
        fn hover_animation_fades_scales_and_slides_to_exact_final_geometry() {
            let start = bubble_motion(0.);
            let mid = bubble_motion(0.5);
            let end = bubble_motion(1.);
            assert_eq!(start, (0., 0.94, 8.));
            assert_eq!(end, (1., 1., 0.));
            assert!(mid.0 > start.0 && mid.0 < end.0);
            assert!(mid.1 > start.1 && mid.1 < end.1);
            assert!(mid.2 < start.2 && mid.2 > end.2);
        }
        #[test]
        fn hover_bubble_fits_all_corners_without_moving_circle() {
            for area in [
                RECT {
                    left: 0,
                    top: 0,
                    right: 1920,
                    bottom: 1040,
                },
                RECT {
                    left: -1280,
                    top: -300,
                    right: 0,
                    bottom: 720,
                },
            ] {
                for diameter in [46., 76., 137.] {
                    for is_right in [false, true] {
                        for bottom in [false, true] {
                            let cx = if is_right {
                                area.right as f32 - 8. - diameter / 2.
                            } else {
                                area.left as f32 + 8. + diameter / 2.
                            };
                            let top = if bottom {
                                area.bottom - 8 - diameter as i32
                            } else {
                                area.top + 8
                            };
                            let p = bubble_placement(&[area], cx, top, diameter, 1.8).unwrap();
                            assert_eq!(p.right, !is_right);
                            assert!(p.x >= area.left + 7 && p.y >= area.top + 7);
                            assert!(p.x + (BUBBLE_W * p.scale).round() as i32 <= area.right - 7);
                            assert!(p.y + (BUBBLE_H * p.scale).round() as i32 <= area.bottom - 7);
                            if p.right {
                                assert!(p.x as f32 > cx + diameter / 2.);
                            } else {
                                assert!((p.x as f32 + BUBBLE_W * p.scale) < cx - diameter / 2.);
                            }
                        }
                    }
                }
            }
        }
        #[test]
        fn hover_does_not_change_circle_render_dimensions() {
            let mut s = initial(true, 0., 0);
            s.open = false;
            s.settings.circle = true;
            s.w = CIRCLE;
            s.h = CIRCLE;
            for peek in [false, true] {
                s.peek = peek;
                let image = unsafe { render_surface(&s) }.unwrap();
                assert_eq!((image.w, image.h), (76, 76));
            }
        }
        #[test]
        fn bubble_has_transparent_corners_and_tail_on_correct_side() {
            let s = initial(true, 0., 0);
            for right in [false, true] {
                let image = unsafe { render_bubble(&s, 1., right, 94.) }.unwrap();
                let pixels = unsafe {
                    std::slice::from_raw_parts(image.bits, (image.w * image.h * 4) as usize)
                };
                let alpha = |x: i32, y: i32| pixels[((y * image.w + x) * 4 + 3) as usize];
                assert_eq!(alpha(0, 0), 0);
                let edge = if right { 3 } else { image.w - 4 };
                assert!(alpha(edge, 94) > 200);
                assert_eq!(alpha(edge, 40), 0);
            }
        }
        #[test]
        fn closing_discards_new_unsaved_accounts_and_keeps_existing_slots() {
            let mut s = initial(true, 0., 0);
            let i = s.settings.add_account().unwrap();
            refresh_slots(&mut s);
            s.page = i / 2;
            assert!(!s.saved[i]);
            assert!(s.identities[i].is_none());
            collapse(&mut s);
            assert_eq!(s.settings.accounts.len(), 2);
            assert_eq!(s.page, 0);
            assert_eq!(s.active, Some(0));
            assert_eq!(
                s.settings
                    .accounts
                    .iter()
                    .map(|a| a.slot)
                    .collect::<Vec<_>>(),
                vec![1, 2]
            );
        }
        #[test]
        fn draft_cleanup_waits_for_inflight_save_then_preserves_success() {
            let mut s = initial(true, 0., 0);
            let i = s.settings.add_account().unwrap();
            refresh_slots(&mut s);
            let (_tx, rx) = mpsc::channel();
            s.worker = Some(rx);
            collapse(&mut s);
            assert_eq!(s.settings.accounts.len(), 3);
            s.worker = None;
            s.settings.accounts[i].draft = false;
            discard_drafts(&mut s);
            assert_eq!(s.settings.accounts.len(), 3);
        }
        #[test]
        fn dismissed_or_failed_draft_save_is_removed_when_worker_finishes() {
            let mut s = initial(true, 0., 0);
            s.settings.add_account();
            let (_tx, rx) = mpsc::channel();
            s.worker = Some(rx);
            collapse(&mut s);
            s.worker = None;
            discard_drafts(&mut s);
            assert_eq!(s.settings.accounts.len(), 2);
        }
        #[test]
        fn provider_tabs_and_theme_choices_have_separate_hit_regions() {
            let mut s = initial(true, 0., 0);
            s.reveal = 1.;
            s.transition = 1.;
            assert_eq!(hit(&s, 110, 80), Action::ClaudeTab);
            assert_eq!(hit(&s, 300, 80), Action::CodexTab);
            s.view = View::Preferences;
            assert_eq!(hit(&s, 300, 220), Action::Theme(1));
            assert_eq!(hit(&s, 100, 260), Action::Theme(2));
            s.settings.select_provider(1);
            s.view = View::Home;
            assert!(!buttons(&s).iter().any(|b| b.action == Action::Protect));
        }
        #[test]
        fn button_hover_eases_in_and_out_instead_of_snapping() {
            let mut s = initial(true, 0., 0);
            s.hover = Action::Preferences;
            assert!(animate_hover(&mut s, 0.016));
            let entered = hot(&s, Action::Preferences);
            assert!(entered > 0. && entered < 1.);
            s.hover = Action::None;
            assert!(animate_hover(&mut s, 0.016));
            let leaving = hot(&s, Action::Preferences);
            assert!(leaving > 0. && leaving < entered);
        }
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
            assert_eq!(hit(&s, 80, 194), Action::Switch(4));
            assert_eq!(hit(&s, 350, 194), Action::Save(4));
            assert_eq!(hit(&s, 307, 318), Action::Details(5));
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
        fn automatic_switch_requires_fresh_complete_lower_usage_and_distinct_identity() {
            let mut s = initial(true, 0., 0);
            s.active = Some(0);
            refresh_slots(&mut s);
            s.settings.accounts[0]
                .usage
                .as_mut()
                .unwrap()
                .session
                .as_mut()
                .unwrap()
                .used = 90.;
            s.settings.accounts[1]
                .usage
                .as_mut()
                .unwrap()
                .session
                .as_mut()
                .unwrap()
                .used = 20.;
            assert_eq!(auto_target(&s), Some(1));
            s.settings.accounts[1].usage.as_mut().unwrap().updated = accounts::now() - 301;
            assert_eq!(auto_target(&s), None);
            s.settings.accounts[1].usage.as_mut().unwrap().updated = accounts::now();
            s.settings.accounts[1].usage.as_mut().unwrap().weekly = None;
            assert_eq!(auto_target(&s), None);
        }
        #[test]
        fn alert_is_once_per_crossing_and_collapse_cancels_countdown() {
            let mut s = initial(true, 0., 0);
            s.active = Some(0);
            refresh_slots(&mut s);
            s.settings.usage_alerts = true;
            s.settings.accounts[0]
                .usage
                .as_mut()
                .unwrap()
                .session
                .as_mut()
                .unwrap()
                .used = 80.;
            assert!(
                threshold_hit(&s, 0),
                "Exact configured percentage must trigger"
            );
            s.settings.accounts[0]
                .usage
                .as_mut()
                .unwrap()
                .session
                .as_mut()
                .unwrap()
                .used = 90.;
            evaluate_limits(&mut s);
            assert!(matches!(s.view, View::Notice(..)));
            navigate(&mut s, View::Home, true);
            evaluate_limits(&mut s);
            assert!(matches!(s.view, View::Home));
            s.auto_due = Some(10.);
            collapse(&mut s);
            assert!(s.auto_due.is_none());
        }
        #[test]
        fn settings_sliders_clamp_and_do_not_change_other_sizes() {
            let mut s = initial(true, 0., 0);
            assert!((slider_from_x(Action::MenuSlider, -100) - 0.75).abs() < 0.001);
            assert!((slider_from_x(Action::MenuSlider, 1000) - 1.6).abs() < 0.001);
            set_slider(&mut s, Action::FloatSlider, 0.7);
            assert!((s.settings.float_scale - 0.7).abs() < 0.001);
            assert_eq!(s.settings.menu_scale, 1.0);
            assert_eq!(s.settings.bubble_scale, 1.0);
            s.slider_drag = Some(Action::MenuSlider);
            s.slider_preview = Some(1.4);
            assert_eq!(slider_value(&s, Action::MenuSlider), 1.4);
            assert_eq!(s.settings.menu_scale, 1.0);
        }
        #[test]
        fn settings_scroll_reveals_lower_actions_without_covering_header() {
            let mut s = initial(true, 0., 0);
            s.view = View::Preferences;
            s.reveal = 1.;
            s.transition = 1.;
            assert!(!buttons(&s).iter().any(|b| b.action == Action::Quit));
            s.prefs_scroll = 610;
            assert!(buttons(&s).iter().any(|b| b.action == Action::Quit));
            assert_eq!(hit(&s, 110, 80), Action::ClaudeTab);
            for button in buttons(&s).iter().skip(5) {
                assert!(button.y >= 112 && button.y + button.h <= 584);
            }
        }
        #[test]
        fn preference_size_controls_are_independent() {
            let mut s = initial(true, 0., 0);
            s.view = View::Preferences;
            s.reveal = 1.;
            s.transition = 1.;
            assert_eq!(hit(&s, 220, 313), Action::MenuSlider);
            assert_eq!(hit(&s, 220, 359), Action::FloatSlider);
            assert_eq!(hit(&s, 220, 405), Action::BubbleSlider);
            s.settings.menu_scale = 1.4;
            s.settings.float_scale = 0.6;
            assert_eq!(s.settings.bubble_scale, 1.0);
            let area = RECT {
                left: 0,
                top: 0,
                right: 1920,
                bottom: 1080,
            };
            for diameter in [46., 76., 137.] {
                let p = bubble_placement(&[area], 400., 200, diameter, s.settings.bubble_scale)
                    .unwrap();
                assert_eq!(p.scale, 1.0);
            }
        }
        #[test]
        fn options_enter_and_exit_move_in_opposite_directions_and_settle() {
            let enter = options_motion(Some(&View::Home), &View::Preferences, 0.);
            let exit = options_motion(Some(&View::Preferences), &View::Home, 0.);
            assert_eq!(enter, (0, 20, 0.));
            assert_eq!(exit, (0, -20, 1.));
            assert_eq!(
                options_motion(Some(&View::Home), &View::Preferences, 1.),
                (-12, 0, 1.)
            );
            assert_eq!(
                options_motion(Some(&View::Preferences), &View::Home, 1.),
                (12, 0, 0.)
            );
            assert_eq!(
                options_motion(Some(&View::Home), &View::Details(0), 0.5).0,
                0
            );
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
        fn island_cannot_be_dragged_outside_any_monitor_work_area() {
            let areas = [
                RECT {
                    left: 0,
                    top: 0,
                    right: 1920,
                    bottom: 1040,
                },
                RECT {
                    left: -1280,
                    top: 0,
                    right: 0,
                    bottom: 984,
                },
            ];
            for (anchor, top) in [(-9000., -4000), (9000., 6000), (-900., 50), (1850., 1000)] {
                let (a, y, k) = bounded_geometry(&areas, anchor, top, 420., 536., 1.6);
                let width = (420. * k).round() as i32;
                let height = (536. * k).round() as i32;
                let x = (a - width as f32 / 2.).round() as i32;
                assert!(areas.iter().any(|r| x >= r.left + 8
                    && x + width <= r.right - 8
                    && y >= r.top + 8
                    && y + height <= r.bottom - 8));
            }
        }
        #[test]
        fn oversized_menu_fits_and_removed_monitor_recovers() {
            let areas = [RECT {
                left: 0,
                top: 40,
                right: 800,
                bottom: 600,
            }];
            let (a, y, k) = bounded_geometry(&areas, -1600., -500, 420., 536., 1.6);
            assert!(k < 1.6);
            assert!(y >= 48);
            assert!(y + (536. * k).round() as i32 <= 592);
            assert!(a >= 8.);
        }
        #[test]
        fn collapsed_circle_is_draggable_and_menu_body_is_not() {
            let mut s = initial(true, 0., 0);
            s.open = false;
            s.settings.circle = true;
            for (x, y) in [(38, 38), (38, 70), (52, 52), (20, 30)] {
                assert!(drag_zone(&s, x, y));
            }
            s.open = true;
            assert!(drag_zone(&s, 120, 28));
            assert!(!drag_zone(&s, 347, 29));
            assert!(!drag_zone(&s, 120, 150));
        }
        #[test]
        fn dismiss_abandons_confirmation_and_preserves_saved_accounts() {
            let mut s = initial(true, 0., 0);
            navigate(&mut s, View::SwitchConfirm(1), false);
            s.edit_focus = true;
            collapse(&mut s);
            assert!(!s.open);
            assert!(!s.peek);
            assert!(!s.edit_focus);
            assert!(matches!(s.view, View::Home));
            assert!(s.worker.is_none());
            assert_eq!(s.settings.accounts.len(), 2);
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
    if std::env::args().any(|a| a == "--check-codex-usage") {
        match codex::fetch_current() {
            Ok(u) => println!("{}", u.to_cache()),
            Err(e) => {
                eprintln!("Codex usage: {e}");
                std::process::exit(1)
            }
        }
        return;
    }
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
