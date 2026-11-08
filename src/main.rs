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
        ptr::null_mut,
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
        fn SetFocus(hwnd: HWND) -> HWND;
        fn ReleaseCapture() -> i32;
        fn SetProcessDPIAware() -> i32;
    }
    type Dc = *mut core::ffi::c_void;
    const BG: u32 = 0x000B0B0B;
    const WHITE: u32 = 0x00F2F3F1;
    const MUTED: u32 = 0x008C918E;
    const GREEN: u32 = 0x0095E2BB;
    const CW: f32 = 228.;
    const CH: f32 = 58.;
    const EW: f32 = 420.;
    const EH: f32 = 480.;
    enum ActionResult {
        Completed(String),
        NeedsClose,
    }
    struct State {
        open: bool,
        w: f32,
        h: f32,
        vw: f32,
        vh: f32,
        reveal: f32,
        hover: usize,
        hot: [f32; 24],
        focus: usize,
        down: Option<(i32, i32)>,
        drag: bool,
        origin: (i32, i32),
        anchor: f32,
        top: i32,
        saved: [bool; 2],
        active: usize,
        status: String,
        error: bool,
        until: f32,
        worker: Option<Receiver<Result<ActionResult, String>>>,
        pending: usize,
        confirmation: Option<usize>,
        closing: bool,
        started: f32,
        clock: Instant,
        last: Instant,
        reduced: bool,
        preview: bool,
        settings: Settings,
        detail: Option<usize>,
        name_edit: HWND,
        name_font: HFONT,
        edit_brush: HBRUSH,
        usage_worker: Option<Receiver<Vec<(usize, Result<Usage, String>)>>>,
        usage_errors: [String; 2],
        usage_next: f32,
        usage_consent: bool,
        minute: u64,
    }
    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }
    fn mix(a: u32, b: u32, t: f32) -> u32 {
        let mut c = 0;
        for sh in [0, 8, 16] {
            let av = ((a >> sh) & 255) as f32;
            let bv = ((b >> sh) & 255) as f32;
            c |= ((av + (bv - av) * t.clamp(0., 1.)) as u32) << sh;
        }
        c
    }
    fn xy(lp: LPARAM) -> (i32, i32) {
        (
            (lp as u32 & 65535) as i16 as i32,
            ((lp as u32 >> 16) & 65535) as i16 as i32,
        )
    }
    unsafe fn panel(dc: Dc, x: i32, y: i32, w: i32, h: i32, r: i32, c: u32) {
        let b = CreateSolidBrush(c);
        let ob = SelectObject(dc, b);
        let op = SelectObject(dc, GetStockObject(NULL_PEN));
        RoundRect(dc, x * 2, y * 2, (x + w) * 2, (y + h) * 2, r * 4, r * 4);
        SelectObject(dc, op);
        SelectObject(dc, ob);
        DeleteObject(b);
    }
    unsafe fn text(dc: Dc, x: i32, y: i32, size: i32, weight: i32, s: &str, c: u32) {
        let f = CreateFontW(
            -size * 2,
            0,
            0,
            0,
            weight,
            0,
            0,
            0,
            DEFAULT_CHARSET as u32,
            OUT_DEFAULT_PRECIS as u32,
            CLIP_DEFAULT_PRECIS as u32,
            ANTIALIASED_QUALITY as u32,
            DEFAULT_PITCH as u32,
            wide("Segoe UI Variable Display").as_ptr(),
        );
        let old = SelectObject(dc, f);
        SetBkMode(dc, TRANSPARENT as i32);
        SetTextColor(dc, c);
        let chars: Vec<u16> = s.encode_utf16().collect();
        TextOutW(dc, x * 2, y * 2, chars.as_ptr(), chars.len() as i32);
        SelectObject(dc, old);
        DeleteObject(f);
    }
    unsafe fn line(dc: Dc, points: &[(i32, i32)], width: i32, c: u32) {
        let pen = CreatePen(PS_SOLID, width * 2, c);
        let old = SelectObject(dc, pen);
        let p: Vec<POINT> = points
            .iter()
            .map(|(x, y)| POINT { x: x * 2, y: y * 2 })
            .collect();
        Polyline(dc, p.as_ptr(), p.len() as i32);
        SelectObject(dc, old);
        DeleteObject(pen);
    }
    unsafe fn mark(dc: Dc, x: i32, y: i32, c: u32) {
        line(
            dc,
            &[
                (x, y + 13),
                (x, y),
                (x + 7, y + 8),
                (x + 14, y),
                (x + 14, y + 13),
            ],
            2,
            c,
        );
    }
    fn hit(s: &State, x: i32, y: i32) -> usize {
        let w = s.w as i32;
        if x < 0 || x >= w || y < 0 || y >= s.h as i32 {
            return 0;
        }
        if s.usage_consent && y >= 58 {
            return if (286..328).contains(&y) {
                if (24..144).contains(&x) {
                    22
                } else if (156..w - 24).contains(&x) {
                    23
                } else {
                    0
                }
            } else {
                0
            };
        }
        if s.detail.is_some() && y >= 58 {
            return if (368..410).contains(&y) {
                if (24..144).contains(&x) {
                    15
                } else if (156..w - 24).contains(&x) {
                    14
                } else {
                    0
                }
            } else if (300..340).contains(&y) && (24..w - 24).contains(&x) {
                13
            } else {
                0
            };
        }
        if y < 58 {
            return if s.open && x > w - 90 && x < w - 48 {
                6
            } else {
                1
            };
        }
        if !s.open || s.reveal < 0.85 {
            return 0;
        }
        if s.confirmation.is_some() {
            return if (232..274).contains(&y) {
                if (24..144).contains(&x) {
                    8
                } else if (156..w - 24).contains(&x) {
                    9
                } else {
                    0
                }
            } else {
                0
            };
        }
        if s.worker.is_some() {
            return 0;
        }
        if (388..426).contains(&y) && (24..w - 24).contains(&x) {
            return 12;
        }
        for i in 0..2 {
            let top = 126 + i * 124;
            if y >= top && y < top + 114 && x >= 16 && x < w - 16 {
                if y < top + 50 && x > w - 96 {
                    return 3 + i as usize * 2;
                }
                if y < top + 50 && x > w - 132 {
                    return 10 + i as usize;
                }
                return 2 + i as usize * 2;
            }
        }
        0
    }
    const WM_APP_NAME: u32 = WM_APP + 11;
    unsafe fn clipped_text(
        dc: Dc,
        x: i32,
        y: i32,
        size: i32,
        weight: i32,
        label: &str,
        color: u32,
        width: i32,
    ) {
        let saved = SaveDC(dc);
        IntersectClipRect(dc, x * 2, y * 2, (x + width) * 2, (y + size + 5) * 2);
        text(dc, x, y, size, weight, label, color);
        RestoreDC(dc, saved);
    }
    unsafe fn draw_quota(
        dc: Dc,
        x: i32,
        y: i32,
        width: i32,
        label: &str,
        window: Option<&Window>,
        a: f32,
    ) {
        let color = |c| mix(BG, c, a);
        let now = accounts::now();
        let expired = window.and_then(|w| w.resets).is_some_and(|t| t <= now);
        let title = window
            .map(|w| format!("{label} · {:.0}% used", w.used))
            .unwrap_or_else(|| format!("{label} · —"));
        text(dc, x, y, 11, 600, &title, color(WHITE));
        panel(dc, x, y + 18, width, 3, 1, color(0x00313B34));
        if let Some(w) = window {
            let accent = if expired {
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
                panel(dc, x, y + 18, fill, 3, 1, color(accent));
            }
        }
        let reset = window
            .map(|w| w.reset_label(now))
            .unwrap_or_else(|| "Not available".into());
        text(dc, x, y + 27, 10, 400, &reset, color(MUTED));
    }
    unsafe fn close_detail(s: &mut State) {
        if !s.name_edit.is_null() {
            DestroyWindow(s.name_edit);
            s.name_edit = null_mut();
        }
        if !s.name_font.is_null() {
            DeleteObject(s.name_font);
            s.name_font = null_mut();
        }
        s.detail = None;
    }
    unsafe fn open_detail(hwnd: HWND, s: &mut State, index: usize) {
        if s.worker.is_some() || s.confirmation.is_some() || s.usage_consent {
            return;
        }
        close_detail(s);
        s.detail = Some(index);
        s.open = true;
        s.focus = 0;
        s.name_font = CreateFontW(
            -17,
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
        s.name_edit = CreateWindowExW(
            0,
            wide("EDIT").as_ptr(),
            wide(&s.settings.accounts[index].name).as_ptr(),
            WS_CHILD | WS_VISIBLE | WS_TABSTOP | ES_AUTOHSCROLL as u32,
            34,
            135,
            EW as i32 - 68,
            27,
            hwnd,
            100usize as HMENU,
            GetModuleHandleW(null_mut()),
            null_mut(),
        );
        SendMessageW(s.name_edit, WM_SETFONT, s.name_font as WPARAM, 1);
        SendMessageW(s.name_edit, 0x00C5, 64, 0);
        SetFocus(s.name_edit);
        SendMessageW(s.name_edit, 0x00B1, 0, -1);
        InvalidateRect(hwnd, null_mut(), 0);
    }
    fn persist(s: &mut State) -> bool {
        if s.preview {
            return true;
        }
        match s.settings.save() {
            Ok(()) => true,
            Err(_) => {
                s.status = "Could not save settings on this device".into();
                s.error = true;
                s.until = s.clock.elapsed().as_secs_f32() + 6.;
                false
            }
        }
    }
    unsafe fn save_name(hwnd: HWND, s: &mut State) {
        if let Some(index) = s.detail {
            let mut buffer = [0u16; 128];
            let len = GetWindowTextW(s.name_edit, buffer.as_mut_ptr(), buffer.len() as i32);
            let name = String::from_utf16_lossy(&buffer[..len.max(0) as usize]);
            match accounts::valid_name(&name) {
                Ok(name) => {
                    let old = s.settings.accounts[index].name.clone();
                    s.settings.accounts[index].name = name;
                    if persist(s) {
                        close_detail(s);
                        s.status = "Account name saved".into();
                        s.until = s.clock.elapsed().as_secs_f32() + 4.;
                        SetFocus(hwnd);
                    } else {
                        s.settings.accounts[index].name = old;
                    }
                }
                Err(e) => {
                    s.status = e.to_string();
                    s.until = s.clock.elapsed().as_secs_f32() + 5.;
                }
            }
        }
    }
    unsafe fn refresh_usage(s: &mut State) {
        if s.usage_worker.is_some() || s.worker.is_some() {
            return;
        }
        if !s.settings.live_usage && !s.preview {
            s.usage_consent = true;
            s.open = true;
            s.focus = 22;
            close_detail(s);
            return;
        }
        s.usage_next = s.clock.elapsed().as_secs_f32() + 300.;
        let indices = (0..2).filter(|&i| s.saved[i]).collect::<Vec<_>>();
        if indices.is_empty() {
            s.status = "Save an account first to connect its usage".into();
            s.until = s.clock.elapsed().as_secs_f32() + 5.;
            return;
        }
        let preview = s.preview;
        let (tx, rx) = mpsc::channel();
        s.usage_worker = Some(rx);
        std::thread::spawn(move || {
            let results = indices
                .into_iter()
                .map(|i| {
                    let result = if preview {
                        Ok(Usage {
                            session: Some(Window {
                                used: 28.,
                                resets: Some(accounts::now() + 7200),
                            }),
                            weekly: Some(Window {
                                used: 46.,
                                resets: Some(accounts::now() + 86400 * 3),
                            }),
                            updated: accounts::now(),
                            plan: "Demo".into(),
                        })
                    } else {
                        usage::fetch(i + 1).map_err(|e| e.to_string())
                    };
                    (i, result)
                })
                .collect();
            let _ = tx.send(results);
        });
    }
    unsafe fn paint(hwnd: HWND, s: &State) {
        let mut ps = zeroed();
        let dc = BeginPaint(hwnd, &mut ps);
        let w = s.w.round() as i32;
        let h = s.h.round() as i32;
        let mem = CreateCompatibleDC(dc);
        let bmp = CreateCompatibleBitmap(dc, w * 2, h * 2);
        let old = SelectObject(mem, bmp);
        let r = RECT {
            left: 0,
            top: 0,
            right: w * 2,
            bottom: h * 2,
        };
        let b = CreateSolidBrush(BG);
        FillRect(mem, &r, b);
        DeleteObject(b);
        // A one-pixel rim keeps the black silhouette readable on dark desktops.
        let radius = 29 + (s.reveal * 7.) as i32;
        panel(mem, 0, 0, w, h, radius, 0x002A2D2B);
        panel(mem, 1, 1, w - 2, h - 2, radius - 1, BG);
        panel(mem, 16, 14, 30, 30, 15, 0x0026362E);
        mark(mem, 24, 22, GREEN);
        text(mem, 57, 17, 18, 600, "Muisland", WHITE);
        if s.open {
            text(mem, w - 79, 17, 18, 600, "···", mix(MUTED, WHITE, s.hot[6]));
        }
        panel(
            mem,
            w - 43,
            14,
            29,
            29,
            15,
            mix(0x001B1E1C, 0x00383D39, s.hot[1]),
        );
        let cy = 28;
        line(mem, &[(w - 33, cy), (w - 24, cy)], 1, WHITE);
        let plus = (1. - s.reveal) * 5.;
        if plus > 0.5 {
            line(
                mem,
                &[(w - 28, cy - plus as i32), (w - 28, cy + plus as i32)],
                1,
                WHITE,
            );
        }
        if s.reveal > 0.01 {
            let a = s.reveal;
            let off = ((1. - a) * 12.) as i32;
            if s.usage_consent {
                let color = |c| mix(BG, c, a);
                text(
                    mem,
                    24,
                    84 + off,
                    23,
                    600,
                    "Connect live usage",
                    color(WHITE),
                );
                text(
                    mem,
                    25,
                    128 + off,
                    13,
                    400,
                    "Read real limits using your saved Claude login.",
                    color(MUTED),
                );
                text(
                    mem,
                    25,
                    154 + off,
                    13,
                    400,
                    "Its sign-in token is sent over HTTPS only to",
                    color(MUTED),
                );
                text(
                    mem,
                    25,
                    176 + off,
                    15,
                    600,
                    "api.anthropic.com",
                    color(GREEN),
                );
                text(
                    mem,
                    25,
                    209 + off,
                    12,
                    400,
                    "No chats are sent. Refreshes every 5 minutes.",
                    color(MUTED),
                );
                text(
                    mem,
                    25,
                    232 + off,
                    12,
                    400,
                    "You can turn this off in the menu at any time.",
                    color(MUTED),
                );
                panel(
                    mem,
                    24,
                    286,
                    120,
                    42,
                    21,
                    color(mix(0x00212622, 0x00384039, s.hot[22])),
                );
                text(mem, 62, 298, 13, 600, "Cancel", color(WHITE));
                panel(
                    mem,
                    156,
                    286,
                    w - 180,
                    42,
                    21,
                    color(mix(GREEN, WHITE, s.hot[23] * 0.2)),
                );
                text(mem, 205, 298, 13, 600, "Enable usage", color(BG));
            } else if let Some(index) = s.detail {
                text(
                    mem,
                    24,
                    78 + off,
                    24,
                    600,
                    "Account details",
                    mix(BG, WHITE, a),
                );
                text(
                    mem,
                    25,
                    108 + off,
                    12,
                    400,
                    "Name · up to 32 characters",
                    mix(BG, MUTED, a),
                );
                panel(mem, 24, 126, w - 48, 44, 12, 0x00212723);
                let account = &s.settings.accounts[index];
                let plan = account
                    .usage
                    .as_ref()
                    .map(|u| u.plan.as_str())
                    .filter(|p| !p.is_empty())
                    .unwrap_or("Not yet read");
                text(
                    mem,
                    25,
                    185,
                    12,
                    600,
                    &format!("PLAN · {}", plan.to_uppercase()),
                    mix(BG, WHITE, a),
                );
                draw_quota(
                    mem,
                    25,
                    219,
                    w / 2 - 44,
                    "5h",
                    account.usage.as_ref().and_then(|u| u.session.as_ref()),
                    a,
                );
                draw_quota(
                    mem,
                    w / 2 + 6,
                    219,
                    w / 2 - 34,
                    "7d",
                    account.usage.as_ref().and_then(|u| u.weekly.as_ref()),
                    a,
                );
                let source = if s.preview {
                    "Preview · sample data".into()
                } else if let Some(u) = &account.usage {
                    format!(
                        "Last read {}",
                        accounts::age(accounts::now().saturating_sub(u.updated))
                    )
                } else {
                    "No quota reading for this account yet".into()
                };
                clipped_text(mem, 25, 277, 11, 400, &source, MUTED, w - 50);
                panel(
                    mem,
                    24,
                    300,
                    w - 48,
                    40,
                    20,
                    mix(0x00212C24, 0x0037473C, s.hot[13]),
                );
                text(
                    mem,
                    130,
                    311,
                    13,
                    600,
                    if s.usage_worker.is_some() {
                        "Refreshing…"
                    } else {
                        "Refresh live usage"
                    },
                    GREEN,
                );
                text(
                    mem,
                    25,
                    348,
                    11,
                    400,
                    "Names stay on this device. Limits come from Claude.",
                    MUTED,
                );
                panel(
                    mem,
                    24,
                    368,
                    120,
                    42,
                    21,
                    mix(0x00212622, 0x00384039, s.hot[15]),
                );
                text(mem, 68, 380, 13, 600, "Back", WHITE);
                panel(
                    mem,
                    156,
                    368,
                    w - 180,
                    42,
                    21,
                    mix(GREEN, WHITE, s.hot[14] * 0.2),
                );
                text(mem, 208, 380, 13, 600, "Save name", BG);
            } else if let Some(id) = s.confirmation {
                let color = |c| mix(BG, c, a);
                panel(mem, 24, 75 + off, 34, 34, 17, color(0x002B3245));
                text(mem, 37, 81 + off, 19, 600, "!", color(0x009FC5F2));
                text(
                    mem,
                    24,
                    124 + off,
                    23,
                    600,
                    "Close Claude & save?",
                    color(WHITE),
                );
                text(
                    mem,
                    25,
                    159 + off,
                    13,
                    400,
                    "Claude Desktop will close. Local tasks may stop",
                    color(MUTED),
                );
                text(
                    mem,
                    25,
                    180 + off,
                    13,
                    400,
                    "and unsaved work may be lost.",
                    color(MUTED),
                );
                text(
                    mem,
                    25,
                    207 + off,
                    12,
                    400,
                    &format!("Save to {}.", s.settings.accounts[(id - 2) / 2].name),
                    color(WHITE),
                );
                panel(
                    mem,
                    24,
                    232,
                    120,
                    42,
                    21,
                    color(mix(0x00212622, 0x00384039, s.hot[8])),
                );
                text(mem, 62, 244, 13, 600, "Cancel", color(WHITE));
                panel(
                    mem,
                    156,
                    232,
                    w - 180,
                    42,
                    21,
                    color(mix(GREEN, WHITE, s.hot[9] * 0.25)),
                );
                text(mem, 191, 244, 13, 600, "Close & save", color(BG));
                if s.focus == 8 {
                    panel(mem, 56, 266, 54, 2, 1, color(GREEN));
                }
                if s.focus == 9 {
                    panel(mem, 186, 266, 92, 2, 1, color(BG));
                }
            } else {
                text(
                    mem,
                    23,
                    74 + off,
                    24,
                    600,
                    "Your accounts.",
                    mix(BG, WHITE, a),
                );
                text(
                    mem,
                    24,
                    105 + off,
                    12,
                    400,
                    "One little island. A seamless switch.",
                    mix(BG, MUTED, a),
                );
                for i in 0..2 {
                    let delay = (a * 1.25 - i as f32 * 0.15).clamp(0., 1.);
                    let y = 126 + i as i32 * 124 + ((1. - delay) * 16.) as i32;
                    let id = 2 + i * 2;
                    let hot = s.hot[id].max(s.hot[id + 1]);
                    let account = &s.settings.accounts[i];
                    panel(
                        mem,
                        16,
                        y,
                        w - 32,
                        114,
                        20,
                        mix(BG, mix(0x00191C1A, 0x002A312C, hot), delay),
                    );
                    panel(
                        mem,
                        28,
                        y + 14,
                        34,
                        34,
                        17,
                        mix(BG, if i == 0 { 0x0034483C } else { 0x00403632 }, delay),
                    );
                    text(
                        mem,
                        40,
                        y + 20,
                        15,
                        600,
                        &(i + 1).to_string(),
                        mix(BG, GREEN, delay),
                    );
                    clipped_text(
                        mem,
                        75,
                        y + 12,
                        15,
                        600,
                        &account.name,
                        mix(BG, WHITE, delay),
                        w - 216,
                    );
                    let subtitle = if s.preview {
                        "Preview · sample data".into()
                    } else if !s.saved[i] {
                        "Not saved · Save to connect".into()
                    } else if let Some(u) = &account.usage {
                        format!(
                            "{} · {}{}",
                            if s.active == i + 1 { "Active" } else { "Saved" },
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
                        "Saved · usage not loaded".into()
                    };
                    clipped_text(
                        mem,
                        75,
                        y + 33,
                        10,
                        400,
                        &subtitle,
                        mix(BG, MUTED, delay),
                        w - 180,
                    );
                    panel(
                        mem,
                        w - 124,
                        y + 17,
                        25,
                        28,
                        13,
                        mix(BG, mix(0x00252C27, 0x00414C44, s.hot[10 + i]), delay),
                    );
                    text(mem, w - 119, y + 21, 15, 600, "···", mix(BG, MUTED, delay));
                    panel(
                        mem,
                        w - 91,
                        y + 17,
                        60,
                        28,
                        14,
                        mix(BG, mix(0x002C332E, 0x00445147, s.hot[id + 1]), delay),
                    );
                    text(mem, w - 75, y + 23, 12, 600, "Save", mix(BG, GREEN, delay));
                    draw_quota(
                        mem,
                        28,
                        y + 59,
                        w / 2 - 46,
                        "5h",
                        account.usage.as_ref().and_then(|u| u.session.as_ref()),
                        delay,
                    );
                    draw_quota(
                        mem,
                        w / 2 + 6,
                        y + 59,
                        w / 2 - 34,
                        "7d",
                        account.usage.as_ref().and_then(|u| u.weekly.as_ref()),
                        delay,
                    );
                    if s.focus == id || s.focus == id + 1 || s.focus == 10 + i {
                        panel(mem, 20, y + 22, 2, 16, 1, GREEN);
                    }
                }
                panel(
                    mem,
                    24,
                    388,
                    w - 48,
                    38,
                    16,
                    mix(BG, mix(0x001A201C, 0x002A342E, s.hot[12]), a),
                );
                text(
                    mem,
                    37,
                    400,
                    11,
                    600,
                    "Back up local data before switch",
                    mix(BG, WHITE, a),
                );
                panel(
                    mem,
                    w - 79,
                    398,
                    40,
                    18,
                    9,
                    mix(
                        BG,
                        if s.settings.protect_local {
                            GREEN
                        } else {
                            0x00434B45
                        },
                        a,
                    ),
                );
                panel(
                    mem,
                    if s.settings.protect_local {
                        w - 58
                    } else {
                        w - 77
                    },
                    400,
                    14,
                    14,
                    7,
                    mix(BG, if s.settings.protect_local { BG } else { WHITE }, a),
                );
            }
            panel(mem, 24, 438, w - 48, 1, 0, mix(BG, 0x00272C28, a));
            let busy = s.worker.is_some();
            let err = s.error && !s.status.is_empty();
            let color = if err { 0x009BA7F0 } else { GREEN };
            if busy {
                let t = s.clock.elapsed().as_secs_f32() * 6.;
                for j in 0..3 {
                    let height = 4 + ((t + j as f32 * 0.9).sin() * 3. + 3.) as i32;
                    panel(
                        mem,
                        25 + j * 5,
                        459 - height / 2,
                        2,
                        height,
                        1,
                        mix(BG, GREEN, a),
                    );
                }
            } else {
                panel(mem, 25, 456, 6, 6, 3, mix(BG, color, a));
            }
            let label = if busy {
                if s.closing {
                    "Closing Claude, then saving your session…"
                } else if s.pending == 30 {
                    "Recovering local backup…"
                } else if matches!(s.pending, 2 | 4) && s.settings.protect_local {
                    "Backing up local data, then switching…"
                } else if s.pending == 7 {
                    "Restoring last session…"
                } else if s.pending % 2 == 1 {
                    "Saving your session…"
                } else {
                    "Switching your account…"
                }
            } else if s.usage_worker.is_some() {
                "Reading real account limits from Anthropic…"
            } else if s.usage_consent {
                "Usage stays off until you enable it"
            } else if s.confirmation.is_some() {
                "Nothing closes until you confirm"
            } else if !s.status.is_empty() {
                &s.status
            } else {
                if let Some(i) = s.detail {
                    if s.usage_errors[i].is_empty() {
                        "Close Claude before switching"
                    } else {
                        &s.usage_errors[i]
                    }
                } else {
                    "Account menu · rename and refresh usage"
                }
            };
            clipped_text(
                mem,
                46,
                451,
                11,
                400,
                label,
                mix(BG, if err { color } else { MUTED }, a),
                w - 65,
            );
        }
        if s.focus == 1 {
            panel(mem, w - 45, 44, 31, 2, 1, GREEN);
        }
        SetStretchBltMode(dc, HALFTONE);
        SetBrushOrgEx(dc, 0, 0, null_mut());
        StretchBlt(dc, 0, 0, w, h, mem, 0, 0, w * 2, h * 2, SRCCOPY);
        SelectObject(mem, old);
        DeleteObject(bmp);
        DeleteDC(mem);
        EndPaint(hwnd, &ps);
    }
    unsafe fn shape(hwnd: HWND, s: &State) {
        let radius = 29 + (s.reveal * 7.) as i32;
        let r = CreateRoundRectRgn(
            0,
            0,
            s.w.round() as i32 + 1,
            s.h.round() as i32 + 1,
            radius * 2,
            radius * 2,
        );
        if !r.is_null() && SetWindowRgn(hwnd, r, 0) == 0 {
            DeleteObject(r);
        }
    }
    unsafe fn start(s: &mut State, id: usize) {
        run_action(s, id, false);
    }
    unsafe fn run_action(s: &mut State, id: usize, close_confirmed: bool) {
        if s.worker.is_some() || s.usage_worker.is_some() {
            s.status = "Please wait for the current action".into();
            s.until = s.clock.elapsed().as_secs_f32() + 4.;
            return;
        }
        if s.confirmation.is_some() && !close_confirmed {
            return;
        }
        s.confirmation = None;
        s.closing = close_confirmed;
        let preview = s.preview;
        let protect_local = s.settings.protect_local;
        let (tx, rx) = mpsc::channel();
        s.worker = Some(rx);
        s.pending = id;
        s.started = s.clock.elapsed().as_secs_f32();
        s.error = false;
        s.status.clear();
        std::thread::spawn(move || {
            let saving = matches!(id, 3 | 5);
            // Checking on the worker keeps tasklist off the animation thread.
            if saving && !close_confirmed && (preview || profiles::claude_running()) {
                let _ = tx.send(Ok(ActionResult::NeedsClose));
                return;
            }
            if preview {
                let _ = tx.send(Ok(ActionResult::Completed(
                    "Preview complete · no session changes".into(),
                )));
                return;
            }
            if close_confirmed {
                if let Err(e) = profiles::close_claude_for_save() {
                    let _ = tx.send(Err(e.to_string()));
                    return;
                }
            }
            if id == 30 {
                let result = crate::backup::recover_latest()
                    .map(|path| {
                        let _ = std::process::Command::new("explorer.exe")
                            .arg(&path)
                            .spawn();
                        ActionResult::Completed("Recovered into a separate folder".into())
                    })
                    .map_err(|e| e.to_string());
                let _ = tx.send(result);
                return;
            }
            let result = if id == 7 {
                profiles::restore_backup()
            } else {
                let account = (id - 2) / 2 + 1;
                if id % 2 == 1 {
                    profiles::capture(account)
                } else {
                    profiles::switch(account, protect_local)
                }
            };
            let result = result
                .map(|_| {
                    if id == 7 {
                        "Previous session restored".into()
                    } else if id % 2 == 1 {
                        format!("Account {} saved", (id - 2) / 2 + 1)
                    } else {
                        profiles::open_claude();
                        format!("Account {} is ready", (id - 2) / 2 + 1)
                    }
                })
                .map_err(|e| e.to_string());
            let _ = tx.send(result.map(ActionResult::Completed));
        });
    }
    unsafe fn menu(hwnd: HWND, s: &mut State) {
        let menu = CreatePopupMenu();
        AppendMenuW(
            menu,
            MF_STRING,
            10,
            wide("Restore previous session").as_ptr(),
        );
        AppendMenuW(
            menu,
            MF_STRING | if s.reduced { MF_CHECKED } else { 0 },
            11,
            wide("Reduce motion").as_ptr(),
        );
        AppendMenuW(menu, MF_SEPARATOR, 0, null_mut());
        for i in 0..2 {
            AppendMenuW(
                menu,
                MF_STRING,
                20 + i,
                wide(&format!(
                    "{} · name & usage",
                    s.settings.accounts[i].name.replace('&', "&&")
                ))
                .as_ptr(),
            );
        }
        AppendMenuW(
            menu,
            MF_STRING,
            22,
            wide("Refresh all account usage").as_ptr(),
        );
        AppendMenuW(
            menu,
            MF_STRING | if s.settings.live_usage { MF_CHECKED } else { 0 },
            24,
            wide("Live usage · Anthropic connection").as_ptr(),
        );
        AppendMenuW(
            menu,
            MF_STRING,
            25,
            wide("Recover latest local backup to folder").as_ptr(),
        );
        AppendMenuW(menu, MF_SEPARATOR, 0, null_mut());
        AppendMenuW(menu, MF_STRING, 12, wide("Quit Muisland").as_ptr());
        let mut p = zeroed();
        GetCursorPos(&mut p);
        let choice = TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_RIGHTBUTTON,
            p.x,
            p.y,
            0,
            hwnd,
            null_mut(),
        );
        DestroyMenu(menu);
        match choice {
            20 | 21 => open_detail(hwnd, s, (choice - 20) as usize),
            22 => refresh_usage(s),
            24 => {
                if s.settings.live_usage {
                    s.settings.live_usage = false;
                    persist(s);
                } else {
                    s.usage_consent = true;
                    s.open = true;
                    s.focus = 22;
                    close_detail(s);
                }
            }
            25 => start(s, 30),
            10 => start(s, 7),
            11 => s.reduced = !s.reduced,
            12 => {
                PostMessageW(hwnd, WM_CLOSE, 0, 0);
            }
            _ => {}
        }
    }
    unsafe fn activate(hwnd: HWND, s: &mut State, id: usize) {
        if id == 22 && s.usage_consent {
            s.usage_consent = false;
            s.focus = 0;
        } else if id == 23 && s.usage_consent {
            s.usage_consent = false;
            s.settings.live_usage = true;
            if persist(s) {
                refresh_usage(s);
            }
            s.focus = 0;
        } else if id == 8 && s.confirmation.is_some() {
            s.confirmation = None;
            s.status = "Cancelled · Claude was left open".into();
            s.until = s.clock.elapsed().as_secs_f32() + 4.;
            s.focus = 0;
        } else if id == 9 {
            if let Some(pending) = s.confirmation {
                run_action(s, pending, true);
                s.focus = 0;
            }
        } else {
            match id {
                1 => {
                    s.open = !s.open;
                    if !s.open {
                        s.confirmation = None;
                        s.usage_consent = false;
                        close_detail(s);
                        s.focus = 0;
                    }
                }
                2..=5 => start(s, id),
                10 | 11 => open_detail(hwnd, s, id - 10),
                12 => {
                    s.settings.protect_local = !s.settings.protect_local;
                    persist(s);
                }
                13 => refresh_usage(s),
                14 => save_name(hwnd, s),
                15 => close_detail(s),
                6 => menu(hwnd, s),
                _ => {}
            }
        }
        InvalidateRect(hwnd, null_mut(), 0);
    }
    unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
        let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut State;
        if ptr.is_null() {
            return DefWindowProcW(hwnd, msg, wp, lp);
        }
        if msg == WM_DESTROY {
            KillTimer(hwnd, 17);
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            let mut owned = Box::from_raw(ptr);
            close_detail(&mut owned);
            if !owned.edit_brush.is_null() {
                DeleteObject(owned.edit_brush);
            }
            drop(owned);
            PostQuitMessage(0);
            return 0;
        }
        let s = &mut *ptr;
        match msg {
            WM_CTLCOLOREDIT => {
                let dc = wp as Dc;
                SetTextColor(dc, WHITE);
                SetBkColor(dc, 0x00212723);
                s.edit_brush as LRESULT
            }
            WM_APP_NAME => {
                if wp == 1 {
                    save_name(hwnd, s);
                } else {
                    close_detail(s);
                }
                InvalidateRect(hwnd, null_mut(), 0);
                0
            }
            WM_CLOSE if s.worker.is_some() => {
                s.status = "Please wait for the current action".into();
                0
            }
            WM_ERASEBKGND => 1,
            WM_PAINT => {
                paint(hwnd, s);
                0
            }
            WM_TIMER => {
                let now = Instant::now();
                let dt = now.duration_since(s.last).as_secs_f32().min(0.05);
                s.last = now;
                let tw = if s.open { EW } else { CW };
                let th = if s.open { EH } else { CH };
                let before = (s.w, s.h, s.reveal, s.hot);
                if s.reduced {
                    s.w = tw;
                    s.h = th;
                    s.vw = 0.;
                    s.vh = 0.;
                    s.reveal = if s.open { 1. } else { 0. };
                } else {
                    // Integrate in small steps: spring speed is independent of timer jitter.
                    let steps = (dt / 0.004).ceil().max(1.) as usize;
                    let step = dt / steps as f32;
                    for _ in 0..steps {
                        s.vw += ((tw - s.w) * 360. - s.vw * 31.) * step;
                        s.vh += ((th - s.h) * 330. - s.vh * 30.) * step;
                        s.w += s.vw * step;
                        s.h += s.vh * step;
                    }
                    if (s.w - tw).abs() < 0.05 && s.vw.abs() < 0.2 {
                        s.w = tw;
                        s.vw = 0.;
                    }
                    if (s.h - th).abs() < 0.05 && s.vh.abs() < 0.2 {
                        s.h = th;
                        s.vh = 0.;
                    }
                    let target = if s.open {
                        ((s.h - CH) / (EH - CH) * 1.5 - 0.3).clamp(0., 1.)
                    } else {
                        0.
                    };
                    s.reveal += (target - s.reveal) * (dt * 18.).min(1.);
                    if (target - s.reveal).abs() < 0.002 {
                        s.reveal = target;
                    }
                }
                let mut cursor = zeroed();
                GetCursorPos(&mut cursor);
                ScreenToClient(hwnd, &mut cursor);
                s.hover = hit(s, cursor.x, cursor.y);
                for i in 0..24 {
                    let target = if s.hover == i || s.focus == i { 1. } else { 0. };
                    s.hot[i] += (target - s.hot[i]) * (dt * 16.).min(1.);
                    if (s.hot[i] - target).abs() < 0.005 {
                        s.hot[i] = target;
                    }
                }
                if (before.0 - s.w).abs() > 0.001 || (before.1 - s.h).abs() > 0.001 {
                    SetWindowPos(
                        hwnd,
                        null_mut(),
                        (s.anchor - s.w / 2.).round() as i32,
                        s.top,
                        s.w.round() as i32,
                        s.h.round() as i32,
                        SWP_NOZORDER | SWP_NOACTIVATE,
                    );
                    shape(hwnd, s);
                }
                let mut changed = false;
                if let Some(result) = s.usage_worker.as_ref().and_then(|rx| rx.try_recv().ok()) {
                    s.usage_worker = None;
                    let any_success = result.iter().any(|(_, r)| r.is_ok());
                    for (i, result) in result {
                        match result {
                            Ok(value) => {
                                s.settings.accounts[i].usage = Some(value);
                                s.usage_errors[i].clear();
                            }
                            Err(error) => s.usage_errors[i] = error,
                        }
                    }
                    if !s.preview {
                        persist(s);
                    }
                    s.status = if any_success {
                        "Account usage updated"
                    } else {
                        "Usage unavailable · open account details"
                    }
                    .into();
                    s.error = !any_success;
                    s.until = s.clock.elapsed().as_secs_f32() + 4.;
                    changed = true;
                }
                let minute = accounts::now() / 60;
                if minute != s.minute {
                    s.minute = minute;
                    changed = true;
                }
                if s.settings.live_usage
                    && s.clock.elapsed().as_secs_f32() >= s.usage_next
                    && s.usage_worker.is_none()
                    && s.worker.is_none()
                {
                    refresh_usage(s);
                }

                if s.clock.elapsed().as_secs_f32() - s.started > 0.45 {
                    if let Some(result) = s.worker.as_ref().and_then(|r| r.try_recv().ok()) {
                        s.worker = None;
                        s.closing = false;
                        s.until = s.clock.elapsed().as_secs_f32() + 6.;
                        match result {
                            Ok(ActionResult::NeedsClose) => {
                                s.confirmation = Some(s.pending);
                                s.open = true;
                                s.status.clear();
                                s.focus = 8; // Cancel is the safe default for Enter.
                            }
                            Ok(ActionResult::Completed(message)) => {
                                s.status = message;
                                if matches!(s.pending, 2 | 4) {
                                    s.active = (s.pending - 2) / 2 + 1;
                                }
                                if matches!(s.pending, 3 | 5) {
                                    let i = (s.pending - 2) / 2;
                                    s.settings.accounts[i].usage = None;
                                    s.usage_errors[i].clear();
                                    if !s.preview {
                                        persist(s);
                                    }
                                    s.usage_next = s.clock.elapsed().as_secs_f32() + 1.;
                                }
                                if !s.preview {
                                    s.saved = [profiles::saved(1), profiles::saved(2)];
                                }
                            }
                            Err(message) => {
                                s.error = true;
                                s.status = "Action could not be completed".into();
                                MessageBoxW(
                                    hwnd,
                                    wide(&message).as_ptr(),
                                    wide("Muisland").as_ptr(),
                                    MB_OK | MB_ICONINFORMATION,
                                );
                            }
                        }
                        changed = true;
                    }
                }
                if !s.status.is_empty() && s.clock.elapsed().as_secs_f32() > s.until {
                    s.status.clear();
                    s.error = false;
                    changed = true;
                }
                if before != (s.w, s.h, s.reveal, s.hot) || s.worker.is_some() || changed {
                    InvalidateRect(hwnd, null_mut(), 0);
                }
                0
            }
            WM_LBUTTONDOWN => {
                let (x, y) = xy(lp);
                s.down = Some((x, y));
                s.drag = false;
                s.focus = 0;
                if y < 58 && x < s.w as i32 - 92 {
                    let mut p = zeroed();
                    GetCursorPos(&mut p);
                    let mut r = zeroed();
                    GetWindowRect(hwnd, &mut r);
                    s.origin = (p.x - r.left, p.y - r.top);
                    SetCapture(hwnd);
                }
                0
            }
            WM_MOUSEMOVE => {
                if let Some((dx, dy)) = s.down {
                    let (x, y) = xy(lp);
                    if dy < 58
                        && dx < s.w as i32 - 92
                        && ((x - dx).abs() > 4 || (y - dy).abs() > 4 || s.drag)
                    {
                        s.drag = true;
                        let mut p = zeroed();
                        GetCursorPos(&mut p);
                        let left = p.x - s.origin.0;
                        s.top = p.y - s.origin.1;
                        s.anchor = left as f32 + s.w / 2.;
                        SetWindowPos(
                            hwnd,
                            null_mut(),
                            left,
                            s.top,
                            0,
                            0,
                            SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
                        );
                    }
                }
                0
            }
            WM_LBUTTONUP => {
                let down = s.down.take();
                let dragged = s.drag;
                ReleaseCapture();
                let (x, y) = xy(lp);
                if let Some((dx, dy)) = down {
                    if !dragged && (x - dx).abs() < 8 && (y - dy).abs() < 8 {
                        let id = hit(s, x, y);
                        if id == hit(s, dx, dy) {
                            activate(hwnd, s, id);
                        }
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
                menu(hwnd, s);
                0
            }
            WM_KEYDOWN => {
                match wp as u32 {
                    0x1B => {
                        if s.usage_consent {
                            activate(hwnd, s, 22);
                        } else if s.detail.is_some() {
                            close_detail(s);
                        } else if s.confirmation.is_some() {
                            activate(hwnd, s, 8);
                        } else {
                            s.open = false;
                            s.focus = 1;
                        }
                    }
                    0x09 => {
                        s.focus = if s.usage_consent {
                            if s.focus == 22 {
                                23
                            } else {
                                22
                            }
                        } else if s.detail.is_some() {
                            match s.focus {
                                13 => 14,
                                14 => 15,
                                _ => 13,
                            }
                        } else if s.confirmation.is_some() {
                            if s.focus == 8 {
                                9
                            } else {
                                8
                            }
                        } else if s.open {
                            match s.focus {
                                1 => 2,
                                2 => 3,
                                3 => 10,
                                10 => 4,
                                4 => 5,
                                5 => 11,
                                11 => 12,
                                12 => 6,
                                _ => 1,
                            }
                        } else {
                            1
                        };
                    }
                    0x0D | 0x20 => activate(hwnd, s, s.focus.max(1)),
                    _ => {}
                }
                InvalidateRect(hwnd, null_mut(), 0);
                0
            }
            WM_KILLFOCUS => {
                s.focus = 0;
                InvalidateRect(hwnd, null_mut(), 0);
                0
            }
            _ => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }
    pub fn run() {
        unsafe {
            SetProcessDPIAware();
            let name = wide("MuislandWindow");
            let existing = FindWindowW(name.as_ptr(), null_mut());
            if !existing.is_null() {
                SetForegroundWindow(existing);
                return;
            }
            let inst = GetModuleHandleW(null_mut());
            let wc = WNDCLASSW {
                lpfnWndProc: Some(wndproc),
                hInstance: inst,
                hCursor: LoadCursorW(null_mut(), IDC_ARROW),
                lpszClassName: name.as_ptr(),
                ..zeroed()
            };
            if RegisterClassW(&wc) == 0 {
                return;
            }
            let mut work: RECT = zeroed();
            SystemParametersInfoW(SPI_GETWORKAREA, 0, (&mut work as *mut RECT).cast(), 0);
            let anchor = (work.left + work.right) as f32 / 2.;
            let top = work.top + 20;
            let preview = std::env::args().any(|a| a == "--preview");
            let hwnd = CreateWindowExW(
                WS_EX_TOPMOST
                    | if preview {
                        WS_EX_APPWINDOW
                    } else {
                        WS_EX_TOOLWINDOW
                    },
                name.as_ptr(),
                wide("Muisland").as_ptr(),
                WS_POPUP,
                (anchor - CW / 2.) as i32,
                top,
                CW as i32,
                CH as i32,
                null_mut(),
                null_mut(),
                inst,
                null_mut(),
            );
            if hwnd.is_null() {
                return;
            }
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
                            used: if i == 0 { 46. } else { 65. },
                            resets: Some(accounts::now() + 86400 * 3),
                        }),
                        updated: accounts::now(),
                        plan: "Demo".into(),
                    });
                }
            }
            let s = Box::new(State {
                open: preview,
                w: CW,
                h: CH,
                vw: 0.,
                vh: 0.,
                reveal: 0.,
                hover: 0,
                hot: [0.; 24],
                focus: 0,
                down: None,
                drag: false,
                origin: (0, 0),
                anchor,
                top,
                saved: if preview {
                    [true, true]
                } else {
                    [profiles::saved(1), profiles::saved(2)]
                },
                active: 0,
                status: String::new(),
                error: false,
                until: 0.,
                worker: None,
                pending: 0,
                confirmation: None,
                closing: false,
                started: 0.,
                clock: now,
                last: now,
                reduced: false,
                preview,
                settings,
                detail: None,
                name_edit: null_mut(),
                name_font: null_mut(),
                edit_brush: CreateSolidBrush(0x00212723),
                usage_worker: None,
                usage_errors: std::array::from_fn(|_| String::new()),
                usage_next: 0.,
                usage_consent: false,
                minute: accounts::now() / 60,
            });
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(s) as isize);
            let s = &*(GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const State);
            shape(hwnd, s);
            ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            SetTimer(hwnd, 17, 16, None);
            let mut m: MSG = zeroed();
            while GetMessageW(&mut m, null_mut(), 0, 0) > 0 {
                let state = &mut *(GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut State);
                if !state.name_edit.is_null()
                    && m.hwnd == state.name_edit
                    && m.message == WM_KEYDOWN
                {
                    if m.wParam == 0x0D || m.wParam == 0x1B {
                        SendMessageW(hwnd, WM_APP_NAME, if m.wParam == 0x0D { 1 } else { 0 }, 0);
                        continue;
                    }
                    if m.wParam == 0x09 {
                        state.focus = 14;
                        SetFocus(hwnd);
                        InvalidateRect(hwnd, null_mut(), 0);
                        continue;
                    }
                }
                TranslateMessage(&m);
                DispatchMessageW(&m);
            }
        }
    }
}
#[cfg(windows)]
fn main() {
    island::run();
}
