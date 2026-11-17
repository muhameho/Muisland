//! Window, state machine and account operations for the Windows island.
//! Drawing lives in `ui`; this module owns behaviour and timing.
use crate::{
    accounts::{self, Settings},
    anim::{approach, Spring},
    codex,
    gfx::{Gfx, Surface, R},
    i18n,
    polling::{self, PollState},
    profiles, system, theme,
    ui::{self, Mem},
    usage::{self, Usage, Window},
};
use std::{
    mem::zeroed,
    ptr::{null, null_mut},
    rc::Rc,
    sync::mpsc::{self, Receiver},
    time::Instant,
};
use windows_sys::Win32::{
    Foundation::*,
    Graphics::{Dwm::DwmFlush, Gdi::*},
    System::{
        DataExchange::{CloseClipboard, GetClipboardData, OpenClipboard},
        LibraryLoader::GetModuleHandleW,
        Memory::{GlobalLock, GlobalSize, GlobalUnlock},
    },
    UI::{
        HiDpi::*,
        Input::KeyboardAndMouse::{
            GetAsyncKeyState, GetKeyState, RegisterHotKey, ReleaseCapture, SetCapture,
            UnregisterHotKey, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT,
        },
        WindowsAndMessaging::*,
    },
};

/// Expanded panel size in logical units.
pub const PW: f32 = 392.;
pub const PH: f32 = 600.;
pub const CIRCLE: f32 = 68.;
pub const PILL_W: f32 = 236.;
pub const PILL_H: f32 = 60.;
/// Transparent room around the shape for its shadow.
pub const MARGIN: f32 = 24.;
/// Hover card size and its shadow margin.
pub const BW: f32 = 300.;
pub const BH: f32 = 222.;
pub const BMARGIN: f32 = 22.;
pub const HEADER: f32 = 58.;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Op {
    Save(usize),
    Switch(usize),
    Rollback,
    Recover,
}
#[derive(Clone, Debug, PartialEq)]
pub enum View {
    Home,
    Details(usize),
    Preferences,
    Notice(String, String),
    Confirm(Op),
    SwitchConfirm(usize),
    Remove(usize, String),
    AutoSwitch(usize),
    QuitConfirm,
    /// A settings category (see `ui::CATEGORIES`).
    Section(u8),
}
impl View {
    pub fn depth(&self) -> u8 {
        match self {
            View::Home => 0,
            View::Remove(..) | View::QuitConfirm | View::Section(_) => 2,
            _ => 1,
        }
    }
}
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Action {
    None,
    Toggle,
    Header,
    Preferences,
    ClaudeTab,
    CodexTab,
    Theme(usize),
    MenuSlider,
    FloatSlider,
    BubbleSlider,
    SessionThreshold,
    WeeklyThreshold,
    Style(bool),
    Center(bool),
    Metric(usize),
    Language(usize),
    UsageAlerts,
    AutoSwitch,
    PauseForWork,
    Motion,
    LaunchAtLogin,
    TrayIcon,
    Hotkey,
    /// Collapse and keep Muisland only in the notification area.
    ToTray,
    ShowIsland,
    Back,
    Details(usize),
    Save(usize),
    Switch(usize),
    Add,
    Protect,
    ShareSessions,
    /// Copies the request that makes Cowork write HANDOFF.md before a switch.
    CopyHandoff,
    /// Copies the request that continues from HANDOFF.md after a switch.
    CopyContinue,
    DismissHandoff,
    /// Picks the folder the Cowork task works in.
    PickFolder,
    /// Opens a new Cowork session (folder from the manual choice).
    OpenCowork,
    /// Opens a new Cowork session in the n-th project that saved HANDOFF.md.
    OpenProject(usize),
    Remove,
    RemoveConfirm,
    Rollback,
    Recover,
    /// Opens a settings category.
    Category(u8),
    OpenBackups,
    /// Asks before quitting.
    Quit,
    /// Quits for real (after confirmation, or when Windows asks the app to close).
    QuitNow,
    Confirm,
    Cancel,
    NameField,
    NameSave,
}
pub enum Outcome {
    Done(Op, String),
    NeedsClose(Op),
    NeedsSwitch(usize),
    Already(usize),
    /// Claude reopened signed out locally; the payload is the slot that kept its account.
    SignIn(usize),
    LoginRequired,
}
pub struct SyncResult {
    generation: u64,
    active: Option<usize>,
    samples: Vec<(usize, String, Result<Usage, usage::Failure>)>,
}
pub struct State {
    pub settings: Settings,
    pub saved: Vec<bool>,
    pub identities: Vec<Option<String>>,
    pub usage_errors: Vec<String>,
    pub active: Option<usize>,
    pub open: bool,
    pub peek: bool,
    peek_until: f32,
    pub bubble: HWND,
    pub bubble_t: Spring,
    pub w: Spring,
    pub h: Spring,
    /// Device pixels per logical unit, including monitor DPI.
    pub scale: f32,
    pub dpi: f32,
    pub anchor: f32,
    pub top: i32,
    pub float_position: (f32, i32),
    pub view: View,
    pub previous: Option<View>,
    pub back: bool,
    pub transition: f32,
    pub hover: Action,
    pub focus: Action,
    pub down: Option<(f32, f32, Action)>,
    pub drag: bool,
    pub slider_drag: Option<Action>,
    pub slider_preview: Option<f32>,
    pub pulse_until: f32,
    pub toast_until: f32,
    pub ring_session: Option<f64>,
    pub ring_weekly: Option<f64>,
    pub auto_due: Option<f32>,
    alerted: std::collections::HashSet<(usize, usize)>,
    mouse_was_down: bool,
    origin: (i32, i32),
    pub edit: Vec<u16>,
    pub caret: usize,
    pub selected: bool,
    pub edit_focus: bool,
    pub status: String,
    status_until: f32,
    pub error: bool,
    pub pending: Option<Op>,
    pub worker: Option<Receiver<Result<Outcome, String>>>,
    pub sync: Option<Receiver<SyncResult>>,
    sync_next: f32,
    polls: std::collections::HashMap<usize, PollState>,
    generation: u64,
    pub clock: Instant,
    last: Instant,
    last_second: u64,
    pub preview: bool,
    pub network_allowed: bool,
    pub work_areas: Vec<RECT>,
    pub mem: Mem,
    pub gfx: Option<Rc<Gfx>>,
    pub system_dark: bool,
    pub launch_at_login: bool,
    dirty: bool,
    tray_key: String,
    tray_hint_shown: bool,
    /// Backup folder summary, read when the Backup page opens.
    pub backup: Option<crate::backup::Summary>,
    /// Minimized to the tray for this run; the tray icon or shortcut restores it.
    in_tray: bool,
    /// Cowork handoff: the request was copied before a switch…
    pub handoff_copied: bool,
    /// …and the switch finished, so the continue banner is shown.
    pub handoff_ready: bool,
    /// The folder dialog is open: losing focus must not collapse the panel.
    picking: bool,
    folder_rx: Option<mpsc::Receiver<Option<String>>>,
    /// Project folders where HANDOFF.md was saved since the request was copied.
    pub handoff_folders: Vec<String>,
    handoff_since: Option<std::time::SystemTime>,
    handoff_rx: Option<mpsc::Receiver<std::path::PathBuf>>,
    /// When the panel was last opened from the tray, shortcut or hover card.
    opened_at: f32,
    hwnd: HWND,
    blink: u128,
    surface: Option<Surface>,
    bubble_surface: Option<Surface>,
    /// When focus loss last collapsed the panel (tray clicks cause this first).
    deactivated_at: f32,
    taskbar_created: u32,
}
impl State {
    /// Another saved entry that holds the same signed-in account, if any.
    pub fn duplicate_of(&self, i: usize) -> Option<usize> {
        let id = self.identities.get(i)?.as_ref()?;
        self.identities
            .iter()
            .enumerate()
            .find(|(j, other)| *j != i && other.as_ref() == Some(id))
            .map(|(j, _)| j)
    }
    /// Whether the floating circle or pill is shown while the panel is closed.
    pub fn island_visible(&self) -> bool {
        self.settings.show_island && !self.in_tray
    }
    pub fn theme(&self) -> &'static theme::Theme {
        theme::resolve(self.settings.theme, self.system_dark)
    }
    pub fn tr(&self, en: &'static str) -> &'static str {
        i18n::t(self.settings.language, en)
    }
    pub fn rtl(&self) -> bool {
        self.settings.language == 1
    }
    pub fn provider_name(&self) -> &'static str {
        if self.settings.provider == 1 {
            "Codex"
        } else {
            "Claude"
        }
    }
    pub fn active_usage(&self) -> Option<&Usage> {
        self.active
            .and_then(|i| self.settings.accounts.get(i))
            .and_then(|a| a.usage.as_ref())
    }
    /// Percentage shown on the floating island for the chosen metric.
    pub fn metric(&self) -> Option<f64> {
        let session = self.ring_session;
        let weekly = self.ring_weekly;
        match self.settings.circle_metric {
            1 => weekly,
            2 => match (session, weekly) {
                (Some(a), Some(b)) => Some(a.max(b)),
                (a, b) => a.or(b),
            },
            _ => session.or(weekly),
        }
    }
    pub fn metric_label(&self) -> &'static str {
        let session = self.ring_session.is_some();
        match self.settings.circle_metric {
            1 => self.tr(if self.settings.provider == 1 {
                "EXT"
            } else {
                "7D"
            }),
            2 => self.tr("MAX"),
            _ if !session => self.tr(if self.settings.provider == 1 {
                "EXT"
            } else {
                "7D"
            }),
            _ => self.tr(if self.settings.provider == 1 {
                "SES"
            } else {
                "5H"
            }),
        }
    }
    pub fn session_label(&self) -> &'static str {
        self.tr(if self.settings.provider == 1 {
            "Session"
        } else {
            "5-hour"
        })
    }
    pub fn weekly_label(&self) -> &'static str {
        self.tr(if self.settings.provider == 1 {
            "Extended"
        } else {
            "Weekly"
        })
    }
    pub fn now(&self) -> f32 {
        self.clock.elapsed().as_secs_f32()
    }
    pub fn collapsed_size(&self) -> (f32, f32) {
        if self.settings.circle {
            (CIRCLE, CIRCLE)
        } else {
            (PILL_W, PILL_H)
        }
    }
    /// 0 when collapsed, 1 when the panel is fully open.
    pub fn openness(&self) -> f32 {
        let (_, ch) = self.collapsed_size();
        ((self.h.value - ch) / (PH - ch)).clamp(0., 1.)
    }
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
fn monitor_dpi(x: f32, y: i32) -> f32 {
    unsafe {
        let monitor = MonitorFromPoint(
            POINT {
                x: x as i32,
                y: y + 4,
            },
            MONITOR_DEFAULTTONEAREST,
        );
        let (mut dx, mut dy) = (96u32, 96u32);
        if monitor.is_null() || GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dx, &mut dy) != 0
        {
            return 96.;
        }
        dx.clamp(48, 480) as f32
    }
}
/// Fits a `w`×`h` logical shape at `scale` inside the nearest work area.
pub fn bounded_geometry(
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
fn target_scale(s: &State) -> f32 {
    let user = if s.open {
        s.settings.menu_scale
    } else {
        s.settings.float_scale
    };
    user * s.dpi / 96.
}
pub fn constrain(s: &mut State) {
    let (fw, fh) = s.collapsed_size();
    let float_scale = s.settings.float_scale * s.dpi / 96.;
    let (fa, ft, _) = bounded_geometry(
        &s.work_areas,
        s.float_position.0,
        s.float_position.1,
        fw,
        fh,
        float_scale,
    );
    s.float_position = (fa, ft);
    // Menu fitting must never overwrite the preferred floating location.
    let (desired_x, desired_y) = if s.open {
        (s.anchor, s.top)
    } else {
        s.float_position
    };
    let (anchor, top, scale) = bounded_geometry(
        &s.work_areas,
        desired_x,
        desired_y,
        s.w.value,
        s.h.value,
        s.scale,
    );
    s.anchor = anchor;
    s.top = top;
    s.scale = scale;
}

#[derive(Debug)]
pub struct BubblePlacement {
    pub x: i32,
    pub y: i32,
    pub scale: f32,
    pub right: bool,
}
/// Places the hover card beside the island inside the nearest work area.
/// `width` is the island's physical width and `scale` the requested card scale.
pub fn bubble_placement(
    areas: &[RECT],
    cx: f32,
    top: i32,
    width: f32,
    height: f32,
    requested: f32,
) -> Option<BubblePlacement> {
    let cy = top as f32 + height / 2.;
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
    let gap = 10.;
    let room_right = (right - cx - width / 2. - gap).max(1.);
    let room_left = (cx - width / 2. - gap - left).max(1.);
    let preferred_right = cx < (left + right) / 2.;
    let right_side = if preferred_right {
        room_right >= BW * requested || room_right >= room_left
    } else {
        !(room_left >= BW * requested || room_left >= room_right)
    };
    let scale = requested
        .min(if right_side { room_right } else { room_left } / BW)
        .min((bottom - upper).max(1.) / BH)
        .max(0.01);
    let w = (BW * scale).round();
    let h = (BH * scale).round();
    let x = if right_side {
        cx + width / 2. + gap
    } else {
        cx - width / 2. - gap - w
    };
    let y = (cy - h / 2.).clamp(upper, (bottom - h).max(upper));
    Some(BubblePlacement {
        x: x.round() as i32,
        y: y.round() as i32,
        scale,
        right: right_side,
    })
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}
pub fn status(s: &mut State, message: impl Into<String>, error: bool) {
    s.status = message.into();
    s.error = error;
    s.status_until = s.now() + 6.;
    s.dirty = true;
}
fn persist(s: &mut State) {
    s.dirty = true;
    s.settings.position = Some(s.float_position);
    if !s.preview && s.settings.save().is_err() {
        let m = s.tr("Could not save your preferences");
        status(s, m, true);
    }
}
pub fn discard_drafts(s: &mut State) {
    // Worker outcomes carry account indices. Keep them stable until completion.
    if s.worker.is_some() || !s.settings.accounts.iter().any(|a| a.draft) {
        return;
    }
    let active_slot = s
        .active
        .and_then(|i| s.settings.accounts.get(i))
        .map(|a| a.slot);
    s.settings.accounts.retain(|a| !a.draft);
    s.active = active_slot.and_then(|slot| s.settings.accounts.iter().position(|a| a.slot == slot));
    s.generation += 1;
    s.sync = None;
    s.sync_next = 0.;
    s.polls
        .retain(|slot, _| s.settings.accounts.iter().any(|a| a.slot == *slot));
    refresh_slots(s);
    s.view = View::Home;
    s.previous = None;
    s.edit_focus = false;
    s.focus = Action::None;
}
pub fn collapse(s: &mut State) {
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
    s.dirty = true;
}
pub fn drag_zone(s: &State, x: f32, y: f32) -> bool {
    let (x, y) = (x - MARGIN, y - MARGIN);
    if !s.open {
        return true;
    }
    let lx = if s.rtl() { PW - x } else { x };
    y >= 0. && y < HEADER && lx >= 0. && lx < PW - 100.
}
pub fn navigate(s: &mut State, next: View, backwards: bool) {
    if !matches!(next, View::AutoSwitch(_)) {
        s.auto_due = None;
    }
    let returning = matches!(next, View::Preferences) && matches!(s.view, View::QuitConfirm);
    if matches!(next, View::Preferences | View::Section(_)) && next != s.view && !returning {
        s.mem.reset_scroll(1);
    }
    s.slider_drag = None;
    s.slider_preview = None;
    s.back = backwards || next.depth() < s.view.depth();
    s.previous = Some(s.view.clone());
    s.view = next;
    s.transition = if s.settings.reduced_motion { 1. } else { 0. };
    s.edit_focus = false;
    s.focus = Action::None;
    s.open = true;
    s.peek = false;
    s.dirty = true;
}
pub fn notice(s: &mut State, title: &str, body: impl Into<String>, error: bool) {
    s.error = error;
    s.status.clear();
    navigate(s, View::Notice(title.into(), body.into()), false);
}
pub fn refresh_slots(s: &mut State) {
    s.saved = (0..s.settings.accounts.len())
        .map(|i| {
            if s.settings.accounts[i].draft {
                false
            } else if s.preview {
                true
            } else if s.settings.provider == 1 {
                codex::saved(s.settings.accounts[i].slot)
            } else {
                profiles::saved(s.settings.accounts[i].slot)
            }
        })
        .collect();
    s.identities = (0..s.settings.accounts.len())
        .map(|i| {
            if s.settings.accounts[i].draft {
                None
            } else if s.preview {
                Some(format!("preview-{}", s.settings.accounts[i].slot))
            } else if s.settings.provider == 1 {
                codex::account_identity(s.settings.accounts[i].slot).ok()
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
    let now = s.now();
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
        } else if provider == 1 {
            codex::current_identity().ok()
        } else {
            usage::current_account_identity().ok()
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
                let Some(id) = id else { continue };
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
        let _ = tx.send(SyncResult {
            generation,
            active,
            samples,
        });
    });
}
pub fn begin(s: &mut State, op: Op, confirmed: bool) {
    if s.worker.is_some() {
        let m = s.tr("Please wait for the current action");
        status(s, m, false);
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
    let share = s.settings.share_sessions;
    let handoff = (s.settings.provider == 0 && s.handoff_copied).then(|| {
        let folder = s
            .handoff_folders
            .first()
            .or(s.settings.handoff_folder.as_ref());
        cowork_link(None, folder.map(String::as_str))
    });
    let provider = s.settings.provider;
    let slot = match op {
        Op::Save(i) | Op::Switch(i) => s.settings.accounts[i].slot,
        _ => 0,
    };
    let names: Vec<(usize, String)> = s
        .settings
        .accounts
        .iter()
        .map(|a| (a.slot, a.name.clone()))
        .collect();
    let (tx, rx) = mpsc::channel();
    s.worker = Some(rx);
    s.pending = Some(op);
    let m = s.tr(if confirmed {
        "Closing Claude, then continuing…"
    } else {
        "Checking your account…"
    });
    status(s, m, false);
    let lang = s.settings.language;
    let tr = move |en: &'static str| i18n::t(lang, en).to_string();
    std::thread::spawn(move || {
        let result = (|| -> Result<Outcome, String> {
            if let Some(expected) = expected_source.as_ref() {
                let current = if provider == 1 {
                    codex::current_identity().ok()
                } else {
                    usage::current_account_identity().ok()
                };
                if !preview && current.as_ref() != Some(expected) {
                    return Err(tr("Signed-in account changed. Auto-switch cancelled."));
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
                    tr("Preview complete · no account changes"),
                ));
            }
            if provider == 1 {
                // Codex cannot be sent to a sign-in screen from here, so saving an
                // account that another entry already holds would only duplicate it.
                if matches!(op, Op::Save(_)) {
                    if let Some(other) = codex::current_identity()
                        .ok()
                        .and_then(|id| codex::slot_holding(&id, slot))
                    {
                        let name = names
                            .iter()
                            .find(|(s, _)| *s == other)
                            .map(|(_, n)| n.clone())
                            .unwrap_or_else(|| tr("another account"));
                        return Err(tr("This Codex account is already saved as {name}. Sign in to a different account in Codex, then choose Save again.").replace("{name}", &name));
                    }
                }
                match op {
                    Op::Save(_) => codex::save(slot),
                    Op::Switch(_) => codex::switch(slot),
                    Op::Rollback => codex::rollback(),
                    Op::Recover => Err(std::io::Error::other(tr("Codex switches only its sign-in cache. Chats and working folders are not moved or restored."))),
                }
                .map_err(|e| e.to_string())?;
                return Ok(Outcome::Done(
                    op,
                    tr(if matches!(op, Op::Switch(_)) {
                        "Codex sign-in switched · restart existing Codex sessions to reload"
                    } else {
                        "Codex sign-in saved"
                    }),
                ));
            }
            let launch = profiles::claude_launch_target();
            let expected = if matches!(op, Op::Save(_)) {
                Some(match usage::current_account_identity() {
                    Ok(identity) => identity,
                    Err(_) => {
                        launch.open().map_err(|_| {
                            tr("Open Claude Desktop, sign in, then choose Save again.")
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
                let prepared = profiles::sign_in_another(other, expected.as_deref().unwrap_or(""));
                let opened = launch.open();
                prepared.map_err(|e| e.to_string())?;
                opened.map_err(|_| {
                    tr("Claude is ready for another sign-in but could not reopen. Open Claude manually.")
                })?;
                return Ok(Outcome::SignIn(other));
            }
            match op {
                Op::Save(_) => {
                    profiles::save_and_reopen(slot, expected.as_deref().unwrap_or(""), &launch)
                        .map_err(|e| e.to_string())?;
                    Ok(Outcome::Done(op, tr("Account saved · Claude reopened")))
                }
                Op::Switch(_) => {
                    let switched = profiles::switch(slot, protect, share);
                    // Starting Claude through the link opens the Cowork handoff
                    // directly (a cold start, which also commits the prefilled text).
                    let reopened = match handoff.as_deref() {
                        Some(link) if switched.is_ok() => {
                            profiles::open_link(link).or_else(|_| launch.open())
                        }
                        _ => launch.open(),
                    };
                    let shared = switched.map_err(|e| e.to_string())?;
                    reopened.map_err(|_| {
                        tr("Account switched, but Claude could not reopen. Open Claude manually.")
                    })?;
                    let n = shared.map(|s| s.added + s.updated).unwrap_or(0);
                    Ok(Outcome::Done(
                        op,
                        if n > 0 {
                            tr("Switched · {n} Code sessions shared").replace("{n}", &n.to_string())
                        } else {
                            tr("Switched successfully")
                        },
                    ))
                }
                Op::Rollback => {
                    let restored = profiles::restore_backup();
                    let reopened = launch.open();
                    restored.map_err(|e| e.to_string())?;
                    reopened.map_err(|_| {
                        tr("Previous sign-in restored, but Claude could not reopen. Open Claude manually.")
                    })?;
                    Ok(Outcome::Done(
                        op,
                        tr("Previous sign-in restored · Claude reopened"),
                    ))
                }
                Op::Recover => {
                    let path = crate::backup::recover_latest().map_err(|e| e.to_string())?;
                    let _ = std::process::Command::new("explorer.exe").arg(path).spawn();
                    Ok(Outcome::Done(
                        op,
                        tr("Backup recovered to a separate folder"),
                    ))
                }
            }
        })();
        let _ = tx.send(result);
    });
}
fn threshold(value: f32) -> f64 {
    (value * 100.).round() as f64
}
pub fn threshold_hit(s: &State, i: usize) -> bool {
    let now = accounts::now();
    s.settings
        .accounts
        .get(i)
        .and_then(|a| a.usage.as_ref())
        .is_some_and(|u| {
            now.saturating_sub(u.updated) <= 90
                && (u.session.as_ref().is_some_and(|w| {
                    w.resets.is_none_or(|r| r > now)
                        && w.used >= threshold(s.settings.session_threshold)
                }) || u.weekly.as_ref().is_some_and(|w| {
                    w.resets.is_none_or(|r| r > now)
                        && w.used >= threshold(s.settings.weekly_threshold)
                }))
        })
}
pub fn auto_target(s: &State) -> Option<usize> {
    let active = s.active?;
    if !threshold_hit(s, active) || !s.usage_errors.get(active).is_some_and(|e| e.is_empty()) {
        return None;
    }
    let now = accounts::now();
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
                    .is_some_and(|id| Some(id) != s.identities.get(active).and_then(|v| v.as_ref()))
        })
        .filter_map(|(i, a)| {
            let u = a.usage.as_ref()?;
            let x = u.session.as_ref()?;
            let y = u.weekly.as_ref()?;
            if now.saturating_sub(u.updated) > 300
                || x.resets.is_some_and(|r| r <= now)
                || y.resets.is_some_and(|r| r <= now)
                || x.used >= threshold(s.settings.session_threshold)
                || y.used >= threshold(s.settings.weekly_threshold)
            {
                return None;
            }
            Some((i, x.used.max(y.used)))
        })
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|v| v.0)
}
pub fn evaluate_limits(s: &mut State) {
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
    s.pulse_until = s.now() + 2.4;
    if s.settings.auto_switch {
        if let Some(target) = auto_target(s) {
            let may_work = s.settings.provider == 1 || (!s.preview && profiles::claude_running());
            s.auto_due = if s.settings.pause_for_work && may_work {
                None
            } else {
                Some(s.now() + 15.)
            };
            navigate(s, View::AutoSwitch(target), false);
            s.focus = Action::Cancel;
            return;
        }
    }
    let name = s.settings.accounts[i].name.clone();
    let body = format!(
        "{} {}",
        s.tr("{name} reached a configured usage threshold.")
            .replace("{name}", &name),
        s.tr(if s.settings.auto_switch {
            "No saved account has fresh, lower usage in both windows. No switch was made."
        } else {
            "Open accounts to review usage or switch."
        })
    );
    if !s.open && s.settings.tray && !s.preview {
        system::tray_balloon(s.hwnd, s.tr("Usage alert"), &body);
    }
    let title = s.tr("Usage alert");
    notice(s, title, body, false);
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
                    let m = s.tr("Could not save the account name");
                    status(s, m, true);
                } else {
                    navigate(s, View::Home, true);
                    let m = s.tr("Account name saved");
                    status(s, m, false);
                }
            }
            Err(_) => {
                let m = s.tr("Use a name of 1–32 characters.");
                status(s, m, true)
            }
        }
    }
}
pub fn remove_account(s: &mut State, index: usize) {
    if s.worker.is_some() {
        let m = s.tr("Wait for the current action before removing an account");
        status(s, m, false);
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
                let title = s.tr("Could not remove account");
                notice(s, title, e.to_string(), true);
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
        let title = s.tr("Could not remove account");
        let body = s.tr("Preferences could not be saved. Your account entry has been kept.");
        notice(s, title, body, true);
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
    s.active = active_slot.and_then(|slot| s.settings.accounts.iter().position(|a| a.slot == slot));
    navigate(s, View::Home, true);
    let m = s.tr("Account removed from Muisland");
    status(s, m, false);
}
pub fn slider_bounds(action: Action) -> Option<(f32, f32)> {
    match action {
        Action::MenuSlider => Some((0.75, 1.6)),
        Action::FloatSlider => Some((0.6, 1.8)),
        Action::BubbleSlider => Some((0.85, 1.6)),
        Action::SessionThreshold | Action::WeeklyThreshold => Some((0.05, 1.0)),
        _ => None,
    }
}
pub fn slider_value(s: &State, action: Action) -> f32 {
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
/// Converts a pointer x (window-logical) into a slider value using the drawn track.
pub fn slider_from_x(s: &State, action: Action, x: f32) -> f32 {
    let (lo, hi) = slider_bounds(action).unwrap_or((0., 1.));
    let track = s.mem.slider_track(action).unwrap_or(R {
        x: 0.,
        y: 0.,
        w: 1.,
        h: 1.,
    });
    let mut t = ((x - track.x) / track.w.max(1.)).clamp(0., 1.);
    if s.rtl() {
        t = 1. - t;
    }
    let value = lo + t * (hi - lo);
    ((value / 0.05).round() * 0.05).clamp(lo, hi)
}
pub fn set_slider(s: &mut State, action: Action, value: f32) {
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
    s.dirty = true;
}
pub fn hit(s: &State, x: f32, y: f32) -> Action {
    s.mem
        .hits
        .iter()
        .rev()
        .find(|(r, _)| r.contains(x, y))
        .map(|(_, a)| *a)
        .unwrap_or(Action::None)
}
fn select_provider(s: &mut State, provider: usize) {
    if s.worker.is_some() {
        let m = s.tr("Wait for the current operation before changing apps");
        status(s, m, false);
        return;
    }
    if s.settings.provider == provider {
        return;
    }
    discard_drafts(s);
    s.settings.select_provider(provider);
    s.generation += 1;
    s.sync = None;
    s.sync_next = 0.;
    s.polls.clear();
    s.active = None;
    s.ring_session = None;
    s.ring_weekly = None;
    s.mem.reset_scroll(0);
    refresh_slots(s);
    persist(s);
    if !matches!(s.view, View::Home) {
        navigate(s, View::Home, true);
    }
    // A prior confirmation may refer to another provider's slot.
    s.previous = None;
    s.transition = 1.;
}
pub fn activate(s: &mut State, a: Action) {
    if !matches!(a, Action::Confirm) {
        s.auto_due = None;
    }
    s.dirty = true;
    match a {
        Action::None => {}
        Action::ClaudeTab => select_provider(s, 0),
        Action::CodexTab => select_provider(s, 1),
        Action::MenuSlider
        | Action::FloatSlider
        | Action::BubbleSlider
        | Action::SessionThreshold
        | Action::WeeklyThreshold => {}
        Action::Style(circle) => {
            if s.settings.circle != circle {
                s.settings.circle = circle;
                persist(s)
            }
        }
        Action::Center(numbers) => {
            s.settings.circle_numbers = numbers;
            persist(s);
        }
        Action::Metric(m) => {
            s.settings.circle_metric = m.min(2);
            persist(s);
        }
        Action::Language(l) => {
            s.settings.language = l.min(i18n::LANGUAGES.len() - 1);
            s.settings.language_unset = false;
            s.tray_key.clear();
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
        Action::LaunchAtLogin => {
            let want = !s.launch_at_login;
            if s.preview {
                s.launch_at_login = want;
            } else {
                match system::set_launch_at_login(want) {
                    Ok(()) => s.launch_at_login = system::launch_at_login(),
                    Err(_) => {
                        let m = s.tr("Could not change the startup setting");
                        status(s, m, true)
                    }
                }
            }
        }
        Action::TrayIcon => {
            s.settings.tray = !s.settings.tray;
            if !s.settings.tray {
                s.settings.show_island = true;
                s.in_tray = false;
            }
            apply_system(s);
            persist(s);
        }
        Action::Hotkey => {
            s.settings.hotkey = !s.settings.hotkey;
            apply_system(s);
            persist(s);
        }
        Action::ToTray => minimize_to_tray(s),
        Action::ShowIsland => {
            if s.settings.show_island {
                // Hiding the island needs the tray icon as the way back.
                s.settings.tray = true;
                s.settings.show_island = false;
                apply_system(s);
            } else {
                s.settings.show_island = true;
                s.in_tray = false;
            }
            persist(s);
        }
        Action::Theme(index) => {
            s.settings.theme = index.min(theme::COUNT - 1);
            s.tray_key.clear();
            persist(s);
        }
        Action::Toggle | Action::Header => {
            if s.open {
                collapse(s)
            } else {
                s.open = true;
                s.peek = false;
                s.edit_focus = false;
            }
        }
        Action::Preferences => {
            if matches!(s.view, View::Preferences | View::Section(_)) {
                navigate(s, View::Home, true)
            } else {
                navigate(s, View::Preferences, false)
            }
        }
        Action::Back | Action::Cancel => {
            discard_drafts(s);
            match s.view.clone() {
                View::Remove(i, _) if i < s.settings.accounts.len() => {
                    open_details(s, i, true);
                }
                View::QuitConfirm if matches!(s.previous, Some(View::Preferences)) => {
                    navigate(s, View::Preferences, true)
                }
                View::Section(_) => navigate(s, View::Preferences, true),
                _ => navigate(s, View::Home, true),
            }
        }
        Action::Details(i) => open_details(s, i, false),
        Action::NameField => {
            s.edit_focus = true;
            s.caret = s.edit.len();
            s.selected = true;
        }
        Action::NameSave => name_save(s),
        Action::Remove => {
            if let View::Details(i) = s.view {
                if s.worker.is_some() {
                    let m = s.tr("Wait for the current action");
                    status(s, m, false);
                } else if s.settings.accounts[i].draft {
                    discard_drafts(s);
                    navigate(s, View::Home, true);
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
                s.mem.scroll_to_end(0);
                open_details(s, i, false);
            } else {
                let title = s.tr("Account slots full");
                let body = s
                    .tr("You can save up to {n} accounts on this device.")
                    .replace("{n}", &accounts::MAX_ACCOUNTS.to_string());
                notice(s, title, body, false);
            }
        }
        Action::Protect => {
            s.settings.protect_local = !s.settings.protect_local;
            persist(s)
        }
        Action::CopyHandoff => {
            let text = s.tr(HANDOFF_REQUEST);
            copy_text(s, text);
            if !s.handoff_copied {
                s.handoff_folders.clear();
                s.handoff_since = Some(std::time::SystemTime::now());
            }
            watch_handoffs(s);
            s.handoff_copied = true;
            let m = s.tr("Copied · paste it in Cowork and wait for HANDOFF.md");
            status(s, m, false);
        }
        Action::CopyContinue => {
            let text = s.tr(CONTINUE_REQUEST);
            copy_text(s, text);
            s.handoff_ready = false;
            s.handoff_copied = false;
            let m = s.tr("Copied · paste it in Cowork, in the same folder");
            status(s, m, false);
        }
        Action::PickFolder => pick_folder(s),
        Action::OpenProject(k) => {
            let text = s.tr(CONTINUE_REQUEST);
            copy_text(s, text);
            if let Some(folder) = s.handoff_folders.get(k).cloned() {
                if s.preview || profiles::open_link(&cowork_link(None, Some(&folder))).is_err() {
                    let m = s.tr("Could not open Claude");
                    status(s, m, !s.preview);
                } else {
                    collapse(s);
                }
            }
        }
        Action::OpenCowork => {
            let text = s.tr(CONTINUE_REQUEST);
            copy_text(s, text);
            let link = cowork_link(None, s.settings.handoff_folder.as_deref());
            if s.preview || profiles::open_link(&link).is_err() {
                let m = s.tr("Could not open Claude");
                status(s, m, !s.preview);
            } else {
                s.handoff_ready = false;
                s.handoff_copied = false;
                collapse(s);
            }
        }
        Action::DismissHandoff => {
            s.handoff_ready = false;
            s.handoff_copied = false;
            s.handoff_folders.clear();
            s.handoff_since = None;
        }
        Action::ShareSessions => {
            s.settings.share_sessions = !s.settings.share_sessions;
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
                let m = s.tr("Wait for the current action before quitting");
                status(s, m, false)
            } else if !matches!(s.view, View::QuitConfirm) {
                navigate(s, View::QuitConfirm, false);
                s.focus = Action::Cancel;
            }
        }
        Action::Category(i) => {
            if i == 4 {
                s.backup = Some(crate::backup::summary());
            }
            navigate(s, View::Section(i), false);
        }
        Action::OpenBackups => match crate::backup::folder() {
            Ok(path) => {
                let _ = std::process::Command::new("explorer").arg(path).spawn();
            }
            Err(e) => {
                let title = s.tr("Couldn't finish this action");
                notice(s, title, e.to_string(), true);
            }
        },
        Action::QuitNow => {
            if s.worker.is_some() {
                let m = s.tr("Wait for the current action before quitting");
                status(s, m, false)
            } else if !s.hwnd.is_null() {
                unsafe { DestroyWindow(s.hwnd) };
            }
        }
    }
}
fn open_details(s: &mut State, i: usize, backwards: bool) {
    s.edit = s.settings.accounts[i].name.encode_utf16().collect();
    s.caret = s.edit.len();
    s.selected = true;
    navigate(s, View::Details(i), backwards);
    s.edit_focus = true;
}
fn replace_selection(s: &mut State) {
    if s.selected {
        s.edit.clear();
        s.caret = 0;
        s.selected = false;
    }
}
/// Sent to Cowork before a switch; the Arabic version comes from i18n.
pub const HANDOFF_REQUEST: &str = "I'm about to switch Claude accounts and continue this task in a new Cowork session. Save a file named HANDOFF.md at the top level of the folder you are working in (not in a subfolder), with: the goal; what is done (which files changed and where); what is left, as a checklist; important decisions and constraints; and any commands or links needed to continue. Make it self-contained so a new session can continue without this conversation. Reply only once the file is saved.";
pub const CONTINUE_REQUEST: &str = "Read HANDOFF.md in this folder and continue the task from where it stopped. First summarize the current state in two lines, then carry on with the next unchecked item, and update HANDOFF.md as you finish items.";
/// `claude://cowork/new?folder=…` opens a new Cowork session in that folder
/// (Claude asks before using it). A `q=` message would be typed in too, but
/// Claude marks every prompt that arrives from a link with a red caution
/// note, so Muisland copies the request to the clipboard instead and leaves
/// `message` empty in the app.
pub fn cowork_link(message: Option<&str>, folder: Option<&str>) -> String {
    fn enc(s: &str) -> String {
        let mut out = String::with_capacity(s.len() * 3);
        for b in s.bytes() {
            match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                    out.push(b as char)
                }
                _ => out.push_str(&format!("%{b:02X}")),
            }
        }
        out
    }
    let mut params = Vec::new();
    if let Some(message) = message.filter(|m| !m.is_empty()) {
        params.push(format!("q={}", enc(message)));
    }
    if let Some(folder) = folder.filter(|f| !f.is_empty()) {
        params.push(format!("folder={}", enc(folder)));
    }
    if params.is_empty() {
        "claude://cowork/new".into()
    } else {
        format!("claude://cowork/new?{}", params.join("&"))
    }
}
const FOLDER_PICKED: u32 = WM_APP + 44;
const HANDOFF_FOUND: u32 = WM_APP + 45;
/// Starts one watcher per fixed drive (once per run); they report every
/// HANDOFF.md written while a handoff is in progress.
fn watch_handoffs(s: &mut State) {
    if s.preview || s.hwnd.is_null() || s.handoff_rx.is_some() {
        return;
    }
    let (tx, rx) = mpsc::channel();
    s.handoff_rx = Some(rx);
    let owner = s.hwnd as usize;
    for drive in crate::handoff::fixed_drives() {
        let tx = tx.clone();
        std::thread::spawn(move || {
            crate::handoff::watch(drive, tx, move || unsafe {
                PostMessageW(owner as HWND, HANDOFF_FOUND, 0, 0);
            })
        });
    }
}
/// Records project folders from the watchers while a handoff is in progress.
pub fn collect_handoffs(s: &mut State) {
    let Some(rx) = s.handoff_rx.as_ref() else { return };
    let paths: Vec<std::path::PathBuf> = rx.try_iter().collect();
    let Some(since) = s.handoff_since.filter(|_| s.handoff_copied) else {
        return;
    };
    for path in paths {
        let Some(folder) = crate::handoff::project_folder(&path) else { continue };
        if !crate::handoff::fresh(&path, since) {
            continue;
        }
        let folder = folder.to_string_lossy().into_owned();
        if !s.handoff_folders.iter().any(|f| f.eq_ignore_ascii_case(&folder)) {
            s.handoff_folders.push(folder);
            s.dirty = true;
        }
    }
}
/// Shows the Windows folder dialog on its own thread so the panel keeps drawing.
fn pick_folder(s: &mut State) {
    if s.preview || s.hwnd.is_null() || s.picking {
        return;
    }
    let (tx, rx) = mpsc::channel();
    s.folder_rx = Some(rx);
    s.picking = true;
    let owner = s.hwnd as usize;
    let title: Vec<u16> = s
        .tr("Choose the folder your Cowork task works in")
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    std::thread::spawn(move || {
        use windows_sys::Win32::System::Com::{CoInitializeEx, CoTaskMemFree, COINIT_APARTMENTTHREADED};
        use windows_sys::Win32::UI::Shell::{SHBrowseForFolderW, SHGetPathFromIDListW, BROWSEINFOW};
        let mut chosen = None;
        unsafe {
            CoInitializeEx(std::ptr::null(), COINIT_APARTMENTTHREADED as _);
            let mut name = [0u16; 260];
            let info = BROWSEINFOW {
                hwndOwner: owner as HWND,
                pidlRoot: std::ptr::null_mut(),
                pszDisplayName: name.as_mut_ptr(),
                lpszTitle: title.as_ptr(),
                // Only file-system folders; resizable dialog with a "Make New Folder" button.
                ulFlags: 0x0001 | 0x0040,
                lpfn: None,
                lParam: 0,
                iImage: 0,
            };
            let pidl = SHBrowseForFolderW(&info);
            if !pidl.is_null() {
                let mut path = vec![0u16; 32768];
                if SHGetPathFromIDListW(pidl, path.as_mut_ptr()) != 0 {
                    let n = path.iter().position(|c| *c == 0).unwrap_or(0);
                    chosen = Some(String::from_utf16_lossy(&path[..n]));
                }
                CoTaskMemFree(pidl as *const _);
            }
        }
        let _ = tx.send(chosen.filter(|p| !p.is_empty()));
        unsafe { PostMessageW(owner as HWND, FOLDER_PICKED, 0, 0) };
    });
}
fn copy_text(s: &State, text: &str) {
    if s.preview || s.hwnd.is_null() {
        return;
    }
    use windows_sys::Win32::System::DataExchange::{EmptyClipboard, SetClipboardData};
    use windows_sys::Win32::Foundation::GlobalFree;
    use windows_sys::Win32::System::Memory::{GlobalAlloc, GMEM_MOVEABLE};
    let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        if OpenClipboard(s.hwnd) == 0 {
            return;
        }
        EmptyClipboard();
        let h = GlobalAlloc(GMEM_MOVEABLE, wide.len() * 2);
        if !h.is_null() {
            let ptr = GlobalLock(h) as *mut u16;
            if ptr.is_null() {
                GlobalFree(h);
            } else {
                std::ptr::copy_nonoverlapping(wide.as_ptr(), ptr, wide.len());
                GlobalUnlock(h);
                // The clipboard owns the memory once this succeeds.
                if SetClipboardData(13, h).is_null() {
                    GlobalFree(h);
                }
            }
        }
        CloseClipboard();
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
/// Collapses the panel and hides the floating island until the tray icon
/// (or Ctrl+Alt+M, when enabled) restores it. The "Show floating island"
/// preference is left as it is.
pub fn minimize_to_tray(s: &mut State) {
    if !s.settings.tray {
        s.settings.tray = true;
        apply_system(s);
        persist(s);
    }
    s.in_tray = true;
    collapse(s);
    if !s.tray_hint_shown && !s.preview && !s.hwnd.is_null() {
        s.tray_hint_shown = true;
        let body = s.tr(if s.settings.hotkey {
            "Click its icon or press Ctrl+Alt+M to open it."
        } else {
            "Click its icon to open it."
        });
        system::tray_balloon(s.hwnd, s.tr("Muisland is in the tray"), body);
    }
}
/// Brings Muisland back from the tray: the island reappears where it was.
pub fn restore_from_tray(s: &mut State) {
    s.in_tray = false;
    s.dirty = true;
}
/// Applies the tray icon and global shortcut preferences.
fn apply_system(s: &mut State) {
    if s.hwnd.is_null() || s.preview {
        return;
    }
    unsafe {
        UnregisterHotKey(s.hwnd, HOTKEY_ID);
        if s.settings.hotkey {
            RegisterHotKey(
                s.hwnd,
                HOTKEY_ID,
                MOD_CONTROL | MOD_ALT | MOD_NOREPEAT,
                b'M' as u32,
            );
        }
    }
    if s.settings.tray {
        s.tray_key.clear();
        update_tray(s);
    } else {
        system::tray_remove(s.hwnd);
        s.tray_key.clear();
    }
}
fn update_tray(s: &mut State) {
    if !s.settings.tray || s.hwnd.is_null() || s.preview {
        return;
    }
    let metric = s.metric().map(|v| v.round() as i32);
    let key = format!(
        "{:?}{}{}{}",
        metric, s.settings.theme, s.system_dark, s.settings.language
    );
    if key == s.tray_key {
        return;
    }
    let Some(gfx) = s.gfx.clone() else { return };
    let size = unsafe { GetSystemMetrics(SM_CXSMICON) }.clamp(16, 64);
    let tip = match (s.active.and_then(|i| s.settings.accounts.get(i)), metric) {
        (Some(a), Some(m)) => format!("Muisland · {} · {}%", a.name, m),
        (Some(a), None) => format!("Muisland · {}", a.name),
        _ => "Muisland".to_string(),
    };
    if let Some(icon) = ui::tray_icon(s, &gfx, size) {
        if system::tray_set(s.hwnd, &icon, &tip) {
            s.tray_key = key;
        }
    }
}
const HOTKEY_ID: i32 = 0x4D49;
const BUBBLE_OPEN: u32 = WM_APP + 42;
const REFOCUS: u32 = WM_APP + 43;

/// Window-logical pointer position.
fn local(s: &State, hwnd: HWND, p: POINT) -> (f32, f32) {
    let mut p = p;
    unsafe { ScreenToClient(hwnd, &mut p) };
    (p.x as f32 / s.scale, p.y as f32 / s.scale)
}
fn lparam_point(lp: LPARAM, scale: f32) -> (f32, f32) {
    let x = (lp as u32 & 65535) as i16 as f32;
    let y = ((lp as u32 >> 16) & 65535) as i16 as f32;
    (x / scale, y / scale)
}
fn inside_shape(s: &State, x: f32, y: f32) -> bool {
    let (w, h) = (s.w.value, s.h.value);
    let (x, y) = (x - MARGIN, y - MARGIN);
    if s.settings.circle && !s.open {
        let rad = w.min(h) / 2.;
        (x - w / 2.).powi(2) + (y - h / 2.).powi(2) < rad * rad
    } else {
        x >= 0. && y >= 0. && x < w && y < h
    }
}

/// Advances every spring and easing by `dt`. Shared by the window loop and
/// the offline motion export so both produce identical motion.
pub fn animate(s: &mut State, dt: f32, t: f32) -> bool {
    let (tw, th) = if s.open { (PW, PH) } else { s.collapsed_size() };
    let motion = !s.settings.reduced_motion;
    let mut moving = false;
    // Usage rings ease toward fresh readings.
    let old_rings = (s.ring_session, s.ring_weekly);
    let usage = s.active_usage().cloned();
    for (display, target) in [
        (
            &mut s.ring_session,
            usage
                .as_ref()
                .and_then(|u| u.session.as_ref())
                .map(|w| w.used),
        ),
        (
            &mut s.ring_weekly,
            usage
                .as_ref()
                .and_then(|u| u.weekly.as_ref())
                .map(|w| w.used),
        ),
    ] {
        *display = target.map(|target| {
            let old = display.unwrap_or(if motion { 0. } else { target });
            if !motion || (target - old).abs() < 0.05 {
                target
            } else {
                old + (target - old) * (1. - (-dt as f64 * 7.).exp())
            }
        });
    }
    moving |= old_rings != (s.ring_session, s.ring_weekly);
    let bubble_target = if (s.peek || s.toast_until > t)
        && (s.w.value - tw).abs() < 0.5
        && (s.h.value - th).abs() < 0.5
        && !s.open
        && s.island_visible()
    {
        1.
    } else {
        0.
    };
    s.bubble_t.set(bubble_target);
    let target = target_scale(s);
    if motion {
        s.w.set(tw);
        s.h.set(th);
        moving |= s.w.step(dt);
        moving |= s.h.step(dt);
        moving |= s.bubble_t.step(dt);
        let old = s.scale;
        approach(&mut s.scale, target, 16., dt);
        moving |= old != s.scale;
        let before = s.transition;
        s.transition = (s.transition + dt / 0.36).min(1.);
        moving |= before != s.transition;
    } else {
        s.w.snap(tw);
        s.h.snap(th);
        s.bubble_t.snap(bubble_target);
        s.scale = target;
        s.transition = 1.;
    }
    if s.transition >= 1. && s.previous.is_some() {
        s.previous = None;
        s.dirty = true;
    }
    moving
}
/// One simulation + presentation step. Returns true while animating.
unsafe fn frame(hwnd: HWND, s: &mut State) -> bool {
    let now = Instant::now();
    let dt = now.duration_since(s.last).as_secs_f32().clamp(0., 0.05);
    s.last = now;
    let t = s.now();
    if let (Some(due), View::AutoSwitch(i)) = (s.auto_due, s.view.clone()) {
        if t >= due && s.open && s.worker.is_none() {
            s.auto_due = None;
            if auto_target(s) == Some(i) {
                begin(s, Op::Switch(i), true);
                navigate(s, View::Home, true);
            } else {
                let title = s.tr("Auto-switch paused");
                let body = s
                    .tr("Usage or account state changed. Review the accounts and switch manually.");
                notice(s, title, body, false);
            }
        }
    }
    let before_position = (s.anchor, s.top);
    if !s.open && GetAsyncKeyState(0x01) < 0 {
        if let Some((dx, dy, _)) = s.down {
            if drag_zone(s, dx, dy) {
                let mut pointer = zeroed();
                GetCursorPos(&mut pointer);
                let anchor = (pointer.x - s.origin.0) as f32 + s.w.value * s.scale / 2.;
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
    let mut cursor: POINT = zeroed();
    GetCursorPos(&mut cursor);
    let under = WindowFromPoint(cursor);
    let on_bubble = !s.bubble.is_null() && under == s.bubble && s.bubble_t.value > 0.3;
    let mouse_down = [0x01, 0x02, 0x04]
        .iter()
        .any(|key| GetAsyncKeyState(*key) < 0);
    if mouse_down
        && !s.mouse_was_down
        && s.down.is_none()
        && s.slider_drag.is_none()
        && !s.drag
        && (s.open || s.peek)
        && !just_opened(s)
        && !s.picking
        && under != hwnd
        && under != s.bubble
        && IsChild(hwnd, under) == 0
    {
        if s.open {
            s.deactivated_at = t;
        }
        collapse(s);
    }
    s.mouse_was_down = mouse_down;
    let (x, y) = local(s, hwnd, cursor);
    if !s.open && s.down.is_none() && !s.drag && s.island_visible() {
        if on_bubble || (under == hwnd && inside_shape(s, x, y)) {
            if !s.peek {
                s.dirty = true;
            }
            s.peek = true;
            s.peek_until = t + 0.32;
        } else if t > s.peek_until && s.peek {
            s.peek = false;
            s.dirty = true;
        }
    }
    if s.open || s.drag || s.down.is_some() {
        s.peek = false;
    }
    let mut moving = animate(s, dt, t);
    let hover = if under == hwnd {
        hit(s, x, y)
    } else {
        Action::None
    };
    if hover != s.hover {
        s.hover = hover;
        s.dirty = true;
    }
    s.dpi = monitor_dpi(s.anchor, s.top);
    constrain(s);
    if before_position != (s.anchor, s.top) {
        s.dirty = true;
    }
    if let Some(result) = s.sync.as_ref().and_then(|r| r.try_recv().ok()) {
        s.sync = None;
        if result.generation == s.generation {
            s.active = result.active;
        }
        let mut measurements = false;
        for (slot, id, result) in result.samples {
            let Some(i) = s.settings.accounts.iter().position(|a| a.slot == slot) else {
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
        s.dirty = true;
    }
    if let Some(result) = s.worker.as_ref().and_then(|r| r.try_recv().ok()) {
        s.worker = None;
        s.pending = None;
        let visible = s.open;
        let provider = s.provider_name();
        match result {
            Ok(Outcome::Already(i)) => {
                s.active = Some(i);
                let title = s.tr("Already logged in");
                let body = s
                    .tr("{name} is already logged into {app}. No sign-in was changed.")
                    .replace("{name}", &s.settings.accounts[i].name)
                    .replace("{app}", provider);
                notice(s, title, body, false);
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
                let title = s.tr("Sign in to Claude");
                let body = s.tr("Claude opened. Complete sign-in there, then choose Save again. Your existing saved accounts were kept.");
                notice(s, title, body, false);
            }
            Ok(Outcome::SignIn(slot)) => {
                let name = s
                    .settings
                    .accounts
                    .iter()
                    .find(|a| a.slot == slot)
                    .map(|a| a.name.clone())
                    .unwrap_or_else(|| s.tr("Your account").into());
                refresh_slots(s);
                s.sync_next = 0.;
                let title = s.tr("Sign in to the other account");
                let body = s.tr("{name} is already saved and was not signed out. Claude reopened at its sign-in screen: sign in to the other account there, then choose Save again. Avoid Claude's own Log out; it ends a saved session.").replace("{name}", &name);
                notice(s, title, body, false);
            }
            Ok(Outcome::Done(op, label)) => {
                if matches!(op, Op::Switch(_)) {
                    s.toast_until = t + 2.4;
                }
                if let Op::Save(i) = op {
                    s.settings.accounts[i].draft = false;
                    s.settings.accounts[i].usage = None;
                    s.usage_errors[i].clear();
                    persist(s);
                }
                if let Op::Switch(i) = op {
                    s.active = Some(i);
                    if s.handoff_copied {
                        s.handoff_ready = true;
                        let text = s.tr(CONTINUE_REQUEST);
                        copy_text(s, text);
                    }
                }
                refresh_slots(s);
                if let Op::Save(i) | Op::Switch(i) = op {
                    s.polls.remove(&s.settings.accounts[i].slot);
                }
                s.sync_next = 0.;
                navigate(s, View::Home, true);
                status(s, label, false);
            }
            Err(e) => {
                let title = s.tr("Couldn't finish this action");
                notice(s, title, e, true);
            }
        }
        if !visible {
            s.open = false;
            s.previous = None;
            s.transition = 1.;
        }
        if !s.open {
            discard_drafts(s);
        }
        s.dirty = true;
    }
    if t >= s.sync_next {
        sync(s)
    }
    if !s.status.is_empty() && t > s.status_until {
        s.status.clear();
        s.error = false;
        s.dirty = true;
    }
    let second = accounts::now();
    if second != s.last_second {
        s.last_second = second;
        s.dirty = true;
        let dark = theme::system_prefers_dark();
        if dark != s.system_dark {
            s.system_dark = dark;
            s.tray_key.clear();
        }
        update_tray(s);
    }
    if s.edit_focus {
        let blink = s.clock.elapsed().as_millis() / 530;
        if blink != s.blink {
            s.blink = blink;
            s.dirty = true;
        }
    }
    moving |= s.pulse_until > t || s.toast_until > t;
    moving |= s.down.is_some() || s.drag || s.slider_drag.is_some();
    moving |= s.mem.busy;
    if moving || s.dirty {
        s.dirty = false;
        present(hwnd, s);
        present_bubble(s);
    }
    moving
}
unsafe fn present(hwnd: HWND, s: &mut State) {
    let Some(gfx) = s.gfx.clone() else { return };
    if !s.island_visible() && !s.open {
        ShowWindow(hwnd, SW_HIDE);
        return;
    }
    let k = s.scale;
    let ww = ((s.w.value + 2. * MARGIN) * k).ceil() as i32;
    let wh = ((s.h.value + 2. * MARGIN) * k).ceil() as i32;
    let mut slot = s.surface.take();
    let Some(surface) = Surface::reuse(&mut slot, ww, wh) else {
        return;
    };
    let mut mem = std::mem::take(&mut s.mem);
    ui::draw_main(s, &mut mem, &gfx, surface);
    s.mem = mem;
    let x0 = (s.anchor - s.w.value * k / 2.).round() - MARGIN * k;
    let y0 = s.top as f32 - MARGIN * k;
    UpdateLayeredWindow(
        hwnd,
        null_mut(),
        &POINT {
            x: x0.round() as i32,
            y: y0.round() as i32,
        },
        &SIZE { cx: ww, cy: wh },
        surface.hdc_raw(),
        &POINT { x: 0, y: 0 },
        0,
        &BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: 255,
            AlphaFormat: AC_SRC_ALPHA as u8,
        },
        ULW_ALPHA,
    );
    s.surface = slot;
    if IsWindowVisible(hwnd) == 0 {
        ShowWindow(hwnd, SW_SHOWNOACTIVATE);
    }
}
unsafe fn present_bubble(s: &mut State) {
    if s.bubble.is_null() {
        return;
    }
    let Some(gfx) = s.gfx.clone() else { return };
    if s.open || s.drag || s.down.is_some() || s.bubble_t.value <= 0.01 {
        ShowWindow(s.bubble, SW_HIDE);
        return;
    }
    let k = s.scale;
    let Some(p) = bubble_placement(
        &s.work_areas,
        s.anchor,
        s.top,
        s.w.value * k,
        s.h.value * k,
        s.settings.bubble_scale * s.dpi / 96.,
    ) else {
        return;
    };
    let ww = ((BW + 2. * BMARGIN) * p.scale).ceil() as i32;
    let wh = ((BH + 2. * BMARGIN) * p.scale).ceil() as i32;
    let mut slot = s.bubble_surface.take();
    let Some(surface) = Surface::reuse(&mut slot, ww, wh) else {
        return;
    };
    let mut mem = std::mem::take(&mut s.mem);
    ui::draw_bubble(
        s,
        &mut mem,
        &gfx,
        surface,
        p.scale,
        p.right,
        s.bubble_t.value,
    );
    s.mem = mem;
    UpdateLayeredWindow(
        s.bubble,
        null_mut(),
        &POINT {
            x: p.x - (BMARGIN * p.scale).round() as i32,
            y: p.y - (BMARGIN * p.scale).round() as i32,
        },
        &SIZE { cx: ww, cy: wh },
        surface.hdc_raw(),
        &POINT { x: 0, y: 0 },
        0,
        &BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: 255,
            AlphaFormat: AC_SRC_ALPHA as u8,
        },
        ULW_ALPHA,
    );
    s.bubble_surface = slot;
    if IsWindowVisible(s.bubble) == 0 {
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
fn open_panel(s: &mut State, hwnd: HWND) {
    restore_from_tray(s);
    if !s.open {
        activate(s, Action::Toggle);
        s.opened_at = s.now();
    }
    unsafe {
        present(hwnd, s);
        ShowWindow(hwnd, SW_SHOW);
        SetForegroundWindow(hwnd);
    }
}
/// Focus can bounce back to the taskbar right after a tray click opens the
/// panel; that is not the user clicking away.
fn just_opened(s: &State) -> bool {
    s.now() - s.opened_at < 0.6
}
unsafe fn tray_menu(hwnd: HWND, s: &mut State) {
    let menu = CreatePopupMenu();
    if menu.is_null() {
        return;
    }
    let item = |id: usize, label: &str, checked: bool| {
        let text = wide(label);
        AppendMenuW(
            menu,
            MF_STRING | if checked { MF_CHECKED } else { MF_UNCHECKED },
            id,
            text.as_ptr(),
        );
    };
    item(
        1,
        s.tr(if s.open {
            "Close Muisland"
        } else {
            "Open Muisland"
        }),
        false,
    );
    item(2, s.tr("Settings"), false);
    item(3, s.tr("Show floating island"), s.island_visible());
    item(4, s.tr("Reset position"), false);
    AppendMenuW(menu, MF_SEPARATOR, 0, null());
    item(9, s.tr("Quit Muisland"), false);
    let mut p: POINT = zeroed();
    GetCursorPos(&mut p);
    SetForegroundWindow(hwnd);
    let flags =
        TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON | if s.rtl() { TPM_LAYOUTRTL } else { 0 };
    let command = TrackPopupMenu(menu, flags, p.x, p.y, 0, hwnd, null());
    DestroyMenu(menu);
    match command {
        1 => {
            if s.open {
                collapse(s)
            } else {
                open_panel(s, hwnd)
            }
        }
        2 => {
            open_panel(s, hwnd);
            if !matches!(s.view, View::Preferences) {
                navigate(s, View::Preferences, false);
            }
        }
        3 => {
            if s.island_visible() {
                s.settings.show_island = false;
            } else {
                s.settings.show_island = true;
                s.in_tray = false;
            }
            persist(s);
        }
        4 => {
            let areas = available_work_areas();
            if let Some(a) = areas.first() {
                s.float_position = ((a.left + a.right) as f32 / 2., a.top + 20);
                if !s.open {
                    s.anchor = s.float_position.0;
                    s.top = s.float_position.1;
                }
                s.settings.show_island = true;
                s.in_tray = false;
                persist(s);
            }
        }
        9 => {
            open_panel(s, hwnd);
            activate(s, Action::Quit);
        }
        _ => {}
    }
    s.dirty = true;
}
unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut State;
    if ptr.is_null() {
        return DefWindowProcW(hwnd, msg, wp, lp);
    }
    if msg == WM_DESTROY {
        UnregisterHotKey(hwnd, HOTKEY_ID);
        system::tray_remove(hwnd);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
        if !(*ptr).bubble.is_null() {
            DestroyWindow((*ptr).bubble);
        }
        drop(Box::from_raw(ptr));
        PostQuitMessage(0);
        return 0;
    }
    let s = &mut *ptr;
    s.dirty = true;
    match msg {
        BUBBLE_OPEN => {
            open_panel(s, hwnd);
            0
        }
        HANDOFF_FOUND => {
            collect_handoffs(s);
            0
        }
        FOLDER_PICKED => {
            s.picking = false;
            if let Some(Ok(Some(folder))) = s.folder_rx.take().map(|rx| rx.try_recv()) {
                // A folder picked by hand also counts as a project of this handoff.
                if s.handoff_copied && !s.handoff_folders.iter().any(|f| f.eq_ignore_ascii_case(&folder)) {
                    s.handoff_folders.push(folder.clone());
                }
                s.settings.handoff_folder = Some(folder);
                persist(s);
            }
            if s.open {
                SetForegroundWindow(hwnd);
            }
            0
        }
        REFOCUS => {
            if s.open {
                SetForegroundWindow(hwnd);
            }
            0
        }
        WM_HOTKEY if wp as i32 == HOTKEY_ID => {
            if s.open && GetForegroundWindow() == hwnd {
                collapse(s);
            } else {
                open_panel(s, hwnd);
            }
            0
        }
        system::TRAY_MESSAGE => {
            match (lp as u32) & 0xFFFF {
                WM_LBUTTONUP => {
                    if s.open {
                        collapse(s)
                    } else if s.now() - s.deactivated_at > 0.4 {
                        // Clicking the icon first deactivates (and closes) the
                        // panel; that click must not reopen it.
                        open_panel(s, hwnd)
                    }
                }
                WM_RBUTTONUP | WM_CONTEXTMENU => tray_menu(hwnd, s),
                _ => {}
            }
            0
        }
        WM_ACTIVATE if (wp as u32 & 0xFFFF) == WA_INACTIVE => {
            if s.open && (just_opened(s) || s.picking) {
                if !s.picking {
                    PostMessageW(hwnd, REFOCUS, 0, 0);
                }
                return 0;
            }
            if s.open || s.peek {
                if s.open {
                    s.deactivated_at = s.now();
                }
                collapse(s);
            }
            0
        }
        m if m == s.taskbar_created && m != 0 => {
            system::tray_reset();
            s.tray_key.clear();
            update_tray(s);
            0
        }
        WM_DISPLAYCHANGE | WM_SETTINGCHANGE | WM_DPICHANGED => {
            s.work_areas = available_work_areas();
            s.system_dark = theme::system_prefers_dark();
            constrain(s);
            0
        }
        WM_ERASEBKGND => 1,
        WM_PAINT => {
            let mut ps = zeroed();
            BeginPaint(hwnd, &mut ps);
            EndPaint(hwnd, &ps);
            0
        }
        WM_CLOSE => {
            activate(s, Action::QuitNow);
            0
        }
        WM_SETCURSOR => {
            let mut p = zeroed();
            GetCursorPos(&mut p);
            let (x, y) = local(s, hwnd, p);
            let action = hit(s, x, y);
            SetCursor(LoadCursorW(
                null_mut(),
                if action == Action::NameField {
                    IDC_IBEAM
                } else if action != Action::None && action != Action::Header {
                    IDC_HAND
                } else {
                    IDC_ARROW
                },
            ));
            1
        }
        WM_LBUTTONDOWN => {
            let (x, y) = lparam_point(lp, s.scale);
            if !s.open && !inside_shape(s, x, y) {
                return 0;
            }
            let action = hit(s, x, y);
            if slider_bounds(action).is_some() {
                s.slider_drag = Some(action);
                s.slider_preview = Some(slider_from_x(s, action, x));
                s.focus = action;
                SetCapture(hwnd);
                return 0;
            }
            if s.open && action == Action::None && !inside_shape(s, x, y) {
                collapse(s);
                return 0;
            }
            s.down = Some((x, y, action));
            s.drag = false;
            s.focus = Action::None;
            if drag_zone(s, x, y) {
                let mut p = zeroed();
                GetCursorPos(&mut p);
                s.origin = (
                    p.x - (s.anchor - s.w.value * s.scale / 2.) as i32,
                    p.y - s.top,
                );
                SetCapture(hwnd);
            }
            0
        }
        WM_MOUSEMOVE => {
            if let Some(action) = s.slider_drag {
                let (x, _) = lparam_point(lp, s.scale);
                s.slider_preview = Some(slider_from_x(s, action, x));
                return 0;
            }
            if let Some((dx, dy, _)) = s.down {
                let (x, y) = lparam_point(lp, s.scale);
                if drag_zone(s, dx, dy) && ((x - dx).abs() > 4. || (y - dy).abs() > 4. || s.drag) {
                    s.drag = true;
                    let mut p = zeroed();
                    GetCursorPos(&mut p);
                    s.anchor = (p.x - s.origin.0) as f32 + s.w.value * s.scale / 2.;
                    s.top = p.y - s.origin.1;
                    // The island returns to wherever the user last dragged it.
                    s.float_position = (s.anchor, s.top);
                    constrain(s);
                }
            }
            0
        }
        WM_LBUTTONUP => {
            if let Some(action) = s.slider_drag.take() {
                let (x, _) = lparam_point(lp, s.scale);
                let value = slider_from_x(s, action, x);
                set_slider(s, action, value);
                s.slider_preview = None;
                ReleaseCapture();
                persist(s);
                return 0;
            }
            let down = s.down.take();
            let drag = s.drag;
            ReleaseCapture();
            let (x, y) = lparam_point(lp, s.scale);
            if let Some((dx, dy, action)) = down {
                if drag && !s.preview {
                    // Remember the new floating position.
                    persist(s);
                }
                if !drag && (x - dx).abs() < 8. && (y - dy).abs() < 8. && hit(s, x, y) == action {
                    activate(s, action);
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
            if s.open && s.slider_drag.is_none() {
                let delta = ((wp >> 16) as u16) as i16;
                let index = if matches!(s.view, View::Preferences | View::Section(_)) {
                    Some(1)
                } else if matches!(s.view, View::Home) {
                    Some(0)
                } else {
                    None
                };
                if let Some(index) = index {
                    s.mem.scroll_by(index, -(delta as f32) / 120. * 64.);
                }
            }
            0
        }
        WM_RBUTTONUP => {
            if !s.open {
                navigate(s, View::Preferences, false);
                SetForegroundWindow(hwnd);
            }
            0
        }
        WM_CHAR => {
            if s.edit_focus {
                let c = wp as u16;
                if c == 8 {
                    if s.selected {
                        replace_selection(s);
                    } else if s.caret > 0 {
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
                s.blink = 0;
            }
            0
        }
        WM_KEYDOWN => {
            key_down(hwnd, s, wp as u32);
            0
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}
unsafe fn key_down(hwnd: HWND, s: &mut State, key: u32) {
    let control = GetKeyState(0x11) < 0;
    let scroll_index = match s.view {
        View::Preferences | View::Section(_) => Some(1),
        View::Home => Some(0),
        _ => None,
    };
    // Arrow keys follow the visual direction of the slider.
    let left = if s.rtl() { 0x27 } else { 0x25 };
    if let (Some(i), true, false) = (scroll_index, [0x21, 0x22].contains(&key), s.edit_focus) {
        s.mem.scroll_by(i, if key == 0x22 { 240. } else { -240. });
    } else if slider_bounds(s.focus).is_some() && [0x25, 0x27, 0x24, 0x23].contains(&key) {
        let action = s.focus;
        let (lo, hi) = slider_bounds(action).unwrap();
        let value = match key {
            0x24 => lo,
            0x23 => hi,
            k if k == left => slider_value(s, action) - 0.05,
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
            activate(s, Action::Back);
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
    } else if s.edit_focus && (key == 0x25 || key == 0x27) {
        // Logical caret movement; visual direction follows the reading order.
        let forward = (key == 0x27) != s.rtl();
        if forward {
            s.caret = (s.caret + 1).min(s.edit.len());
        } else {
            s.caret = s.caret.saturating_sub(1);
        }
        s.selected = false;
    } else if s.edit_focus && key == 0x2E {
        if s.selected {
            replace_selection(s)
        } else if s.caret < s.edit.len() {
            s.edit.remove(s.caret);
        }
    } else if key == 0x09 {
        s.edit_focus = false;
        let actions: Vec<Action> = s
            .mem
            .hits
            .iter()
            .map(|(_, a)| *a)
            .filter(|a| !matches!(a, Action::Header | Action::None))
            .fold(Vec::new(), |mut v, a| {
                if !v.contains(&a) {
                    v.push(a)
                }
                v
            });
        if !actions.is_empty() {
            let shift = GetKeyState(0x10) < 0;
            let current = actions.iter().position(|a| *a == s.focus);
            let n = actions.len();
            s.focus = actions[match current {
                Some(i) if shift => (i + n - 1) % n,
                Some(i) => (i + 1) % n,
                None => 0,
            }];
            s.mem.ensure_visible(s.focus);
        }
    } else if (key == 0x0D || key == 0x20) && !s.edit_focus {
        let a = if s.focus == Action::None {
            if matches!(
                s.view,
                View::Confirm(_) | View::SwitchConfirm(_) | View::Remove(_, _) | View::QuitConfirm
            ) {
                Action::Cancel
            } else {
                Action::Toggle
            }
        } else {
            s.focus
        };
        activate(s, a);
    }
}
fn system_language() -> usize {
    #[link(name = "kernel32")]
    extern "system" {
        fn GetUserDefaultUILanguage() -> u16;
    }
    if unsafe { GetUserDefaultUILanguage() } & 0x3FF == 0x01 {
        1
    } else {
        0
    }
}
pub fn initial(preview: bool, anchor: f32, top: i32) -> State {
    let now = Instant::now();
    let mut settings = if preview {
        Settings::default()
    } else {
        Settings::load().unwrap_or_default()
    };
    if settings.language_unset {
        settings.language = system_language();
    }
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
                    used: if i == 0 { 46. } else { 64. },
                    resets: Some(accounts::now() + 86400 * 3),
                }),
                updated: accounts::now(),
                plan: "Max".into(),
            });
        }
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
    let (cw, ch) = if settings.circle {
        (CIRCLE, CIRCLE)
    } else {
        (PILL_W, PILL_H)
    };
    let saved_position = settings.position;
    let mut s = State {
        scale: settings.float_scale,
        settings,
        saved: Vec::new(),
        identities: Vec::new(),
        usage_errors: Vec::new(),
        active: if preview { Some(0) } else { None },
        open: false,
        peek: false,
        peek_until: 0.,
        bubble: null_mut(),
        bubble_t: Spring::new(0., 420., 34.),
        w: Spring::new(cw, 300., 26.),
        h: Spring::new(ch, 270., 25.),
        dpi: 96.,
        anchor,
        top,
        float_position: saved_position.unwrap_or((anchor, top)),
        view: View::Home,
        previous: None,
        back: false,
        transition: 1.,
        hover: Action::None,
        focus: Action::None,
        down: None,
        drag: false,
        slider_drag: None,
        slider_preview: None,
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
        mem: Mem::default(),
        gfx: None,
        system_dark: theme::system_prefers_dark(),
        launch_at_login: !preview && system::launch_at_login(),
        dirty: true,
        tray_key: String::new(),
        tray_hint_shown: false,
        backup: None,
        in_tray: false,
        handoff_copied: false,
        handoff_ready: false,
        picking: false,
        folder_rx: None,
        handoff_folders: Vec::new(),
        handoff_since: None,
        handoff_rx: None,
        opened_at: -10.,
        hwnd: null_mut(),
        blink: 0,
        surface: None,
        bubble_surface: None,
        deactivated_at: -10.,
        taskbar_created: 0,
    };
    if let Some((a, t)) = saved_position {
        s.anchor = a;
        s.top = t;
    }
    refresh_slots(&mut s);
    s
}
pub fn run() {
    unsafe {
        if SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) == 0 {
            SetProcessDPIAware();
        }
        let preview = std::env::args().any(|a| a == "--preview");
        let class = wide(if preview {
            "MuislandPreviewWindow"
        } else {
            "MuislandWindow"
        });
        let existing = FindWindowW(class.as_ptr(), null());
        if !existing.is_null() {
            // Launching again opens the running island.
            PostMessageW(existing, BUBBLE_OPEN, 0, 0);
            return;
        }
        let gfx = match Gfx::new() {
            Ok(g) => Rc::new(g),
            Err(e) => {
                let text = wide(&format!("Muisland could not start its renderer: {e}"));
                MessageBoxW(
                    null_mut(),
                    text.as_ptr(),
                    wide("Muisland").as_ptr(),
                    MB_ICONERROR,
                );
                return;
            }
        };
        let instance = GetModuleHandleW(null());
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: instance,
            hCursor: LoadCursorW(null_mut(), IDC_ARROW),
            lpszClassName: class.as_ptr(),
            hIcon: LoadIconW(instance, 1 as _),
            ..zeroed()
        };
        if RegisterClassW(&wc) == 0 {
            return;
        }
        let mut area: RECT = zeroed();
        SystemParametersInfoW(SPI_GETWORKAREA, 0, (&mut area as *mut RECT).cast(), 0);
        let mut s = Box::new(initial(
            preview,
            (area.left + area.right) as f32 / 2.,
            area.top + 20,
        ));
        s.gfx = Some(gfx);
        s.work_areas = available_work_areas();
        s.dpi = monitor_dpi(s.float_position.0, s.float_position.1);
        if std::env::args().any(|a| a == "--reset-position") {
            s.float_position = ((area.left + area.right) as f32 / 2., area.top + 20);
            s.settings.show_island = true;
        }
        s.anchor = s.float_position.0;
        s.top = s.float_position.1;
        s.scale = target_scale(&s);
        constrain(&mut s);
        if preview {
            s.open = true;
        }
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
            0,
            0,
            1,
            1,
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
        s.hwnd = hwnd;
        s.taskbar_created = system::taskbar_created();
        apply_system(&mut s);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(s) as isize);
        if std::env::args().any(|a| a == "--show") {
            PostMessageW(hwnd, BUBBLE_OPEN, 0, 0);
        }
        // Frame loop: present on DWM's vsync while anything moves, otherwise
        // sleep until input arrives or a short housekeeping interval elapses.
        let mut message: MSG = zeroed();
        loop {
            while PeekMessageW(&mut message, null_mut(), 0, 0, PM_REMOVE) != 0 {
                if message.message == WM_QUIT {
                    return;
                }
                TranslateMessage(&message);
                DispatchMessageW(&message);
            }
            let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut State;
            if ptr.is_null() {
                // Window destroyed; drain until WM_QUIT.
                if GetMessageW(&mut message, null_mut(), 0, 0) <= 0 {
                    return;
                }
                TranslateMessage(&message);
                DispatchMessageW(&message);
                continue;
            }
            let state = &mut *ptr;
            let animating = frame(hwnd, state);
            if animating {
                if DwmFlush() != 0 {
                    std::thread::sleep(std::time::Duration::from_millis(7));
                }
            } else {
                let wait = if state.open || state.peek || state.edit_focus {
                    33
                } else {
                    120
                };
                MsgWaitForMultipleObjects(0, null(), 0, wait, QS_ALLINPUT);
            }
        }
    }
}

/// Renders documentation previews (PNG) from sample data without a window.
pub fn export_previews(folder: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(folder)?;
    let gfx = Rc::new(Gfx::new().map_err(std::io::Error::other)?);
    let scenes: &[(&str, usize, usize)] = &[
        ("accounts", 4, 0),
        ("accounts-arabic", 4, 1),
        ("preferences", 4, 0),
        ("preferences-themes", 0, 0),
        ("preferences-arabic", 4, 1),
        ("usage-settings", 4, 0),
        ("island-settings", 4, 0),
        ("backup-settings", 4, 1),
        ("account-name", 4, 0),
        ("switch-confirm", 4, 0),
        ("remove-account", 4, 0),
        ("quit-confirm", 4, 0),
        ("handoff-switch", 4, 0),
        ("handoff-continue", 4, 0),
        ("handoff-arabic", 4, 1),
        ("handoff-found", 4, 0),
        ("notice", 4, 0),
        ("empty", 4, 0),
        ("auto-switch", 4, 0),
        ("circle", 4, 0),
        ("circle-hover", 4, 0),
        ("circle-hover-left", 4, 1),
        ("circle-logo", 4, 0),
        ("pill", 4, 0),
        ("daylight", 8, 0),
        ("sunset", 2, 0),
        ("ocean-circle", 1, 0),
        ("high-usage", 4, 0),
    ];
    for &(name, theme_index, language) in scenes {
        let mut s = initial(true, 0., 0);
        s.gfx = Some(gfx.clone());
        s.settings.theme = theme_index;
        s.settings.language = language;
        s.system_dark = true;
        s.dpi = 96.;
        s.scale = 2.;
        s.open = true;
        s.w.snap(PW);
        s.h.snap(PH);
        s.ring_session = Some(28.);
        s.ring_weekly = Some(46.);
        s.mem.instant = true;
        match name {
            "preferences" | "preferences-arabic" => s.view = View::Preferences,
            "preferences-themes" => s.view = View::Section(0),
            "island-settings" => s.view = View::Section(1),
            "usage-settings" => s.view = View::Section(2),
            "backup-settings" => {
                s.settings.protect_local = true;
                s.backup = Some(crate::backup::Summary {
                    count: 3,
                    bytes: 21_400_000,
                    latest: Some(accounts::now() - 7200),
                });
                s.view = View::Section(4);
            }
            "account-name" => {
                s.edit = "حساب الشغل".encode_utf16().collect();
                s.view = View::Details(0);
                s.caret = s.edit.len();
                s.edit_focus = true;
                s.selected = false;
            }
            "switch-confirm" => s.view = View::SwitchConfirm(1),
            "remove-account" => s.view = View::Remove(1, "Work".into()),
            "quit-confirm" => s.view = View::QuitConfirm,
            "handoff-switch" => s.view = View::SwitchConfirm(1),
            "handoff-continue" => {
                s.handoff_ready = true;
                s.handoff_folders = vec![
                    "D:\\muhammad\\projects\\crm".into(),
                    "D:\\Muisland".into(),
                ];
            }
            "handoff-found" => {
                s.handoff_copied = true;
                s.handoff_folders = vec![
                    "D:\\muhammad\\projects\\crm".into(),
                    "D:\\Muisland".into(),
                ];
                s.view = View::SwitchConfirm(1);
            }
            "handoff-arabic" => {
                s.handoff_copied = true;
                s.view = View::SwitchConfirm(1);
            }
            "notice" => {
                s.view = View::Notice(
                    s.tr("Already logged in").into(),
                    s.tr("{name} is already logged into {app}. No sign-in was changed.")
                        .replace("{name}", "Personal")
                        .replace("{app}", "Claude"),
                )
            }
            "empty" => {
                s.settings.accounts.clear();
                s.active = None;
                refresh_slots(&mut s);
            }
            "auto-switch" => {
                s.view = View::AutoSwitch(1);
                s.auto_due = Some(s.now() + 11.);
            }
            "high-usage" => {
                s.active = Some(1);
                s.ring_session = Some(82.);
                s.ring_weekly = Some(64.);
                s.open = false;
                s.w.snap(CIRCLE);
                s.h.snap(CIRCLE);
            }
            "circle" | "circle-hover" | "circle-hover-left" | "circle-logo" | "ocean-circle" => {
                s.open = false;
                s.settings.circle = true;
                s.settings.circle_numbers = name != "circle-logo";
                s.w.snap(CIRCLE);
                s.h.snap(CIRCLE);
            }
            "pill" => {
                s.open = false;
                s.settings.circle = false;
                s.w.snap(PILL_W);
                s.h.snap(PILL_H);
            }
            _ => {}
        }
        // Two layout passes: the first measures scroll extents.
        for _ in 0..2 {
            let surface =
                render_main_surface(&mut s).ok_or_else(|| std::io::Error::other("render"))?;
            if name.starts_with("circle-hover") {
                let card = render_bubble_surface(&mut s, 2., name == "circle-hover")
                    .ok_or_else(|| std::io::Error::other("render"))?;
                let board = compose_side_by_side(&surface, &card, name == "circle-hover");
                std::fs::write(
                    folder.join(format!("{name}.png")),
                    board.png(Some(s.theme().bg.mix(crate::gfx::hex(0x000000), 0.4))),
                )?;
            } else {
                std::fs::write(
                    folder.join(format!("{name}.png")),
                    surface.png(Some(if s.theme().dark {
                        crate::gfx::hex(0x2A2D3A)
                    } else {
                        crate::gfx::hex(0xC9CEDB)
                    })),
                )?;
            }
        }
    }
    Ok(())
}
/// Renders 60 fps motion sequences (open, settings, hover, close) as numbered
/// PNG frames, using exactly the same springs as the live window.
pub fn export_motion(folder: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(folder)?;
    let gfx = Rc::new(Gfx::new().map_err(std::io::Error::other)?);
    let mut s = initial(true, 0., 0);
    s.gfx = Some(gfx);
    s.dpi = 96.;
    s.settings.float_scale = 1.5;
    s.settings.menu_scale = 1.5;
    s.settings.bubble_scale = 1.5;
    s.scale = 1.5;
    s.system_dark = true;
    s.open = false;
    s.w.snap(CIRCLE);
    s.h.snap(CIRCLE);
    s.mem.fixed_dt = Some(1. / 60.);
    let dt = 1. / 60.;
    let mut t = 0.;
    let mut index = 0;
    // A fixed canvas keeps frames aligned for animation tools.
    let cw = ((PW + 2. * MARGIN) * 1.5).ceil() as i32;
    let ch = ((PH + 2. * MARGIN) * 1.5).ceil() as i32;
    let shoot = |s: &mut State, index: &mut i32| -> std::io::Result<()> {
        let image = render_main_surface(s).ok_or_else(|| std::io::Error::other("render"))?;
        let board = Surface::new(cw, ch).ok_or_else(|| std::io::Error::other("surface"))?;
        // Centre horizontally; blit clips springs that overshoot the canvas.
        board.blit(&image, (cw - image.w) / 2, 0);
        std::fs::write(
            folder.join(format!("frame-{:03}.png", *index)),
            board.png(Some(crate::gfx::hex(0x1E2130))),
        )?;
        *index += 1;
        Ok(())
    };
    let run = |s: &mut State, frames: usize, index: &mut i32, t: &mut f32| -> std::io::Result<()> {
        for _ in 0..frames {
            *t += dt;
            animate(s, dt, *t);
            shoot(s, index)?;
        }
        Ok(())
    };
    s.ring_session = Some(0.);
    s.ring_weekly = Some(0.);
    run(&mut s, 40, &mut index, &mut t)?;
    s.peek = true;
    run(&mut s, 16, &mut index, &mut t)?;
    s.peek = false;
    s.open = true;
    run(&mut s, 50, &mut index, &mut t)?;
    navigate(&mut s, View::Preferences, false);
    run(&mut s, 34, &mut index, &mut t)?;
    navigate(&mut s, View::Home, true);
    run(&mut s, 34, &mut index, &mut t)?;
    collapse(&mut s);
    run(&mut s, 50, &mut index, &mut t)?;
    Ok(())
}
pub fn render_main_surface(s: &mut State) -> Option<Surface> {
    let gfx = s.gfx.clone()?;
    let k = s.scale;
    let surface = Surface::new(
        ((s.w.value + 2. * MARGIN) * k).ceil() as i32,
        ((s.h.value + 2. * MARGIN) * k).ceil() as i32,
    )?;
    let mut mem = std::mem::take(&mut s.mem);
    ui::draw_main(s, &mut mem, &gfx, &surface);
    s.mem = mem;
    Some(surface)
}
pub fn render_bubble_surface(s: &mut State, scale: f32, right: bool) -> Option<Surface> {
    let gfx = s.gfx.clone()?;
    let surface = Surface::new(
        ((BW + 2. * BMARGIN) * scale).ceil() as i32,
        ((BH + 2. * BMARGIN) * scale).ceil() as i32,
    )?;
    let mut mem = std::mem::take(&mut s.mem);
    ui::draw_bubble(s, &mut mem, &gfx, &surface, scale, right, 1.);
    s.mem = mem;
    Some(surface)
}
fn compose_side_by_side(island: &Surface, card: &Surface, card_right: bool) -> Surface {
    let w = island.w + card.w - 20;
    let h = island.h.max(card.h);
    let board = Surface::new(w, h).expect("surface");
    let place = |img: &Surface, x: i32, y: i32| board.blit(img, x, y);
    let iy = (h - island.h) / 2;
    if card_right {
        place(card, island.w - 20, 0);
        place(island, 0, iy);
    } else {
        place(card, 0, 0);
        place(island, card.w - 20, iy);
    }
    board
}

#[cfg(test)]
mod tests {
    use super::*;
    fn sample() -> State {
        let mut s = initial(true, 0., 0);
        s.gfx = Some(Rc::new(Gfx::new().unwrap()));
        s.mem.instant = true;
        s.scale = 1.;
        s
    }
    fn open(s: &mut State) {
        s.open = true;
        s.w.snap(PW);
        s.h.snap(PH);
        s.transition = 1.;
        s.previous = None;
        render_main_surface(s).unwrap();
    }
    fn center_of(s: &State, a: Action) -> (f32, f32) {
        let (r, _) = s
            .mem
            .hits
            .iter()
            .find(|(_, x)| *x == a)
            .unwrap_or_else(|| panic!("{a:?} not drawn"));
        (r.cx(), r.cy())
    }
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
            s.w.snap(PW);
            s.h.snap(PH);
            constrain(&mut s);
            assert_eq!(s.float_position, home);
            assert_ne!((s.anchor, s.top), home);
            collapse(&mut s);
            constrain(&mut s);
            assert_eq!(s.float_position, home);
            s.w.snap(CIRCLE);
            s.h.snap(CIRCLE);
            constrain(&mut s);
            assert_eq!((s.anchor, s.top), home);
            s.open = true;
            s.w.snap(PW);
            s.h.snap(PH);
            constrain(&mut s);
            collapse(&mut s);
            s.w.snap(CIRCLE);
            s.h.snap(CIRCLE);
            constrain(&mut s);
            assert_eq!((s.anchor, s.top), home);
        }
    }
    #[test]
    fn default_island_is_a_circle_showing_the_percentage() {
        let s = accounts::Settings::default();
        assert!(s.circle);
        assert!(s.circle_numbers);
        assert_eq!(s.circle_metric, 0);
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
                        let p =
                            bubble_placement(&[area], cx, top, diameter, diameter, 1.8).unwrap();
                        assert_eq!(p.right, !is_right);
                        assert!(p.x >= area.left + 7 && p.y >= area.top + 7);
                        assert!(p.x + (BW * p.scale).round() as i32 <= area.right - 7);
                        assert!(p.y + (BH * p.scale).round() as i32 <= area.bottom - 7);
                        if p.right {
                            assert!(p.x as f32 > cx + diameter / 2.);
                        } else {
                            assert!((p.x as f32 + BW * p.scale) < cx - diameter / 2.);
                        }
                    }
                }
            }
        }
    }
    #[test]
    fn circle_renders_round_with_transparent_corners_and_soft_edges() {
        let mut s = sample();
        s.open = false;
        s.settings.circle = true;
        s.w.snap(CIRCLE);
        s.h.snap(CIRCLE);
        s.scale = 2.;
        let image = render_main_surface(&mut s).unwrap();
        let m = (MARGIN * 2.) as i32;
        assert_eq!(image.w, ((CIRCLE + 2. * MARGIN) * 2.).ceil() as i32);
        assert_eq!(image.alpha_at(m + 2, m + 2), 0, "corner must be clear");
        assert_eq!(image.alpha_at(image.w / 2, image.h / 2), 255);
        let edge: Vec<u8> = (0..image.w)
            .map(|x| image.alpha_at(x, image.h / 2))
            .collect();
        assert!(edge.iter().any(|a| *a > 30 && *a < 225), "anti-aliased rim");
    }
    #[test]
    fn bubble_card_has_transparent_margin_and_opaque_body() {
        let mut s = sample();
        for right in [false, true] {
            let image = render_bubble_surface(&mut s, 1., right).unwrap();
            assert_eq!(image.alpha_at(0, 0), 0);
            assert_eq!(
                image.alpha_at((BMARGIN + BW / 2.) as i32, (BMARGIN + BH / 2.) as i32),
                255
            );
        }
    }
    #[test]
    fn closing_discards_new_unsaved_accounts_and_keeps_existing_slots() {
        let mut s = initial(true, 0., 0);
        let i = s.settings.add_account().unwrap();
        refresh_slots(&mut s);
        assert!(!s.saved[i]);
        assert!(s.identities[i].is_none());
        collapse(&mut s);
        assert_eq!(s.settings.accounts.len(), 2);
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
        let mut s = sample();
        open(&mut s);
        let claude = center_of(&s, Action::ClaudeTab);
        let codex = center_of(&s, Action::CodexTab);
        assert_eq!(hit(&s, claude.0, claude.1), Action::ClaudeTab);
        assert_eq!(hit(&s, codex.0, codex.1), Action::CodexTab);
        s.view = View::Section(0);
        render_main_surface(&mut s).unwrap();
        let a = center_of(&s, Action::Theme(1));
        let b = center_of(&s, Action::Theme(2));
        assert_eq!(hit(&s, a.0, a.1), Action::Theme(1));
        assert_eq!(hit(&s, b.0, b.1), Action::Theme(2));
        s.settings.select_provider(1);
        s.view = View::Home;
        render_main_surface(&mut s).unwrap();
        assert!(!s.mem.hits.iter().any(|(_, a)| *a == Action::Protect));
    }
    #[test]
    fn every_account_is_reachable_by_scrolling() {
        let mut s = sample();
        for _ in 0..6 {
            s.settings.add_account();
        }
        refresh_slots(&mut s);
        open(&mut s);
        assert!(s.mem.hits.iter().any(|(_, a)| *a == Action::Switch(0)));
        assert!(!s.mem.hits.iter().any(|(_, a)| *a == Action::Switch(7)));
        s.mem.scroll_by(0, 10000.);
        render_main_surface(&mut s).unwrap();
        render_main_surface(&mut s).unwrap();
        let (x, y) = center_of(&s, Action::Switch(7));
        assert_eq!(hit(&s, x, y), Action::Switch(7));
        let (x, y) = center_of(&s, Action::Details(7));
        assert_eq!(hit(&s, x, y), Action::Details(7));
    }
    #[test]
    fn scrolled_rows_never_steal_clicks_from_the_header_or_footer() {
        let mut s = sample();
        for _ in 0..6 {
            s.settings.add_account();
        }
        refresh_slots(&mut s);
        open(&mut s);
        s.mem.scroll_by(0, 60.);
        render_main_surface(&mut s).unwrap();
        let (x, y) = center_of(&s, Action::ClaudeTab);
        assert_eq!(hit(&s, x, y), Action::ClaudeTab);
        assert_eq!(hit(&s, MARGIN + 40., MARGIN + 20.), Action::Header);
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
        let mut s = sample();
        remove_account(&mut s, 1);
        remove_account(&mut s, 0);
        open(&mut s);
        assert!(s.settings.accounts.is_empty());
        assert!(s.active.is_none());
        assert!(s.mem.hits.iter().any(|(_, a)| *a == Action::Add));
    }
    #[test]
    fn automatic_switch_requires_fresh_complete_lower_usage_and_distinct_identity() {
        let mut s = initial(true, 0., 0);
        s.active = Some(0);
        refresh_slots(&mut s);
        let set = |s: &mut State, i: usize, v: f64| {
            s.settings.accounts[i]
                .usage
                .as_mut()
                .unwrap()
                .session
                .as_mut()
                .unwrap()
                .used = v
        };
        set(&mut s, 0, 90.);
        set(&mut s, 1, 20.);
        s.settings.accounts[1]
            .usage
            .as_mut()
            .unwrap()
            .weekly
            .as_mut()
            .unwrap()
            .used = 30.;
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
        let session = |s: &mut State, v: f64| {
            s.settings.accounts[0]
                .usage
                .as_mut()
                .unwrap()
                .session
                .as_mut()
                .unwrap()
                .used = v
        };
        session(&mut s, 80.);
        assert!(
            threshold_hit(&s, 0),
            "Exact configured percentage must trigger"
        );
        session(&mut s, 90.);
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
    fn sliders_follow_the_drawn_track_clamp_and_stay_independent() {
        let mut s = sample();
        s.view = View::Section(1);
        open(&mut s);
        let track = s.mem.slider_track(Action::MenuSlider).unwrap();
        assert!((slider_from_x(&s, Action::MenuSlider, track.x - 100.) - 0.75).abs() < 0.001);
        assert!((slider_from_x(&s, Action::MenuSlider, track.right() + 100.) - 1.6).abs() < 0.001);
        s.settings.language = 1;
        assert!((slider_from_x(&s, Action::MenuSlider, track.x - 100.) - 1.6).abs() < 0.001);
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
        let mut s = sample();
        s.view = View::Section(0);
        open(&mut s);
        s.mem.scroll_by(1, 100000.);
        render_main_surface(&mut s).unwrap();
        render_main_surface(&mut s).unwrap();
        assert!(s.mem.hits.iter().any(|(_, a)| *a == Action::Language(0)));
        let (x, y) = center_of(&s, Action::ClaudeTab);
        assert_eq!(hit(&s, x, y), Action::ClaudeTab);
    }
    #[test]
    fn settings_list_opens_every_category_and_back_returns_to_it() {
        let mut s = sample();
        s.view = View::Preferences;
        open(&mut s);
        for i in 0..ui::CATEGORIES as u8 {
            assert!(s.mem.hits.iter().any(|(_, a)| *a == Action::Category(i)));
        }
        assert!(s.mem.hits.iter().any(|(_, a)| *a == Action::Quit));
        activate(&mut s, Action::Category(3));
        assert!(matches!(s.view, View::Section(3)));
        s.transition = 1.;
        s.previous = None;
        render_main_surface(&mut s).unwrap();
        assert!(s.mem.hits.iter().any(|(_, a)| *a == Action::Motion));
        activate(&mut s, Action::Back);
        assert!(matches!(s.view, View::Preferences));
        activate(&mut s, Action::Category(4));
        assert!(s.backup.is_some(), "the backup page reads the folder summary");
        activate(&mut s, Action::Preferences);
        assert!(matches!(s.view, View::Home));
    }
    #[test]
    fn arabic_layout_mirrors_controls() {
        let mut s = sample();
        open(&mut s);
        let ltr = center_of(&s, Action::Preferences);
        s.settings.language = 1;
        render_main_surface(&mut s).unwrap();
        let rtl = center_of(&s, Action::Preferences);
        assert!(ltr.0 > MARGIN + PW / 2.);
        assert!(rtl.0 < MARGIN + PW / 2.);
        assert!(drag_zone(&s, MARGIN + PW - 40., MARGIN + 20.));
        assert!(!drag_zone(&s, MARGIN + 40., MARGIN + 20.));
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
    fn collapsed_island_is_draggable_and_menu_body_is_not() {
        let mut s = initial(true, 0., 0);
        s.open = false;
        assert!(drag_zone(&s, MARGIN + 30., MARGIN + 30.));
        s.open = true;
        assert!(drag_zone(&s, MARGIN + 120., MARGIN + 28.));
        assert!(!drag_zone(&s, MARGIN + PW - 50., MARGIN + 29.));
        assert!(!drag_zone(&s, MARGIN + 120., MARGIN + 150.));
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
    fn header_collapses_except_its_buttons() {
        let mut s = sample();
        open(&mut s);
        assert_eq!(hit(&s, MARGIN + 30., MARGIN + 20.), Action::Header);
        let (x, y) = center_of(&s, Action::Preferences);
        assert_eq!(hit(&s, x, y), Action::Preferences);
        let (x, y) = center_of(&s, Action::Toggle);
        assert_eq!(hit(&s, x, y), Action::Toggle);
        let (x, y) = center_of(&s, Action::ToTray);
        assert_eq!(hit(&s, x, y), Action::ToTray);
    }
    #[test]
    fn quit_asks_first_and_cancel_returns_to_settings() {
        let mut s = initial(true, 0., 0);
        s.open = true;
        navigate(&mut s, View::Preferences, false);
        activate(&mut s, Action::Quit);
        assert!(matches!(s.view, View::QuitConfirm));
        assert_eq!(s.focus, Action::Cancel);
        activate(&mut s, Action::Cancel);
        assert!(matches!(s.view, View::Preferences));
    }
    #[test]
    fn detected_handoff_folders_are_collected_once_and_cleared() {
        let mut s = initial(true, 0., 0);
        let dir = std::env::current_dir().unwrap().join("target").join(format!(
            "handoff-collect-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let (tx, rx) = mpsc::channel();
        s.handoff_rx = Some(rx);
        activate(&mut s, Action::CopyHandoff);
        std::fs::write(dir.join("HANDOFF.md"), b"x").unwrap();
        tx.send(dir.join("HANDOFF.md")).unwrap();
        tx.send(dir.join("handoff.md")).unwrap();
        tx.send(dir.join("other.md")).unwrap();
        collect_handoffs(&mut s);
        assert_eq!(s.handoff_folders, vec![dir.to_string_lossy().into_owned()]);
        activate(&mut s, Action::DismissHandoff);
        assert!(s.handoff_folders.is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }
    #[test]
    fn cowork_link_is_encoded_and_carries_the_folder() {
        let link = cowork_link(Some("Read HANDOFF.md & go"), Some("D:\\My Work\\شغل"));
        assert!(link.starts_with("claude://cowork/new?q=Read%20HANDOFF.md%20%26%20go&folder="));
        assert!(link.ends_with("D%3A%5CMy%20Work%5C%D8%B4%D8%BA%D9%84"));
        assert_eq!(cowork_link(Some("x"), None), "claude://cowork/new?q=x");
        // What the app opens: the folder only, no prompt (so no caution note).
        assert_eq!(cowork_link(None, Some("C:\\w")), "claude://cowork/new?folder=C%3A%5Cw");
        assert_eq!(cowork_link(None, Some("")), "claude://cowork/new");
    }
    #[test]
    fn handoff_banner_appears_only_after_a_switch_that_followed_a_copy() {
        let mut s = initial(true, 0., 0);
        activate(&mut s, Action::CopyHandoff);
        assert!(s.handoff_copied && !s.handoff_ready);
        activate(&mut s, Action::CopyContinue);
        assert!(!s.handoff_copied && !s.handoff_ready);
        activate(&mut s, Action::CopyHandoff);
        activate(&mut s, Action::DismissHandoff);
        assert!(!s.handoff_copied && !s.handoff_ready);
    }
    #[test]
    fn entries_holding_the_same_account_are_flagged() {
        let mut s = initial(true, 0., 0);
        s.identities = vec![Some("a".into()), Some("a".into()), Some("b".into())];
        assert_eq!(s.duplicate_of(0), Some(1));
        assert_eq!(s.duplicate_of(1), Some(0));
        assert_eq!(s.duplicate_of(2), None);
        s.identities[1] = None;
        assert_eq!(s.duplicate_of(0), None);
    }
    #[test]
    fn minimize_to_tray_hides_island_and_keeps_a_way_back() {
        let mut s = initial(true, 0., 0);
        s.settings.tray = false;
        s.open = true;
        activate(&mut s, Action::ToTray);
        assert!(!s.open);
        assert!(!s.island_visible());
        assert!(s.settings.show_island, "minimizing does not change the preference");
        assert!(s.settings.tray);
        restore_from_tray(&mut s);
        assert!(s.island_visible());
        activate(&mut s, Action::ToTray);
        activate(&mut s, Action::ShowIsland);
        assert!(!s.island_visible());
        activate(&mut s, Action::ShowIsland);
        assert!(s.island_visible());
        activate(&mut s, Action::TrayIcon);
        assert!(!s.settings.tray);
        activate(&mut s, Action::ShowIsland);
        assert!(!s.settings.show_island);
        assert!(s.settings.tray, "hiding the island turns the tray icon back on");
    }
    #[test]
    fn view_transitions_track_direction() {
        let mut s = initial(true, 0., 0);
        navigate(&mut s, View::Preferences, false);
        assert!(!s.back);
        assert_eq!(s.previous, Some(View::Home));
        navigate(&mut s, View::Home, true);
        assert!(s.back);
    }
    #[test]
    fn removing_from_details_returns_to_details_on_cancel() {
        let mut s = initial(true, 0., 0);
        activate(&mut s, Action::Details(1));
        activate(&mut s, Action::Remove);
        assert!(matches!(s.view, View::Remove(1, _)));
        activate(&mut s, Action::Cancel);
        assert!(matches!(s.view, View::Details(1)));
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
    #[test]
    fn metric_chooses_the_requested_window() {
        let mut s = initial(true, 0., 0);
        s.ring_session = Some(20.);
        s.ring_weekly = Some(60.);
        assert_eq!(s.metric(), Some(20.));
        s.settings.circle_metric = 1;
        assert_eq!(s.metric(), Some(60.));
        s.settings.circle_metric = 2;
        assert_eq!(s.metric(), Some(60.));
        s.settings.circle_metric = 0;
        s.ring_session = None;
        assert_eq!(s.metric(), Some(60.));
    }
}
#[cfg(test)]
mod bench {
    use super::*;
    #[test]
    #[ignore]
    fn frame_cost() {
        let mut s = initial(true, 0., 0);
        let gfx = Rc::new(Gfx::new().unwrap());
        s.gfx = Some(gfx.clone());
        s.scale = 1.5;
        s.open = true;
        s.w.snap(PW);
        s.h.snap(PH);
        let mut slot = None;
        let mut time = |s: &mut State, name: &str| {
            let k = s.scale;
            let ww = ((s.w.value + 2. * MARGIN) * k).ceil() as i32;
            let wh = ((s.h.value + 2. * MARGIN) * k).ceil() as i32;
            let start = Instant::now();
            for _ in 0..120 {
                let surface = Surface::reuse(&mut slot, ww, wh).unwrap();
                let mut mem = std::mem::take(&mut s.mem);
                ui::draw_main(s, &mut mem, &gfx, surface);
                s.mem = mem;
            }
            eprintln!(
                "{name}: {:.2} ms/frame",
                start.elapsed().as_secs_f64() * 1000. / 120.
            );
        };
        for (name, view) in [("home", View::Home), ("prefs", View::Preferences)] {
            s.view = view;
            time(&mut s, name);
        }
        s.view = View::Home;
        s.w.snap(PW * 0.7);
        s.h.snap(PH * 0.7);
        time(&mut s, "morph");
        s.open = false;
        s.w.snap(CIRCLE);
        s.h.snap(CIRCLE);
        time(&mut s, "circle");
    }
}
