//! Immediate-mode drawing of every Muisland surface. Each frame both paints and
//! records interactive regions, so layout and hit-testing can never drift apart.
use crate::{
    accounts,
    anim::{approach, ease_out, lerp, smoothstep},
    app::{self, Action, State, View, BH, BMARGIN, BW, HEADER, MARGIN, PH, PW},
    gfx::{font, hex, r, Align, Canvas, Color, Gfx, Surface, R},
    theme::{self, Theme},
    usage::Window,
};
use std::{collections::HashMap, f32::consts::TAU, time::Instant};

type C<'a> = Canvas<'a, Action>;
const PAD: f32 = 16.;
const CT: f32 = 108.;
const CB: f32 = PH - 56.;
const CARD_H: f32 = 124.;
const CARD_GAP: f32 = 10.;

#[derive(Default)]
pub struct Mem {
    pub hits: Vec<(R, Action)>,
    sliders: Vec<(Action, R)>,
    anims: HashMap<(u8, Action), f32>,
    pub scroll: [f32; 2],
    pub scroll_target: [f32; 2],
    scroll_max: [f32; 2],
    /// Content-space (top, height) of focusable rows in each scroll view.
    rows: [Vec<(Action, f32, f32)>; 2],
    reveal: Option<Action>,
    pub busy: bool,
    /// Snap every animation (tests and previews).
    pub instant: bool,
    reduced: bool,
    dt: f32,
    last: Option<Instant>,
    /// Deterministic frame time for offline motion renders.
    pub fixed_dt: Option<f32>,
}
impl Mem {
    pub fn slider_track(&self, a: Action) -> Option<R> {
        self.sliders.iter().find(|(x, _)| *x == a).map(|(_, r)| *r)
    }
    pub fn reset_scroll(&mut self, i: usize) {
        self.scroll[i] = 0.;
        self.scroll_target[i] = 0.;
    }
    pub fn scroll_by(&mut self, i: usize, d: f32) {
        self.scroll_target[i] = (self.scroll_target[i] + d).clamp(0., self.scroll_max[i].max(0.));
        if self.instant {
            self.scroll[i] = self.scroll_target[i];
        }
        self.busy = true;
    }
    pub fn scroll_to_end(&mut self, i: usize) {
        self.scroll_target[i] = f32::MAX / 4.;
    }
    pub fn ensure_visible(&mut self, a: Action) {
        self.reveal = Some(a);
    }
    fn ease(&mut self, tag: u8, a: Action, target: f32, rate: f32, initial: Option<f32>) -> f32 {
        let snap = self.instant || self.reduced;
        let dt = self.dt;
        let v = self
            .anims
            .entry((tag, a))
            .or_insert(initial.unwrap_or(target));
        if snap {
            *v = target;
        } else if approach(v, target, rate, dt) {
            self.busy = true;
        }
        *v
    }
    fn hover(&mut self, s: &State, a: Action) -> f32 {
        let on = s.hover == a && s.open_hover_ok();
        self.ease(0, a, if on { 1. } else { 0. }, 18., Some(0.))
    }
    fn begin_frame(&mut self, reduced: bool) {
        let now = Instant::now();
        self.dt = self
            .last
            .map(|l| now.duration_since(l).as_secs_f32())
            .unwrap_or(1. / 60.)
            .clamp(0., 1. / 30.);
        self.last = Some(now);
        if let Some(dt) = self.fixed_dt {
            self.dt = dt;
        }
        self.busy = false;
        self.reduced = reduced;
        self.sliders.clear();
    }
    /// Advances scrolling toward the target; `finish_scroll` clamps the target
    /// once the view has measured its content.
    fn step_scroll(&mut self, i: usize) -> f32 {
        let max = self.scroll_max[i].max(0.);
        self.scroll_target[i] = self.scroll_target[i].max(0.);
        if self.instant || self.reduced {
            self.scroll[i] = self.scroll_target[i];
        } else if approach(&mut self.scroll[i], self.scroll_target[i], 18., self.dt) {
            self.busy = true;
        }
        self.scroll[i] = self.scroll[i].clamp(0., max);
        self.scroll[i]
    }
    fn finish_scroll(&mut self, i: usize, max: f32) {
        self.scroll_max[i] = max.max(0.);
        self.scroll_target[i] = self.scroll_target[i].clamp(0., self.scroll_max[i]);
    }
}
impl State {
    fn open_hover_ok(&self) -> bool {
        self.slider_drag.is_none() && !self.drag
    }
}

// ---------------------------------------------------------------- helpers

fn pressed(s: &State, a: Action) -> bool {
    s.down.is_some_and(|(_, _, d)| d == a) && s.hover == a
}
fn surface_hover(t: &Theme, h: f32) -> Color {
    t.surface.mix(t.raised, 0.55 * h)
}
fn panel_bg(t: &Theme) -> Color {
    t.bg.mix(t.surface, 0.35)
}
fn focus_ring(c: &C, s: &State, b: R, radius: f32, a: Action) {
    if s.focus == a {
        let t = s.theme();
        c.stroke(
            r(b.x - 3., b.y - 3., b.w + 6., b.h + 6.),
            radius + 3.,
            2.,
            t.accent.alpha(0.85),
        );
    }
}
fn age(s: &State, seconds: u64) -> String {
    if seconds < 60 {
        s.tr("just now").into()
    } else if seconds < 3600 {
        s.tr("{n}m ago").replace("{n}", &(seconds / 60).to_string())
    } else if seconds < 86400 {
        s.tr("{n}h ago")
            .replace("{n}", &(seconds / 3600).to_string())
    } else {
        s.tr("{n}d ago")
            .replace("{n}", &(seconds / 86400).to_string())
    }
}
fn reset_label(s: &State, w: Option<&Window>) -> String {
    let now = accounts::now();
    match w {
        None => s.tr("Not available").into(),
        Some(w) => match w.resets {
            None => s.tr("Reset time unavailable").into(),
            Some(t) if t <= now => s.tr("Reset due · refreshing").into(),
            Some(t) => {
                let minutes = (t - now).div_ceil(60);
                let span = if minutes >= 1440 {
                    s.tr("{d}d {h}h")
                        .replace("{d}", &(minutes / 1440).to_string())
                        .replace("{h}", &(minutes % 1440 / 60).to_string())
                } else if minutes >= 60 {
                    s.tr("{h}h {m}m")
                        .replace("{h}", &(minutes / 60).to_string())
                        .replace("{m}", &(minutes % 60).to_string())
                } else {
                    s.tr("{m}m").replace("{m}", &minutes.to_string())
                };
                s.tr("Resets in {t}").replace("{t}", &span)
            }
        },
    }
}
fn initials(name: &str) -> String {
    let words: Vec<&str> = name.split_whitespace().collect();
    let mut out: String = words
        .iter()
        .take(2)
        .filter_map(|w| w.chars().next())
        .collect();
    if out.is_empty() {
        out = "?".into();
    }
    out.to_uppercase()
}
fn percent(v: f64) -> String {
    format!("{}%", v.round().clamp(0., 100.) as i32)
}

#[derive(Clone, Copy, PartialEq)]
pub enum Icon {
    Gear,
    Close,
    Back,
    More,
    Plus,
    Info,
    Warning,
    Swap,
    Trash,
    Check,
    Expand,
    Tray,
}
/// Line icons on a 20-unit grid centred at (cx, cy).
fn icon(c: &C, kind: Icon, cx: f32, cy: f32, size: f32, col: Color, spin: f32) {
    let k = size / 20.;
    let p = |x: f32, y: f32| (cx + x * k, cy + y * k);
    let w = 1.7 * k.max(0.8);
    match kind {
        Icon::Gear => {
            for i in 0..8 {
                let a = spin + i as f32 * TAU / 8.;
                let (sn, cs) = a.sin_cos();
                c.line(
                    cx + sn * 6.2 * k,
                    cy - cs * 6.2 * k,
                    cx + sn * 8.6 * k,
                    cy - cs * 8.6 * k,
                    2.6 * k,
                    col,
                );
            }
            c.ring(cx, cy, 5.6 * k, w, col);
            c.ring(cx, cy, 2.2 * k, w, col);
        }
        Icon::Close => {
            c.line(
                p(-5., -5.).0,
                p(-5., -5.).1,
                p(5., 5.).0,
                p(5., 5.).1,
                w,
                col,
            );
            c.line(
                p(5., -5.).0,
                p(5., -5.).1,
                p(-5., 5.).0,
                p(-5., 5.).1,
                w,
                col,
            );
        }
        Icon::Back => c.polyline(&[p(3., -6.), p(-3., 0.), p(3., 6.)], w, col, false),
        Icon::More => {
            for x in [-6., 0., 6.] {
                c.circle(p(x, 0.).0, cy, 1.6 * k, col);
            }
        }
        Icon::Plus => {
            c.line(p(0., -6.).0, p(0., -6.).1, p(0., 6.).0, p(0., 6.).1, w, col);
            c.line(p(-6., 0.).0, cy, p(6., 0.).0, cy, w, col);
        }
        Icon::Info => {
            c.line(cx, cy - 1. * k, cx, cy + 6. * k, w * 1.15, col);
            c.circle(cx, cy - 5.5 * k, 1.5 * k, col);
        }
        Icon::Warning => {
            c.line(cx, cy - 6. * k, cx, cy + 1.5 * k, w * 1.15, col);
            c.circle(cx, cy + 6. * k, 1.5 * k, col);
        }
        Icon::Swap => {
            c.polyline(&[p(-7., -3.), p(6., -3.)], w, col, false);
            c.polyline(&[p(2., -7.), p(6., -3.), p(2., 1.)], w, col, false);
            c.polyline(&[p(7., 3.), p(-6., 3.)], w, col, false);
            c.polyline(&[p(-2., -1.), p(-6., 3.), p(-2., 7.)], w, col, false);
        }
        Icon::Trash => {
            c.polyline(&[p(-7., -4.), p(7., -4.)], w, col, false);
            c.polyline(
                &[p(-3., -4.), p(-2., -7.), p(2., -7.), p(3., -4.)],
                w,
                col,
                false,
            );
            c.polyline(
                &[p(-5., -4.), p(-4., 7.), p(4., 7.), p(5., -4.)],
                w,
                col,
                false,
            );
        }
        Icon::Check => c.polyline(&[p(-6., 0.), p(-2., 4.5), p(6.5, -5.)], w * 1.1, col, false),
        Icon::Expand => c.polyline(&[p(-5., -2.5), p(0., 2.5), p(5., -2.5)], w, col, false),
        Icon::Tray => {
            // Arrow dropping into a tray.
            c.line(p(0., -7.).0, p(0., -7.).1, p(0., 1.5).0, p(0., 1.5).1, w, col);
            c.polyline(&[p(-3.6, -2.), p(0., 1.8), p(3.6, -2.)], w, col, false);
            c.polyline(&[p(-7., 2.), p(-7., 6.5), p(7., 6.5), p(7., 2.)], w, col, false);
        }
    }
}
/// The Muisland "M" mark.
fn logo(c: &C, cx: f32, cy: f32, size: f32, col: Color, width: f32) {
    let h = size * 0.5;
    let w = size * 0.56;
    c.polyline(
        &[
            (cx - w, cy + h * 0.9),
            (cx - w, cy - h * 0.9),
            (cx, cy + h * 0.25),
            (cx + w, cy - h * 0.9),
            (cx + w, cy + h * 0.9),
        ],
        width,
        col,
        false,
    );
}

#[derive(Clone, Copy, PartialEq)]
enum Style {
    Primary,
    Secondary,
    Ghost,
    Danger,
}
#[allow(clippy::too_many_arguments)]
fn button(
    c: &mut C,
    s: &State,
    m: &mut Mem,
    b: R,
    a: Action,
    label: &str,
    style: Style,
    ic: Option<Icon>,
) {
    let t = s.theme();
    let h = m.hover(s, a);
    let down = pressed(s, a);
    let b = if down { b.inset(0.8) } else { b };
    let radius = (b.h / 2.).min(13.);
    let fg = match style {
        Style::Primary => {
            c.fill_gradient(
                b,
                radius,
                t.accent.mix(hex(0xFFFFFF), 0.10 * h),
                t.accent2.mix(hex(0xFFFFFF), 0.10 * h),
                false,
            );
            t.on_accent
        }
        Style::Secondary => {
            c.fill(b, radius, t.raised.mix(t.text, 0.06 * h));
            t.text
        }
        Style::Ghost => {
            c.fill(b, radius, t.raised.alpha(h));
            t.text.mix(t.muted, 0.35 * (1. - h))
        }
        Style::Danger => {
            c.fill(b, radius, t.danger.alpha(0.12 + 0.08 * h));
            t.danger
        }
    };
    let f = font(13.5, 650);
    let lw = if label.is_empty() {
        0.
    } else {
        c.measure(label, f)
    };
    let iw = if ic.is_some() { 16. } else { 0. };
    let gap = if ic.is_some() && lw > 0. { 7. } else { 0. };
    let total = (lw + iw + gap).min(b.w - 16.);
    let x0 = b.cx() - total / 2.;
    if let Some(ic) = ic {
        icon(c, ic, x0 + 8., b.cy(), 16., fg, 0.);
    }
    if lw > 0. {
        // Labels sit in their own box so RTL mirroring keeps icon/label order.
        c.text(
            label,
            r(x0 + iw + gap, b.y, total - iw - gap + 2., b.h),
            f,
            fg,
            Align::Start,
        );
    }
    focus_ring(c, s, b, radius, a);
    c.hit(b, a);
}
fn icon_button(c: &mut C, s: &State, m: &mut Mem, b: R, a: Action, ic: Icon, spin: f32, on: f32) {
    let t = s.theme();
    let h = m.hover(s, a).max(on * 0.7);
    c.fill(b, b.h / 2.6, t.raised.alpha(h));
    let col = t.muted.mix(t.text, h).mix(t.accent, on);
    icon(c, ic, b.cx(), b.cy(), 18., col, spin);
    focus_ring(c, s, b, b.h / 2.6, a);
    c.hit(b, a);
}
fn toggle_row(
    c: &mut C,
    s: &State,
    m: &mut Mem,
    y: f32,
    a: Action,
    label: &str,
    caption: Option<&str>,
    on: bool,
) -> f32 {
    let t = s.theme();
    let h_row = if caption.is_some() { 60. } else { 48. };
    let b = r(PAD, y, PW - 2. * PAD, h_row);
    let hover = m.hover(s, a);
    c.fill(b, 14., surface_hover(t, hover));
    let text_w = b.w - 84.;
    if let Some(cap) = caption {
        c.text(
            label,
            r(b.x + 14., y + 10., text_w, 20.),
            font(13.5, 550),
            t.text,
            Align::Start,
        );
        c.text(
            cap,
            r(b.x + 14., y + 31., text_w, 17.),
            font(11.5, 400),
            t.muted,
            Align::Start,
        );
    } else {
        c.text(
            label,
            r(b.x + 14., y, text_w, h_row),
            font(13.5, 550),
            t.text,
            Align::Start,
        );
    }
    let k = m.ease(4, a, if on { 1. } else { 0. }, 16., None);
    let track = r(b.right() - 14. - 42., y + h_row / 2. - 12., 42., 24.);
    c.fill(track, 12., t.raised.mix(t.line, 0.6));
    if k > 0.01 {
        let tk = r(track.x, track.y, track.w, track.h);
        c.alpha *= k;
        c.fill_gradient(tk, 12., t.accent, t.accent2, false);
        c.alpha /= k.max(0.0001);
    }
    let knob_x = lerp(track.x + 12., track.right() - 12., ease_out(k));
    let knob = hex(0xFFFFFF);
    c.circle(knob_x, track.cy() + 0.6, 9.5, hex(0x000000).alpha(0.18));
    c.circle(knob_x, track.cy(), 9., knob.mix(t.on_accent, 0.04 * k));
    focus_ring(c, s, b, 14., a);
    c.hit(b, a);
    h_row
}
fn slider_row(c: &mut C, s: &State, m: &mut Mem, y: f32, a: Action, label: &str) -> f32 {
    let t = s.theme();
    let (lo, hi) = app::slider_bounds(a).unwrap_or((0., 1.));
    let target = app::slider_value(s, a);
    let v = m.ease(5, a, target, 22., None);
    let frac = ((v - lo) / (hi - lo)).clamp(0., 1.);
    let hover = m
        .hover(s, a)
        .max(if s.slider_drag == Some(a) { 1. } else { 0. });
    c.text(
        label,
        r(PAD, y, 132., 36.),
        font(13., 550),
        t.text,
        Align::Start,
    );
    let track = r(PAD + 138., y + 15., PW - 2. * PAD - 138. - 54., 6.);
    c.fill(track, 3., t.raised.mix(t.line, 0.6));
    if frac > 0. {
        c.fill_gradient(
            r(track.x, track.y, (track.w * frac).max(6.), track.h),
            3.,
            t.accent,
            t.accent2,
            false,
        );
    }
    let kx = track.x + track.w * frac;
    let kr = 8. + 1.5 * hover;
    c.circle(kx, track.cy() + 0.8, kr + 1., hex(0x000000).alpha(0.22));
    c.circle(kx, track.cy(), kr, hex(0xFFFFFF));
    c.circle(kx, track.cy(), 3., t.accent.alpha(hover));
    c.text(
        &format!("{}%", (target * 100.).round() as i32),
        r(PW - PAD - 50., y, 50., 36.),
        font(12.5, 650),
        t.accent,
        Align::End,
    );
    let hit = r(track.x - 10., y, track.w + 20., 36.);
    focus_ring(c, s, r(track.x - 6., y + 6., track.w + 12., 24.), 12., a);
    c.hit(hit, a);
    m.sliders.push((a, c.map(track)));
    36.
}
fn segmented(
    c: &mut C,
    s: &State,
    m: &mut Mem,
    b: R,
    items: &[(&str, Action)],
    selected: usize,
    key: Action,
) {
    let t = s.theme();
    c.fill(b, 11., t.bg.mix(t.raised, 0.5));
    let n = items.len().max(1) as f32;
    let cw = (b.w - 6.) / n;
    let pos = m.ease(3, key, selected as f32, 18., None);
    let ind = r(b.x + 3. + pos * cw, b.y + 3., cw, b.h - 6.);
    c.shadow(ind, 8., 3., if t.dark { 0.5 } else { 0.3 }, t.shadow);
    c.fill(
        ind,
        8.5,
        t.raised.mix(t.text, if t.dark { 0.06 } else { 0. }),
    );
    if !t.dark {
        c.fill(ind, 8.5, hex(0xFFFFFF));
    }
    for (i, (label, a)) in items.iter().enumerate() {
        let cell = r(b.x + 3. + i as f32 * cw, b.y + 3., cw, b.h - 6.);
        let near = 1. - (pos - i as f32).abs().min(1.);
        let h = m.hover(s, *a);
        let col = t.muted.mix(t.text, near.max(h * 0.6));
        c.text(label, cell, font(12.5, 600), col, Align::Center);
        focus_ring(c, s, cell, 8., *a);
        c.hit(cell, *a);
    }
}
fn chip(c: &C, x: f32, y: f32, label: &str, bg: Color, fg: Color) -> f32 {
    let f = font(11., 650);
    let w = c.measure(label, f) + 16.;
    c.fill(r(x, y, w, 20.), 10., bg);
    c.text(label, r(x + 8., y, w - 16. + 2., 20.), f, fg, Align::Start);
    w
}
fn avatar(c: &C, s: &State, name: &str, cx: f32, cy: f32, radius: f32) {
    let t = s.theme();
    let (a, b) = theme::avatar(name, t);
    c.circle_gradient(cx, cy, radius, a, b);
    c.text(
        &initials(name),
        r(cx - radius, cy - radius, radius * 2., radius * 2.),
        font(radius * 0.68, 700),
        hex(0xFFFFFF),
        Align::Center,
    );
}
/// Labelled quota meter: name, percentage, gradient bar and reset time.
fn meter(c: &C, s: &State, b: R, label: &str, w: Option<&Window>, big: bool) {
    let t = s.theme();
    let stale = w.is_some_and(|w| w.resets.is_some_and(|r| r <= accounts::now()));
    let fsize = if big { 13. } else { 12. };
    c.text(
        label,
        r(b.x, b.y, b.w * 0.6, 18.),
        font(fsize - 0.5, 600),
        t.muted,
        Align::Start,
    );
    let pct = w.map(|w| percent(w.used)).unwrap_or_else(|| "—".into());
    c.text(
        &pct,
        r(b.x, b.y, b.w, 18.),
        font(fsize + 1., 700),
        t.text,
        Align::End,
    );
    let bar = r(b.x, b.y + 23., b.w, if big { 7. } else { 6. });
    c.fill(bar, bar.h / 2., t.raised.mix(t.line, 0.5));
    if let Some(w) = w {
        let (ca, cb) = if stale {
            (t.muted, t.muted)
        } else {
            t.usage(w.used)
        };
        let fw =
            (b.w * (w.used as f32 / 100.).clamp(0., 1.)).max(if w.used > 0. { bar.h } else { 0. });
        if fw > 0. {
            c.fill_gradient(r(bar.x, bar.y, fw, bar.h), bar.h / 2., ca, cb, false);
        }
    }
    c.text(
        &reset_label(s, w),
        r(b.x, bar.bottom() + 4., b.w, 16.),
        font(11., 400),
        t.muted,
        Align::Start,
    );
}
/// Ring gauge with the percentage in its centre.
fn gauge(
    c: &C,
    s: &State,
    cx: f32,
    cy: f32,
    radius: f32,
    width: f32,
    value: Option<f64>,
    size: f32,
) {
    let t = s.theme();
    c.ring(
        cx,
        cy,
        radius,
        width,
        t.text.alpha(if t.dark { 0.08 } else { 0.07 }),
    );
    if let Some(v) = value {
        let (a, b) = t.usage(v);
        c.arc_gradient(
            cx,
            cy,
            radius,
            width,
            0.,
            TAU * (v as f32 / 100.).clamp(0., 1.),
            a,
            b,
        );
    }
    let label = value.map(percent).unwrap_or_else(|| "—".into());
    c.text(
        &label,
        r(cx - radius, cy - radius, radius * 2., radius * 2.),
        font(size, 700),
        t.text,
        Align::Center,
    );
}

// ------------------------------------------------------------ main window

pub fn draw_main(s: &State, m: &mut Mem, g: &Gfx, surface: &Surface) {
    m.begin_frame(s.settings.reduced_motion);
    let Some(mut c) = Canvas::begin(g, surface, s.scale, 0., 0.) else {
        return;
    };
    let t = s.theme();
    let (w, h) = (s.w.value, s.h.value);
    let p = s.openness();
    let (cw, ch) = s.collapsed_size();
    let collapsed_alpha = 1. - smoothstep(0.02, 0.25, p);
    let panel_alpha = smoothstep(0.5, 0.95, p);
    let peek = m.ease(
        1,
        Action::Toggle,
        if s.peek && !s.open { 1. } else { 0. },
        14.,
        Some(0.),
    );
    let now = s.now();
    let pulse = if s.pulse_until > now && !s.settings.reduced_motion {
        ((s.pulse_until - now) * 7.).sin().abs() * smoothstep(0., 0.6, s.pulse_until - now)
    } else {
        0.
    };
    let zoom = 1. + (0.05 * peek + 0.04 * pulse) * collapsed_alpha;
    c.set_zoom(zoom, MARGIN + w / 2., MARGIN + h / 2.);
    let rc = cw.min(ch) / 2.;
    let radius = lerp(rc, 26., smoothstep(0., 1., p)).min(w / 2.).min(h / 2.);
    c.ox = MARGIN;
    c.oy = MARGIN;
    let shape = r(0., 0., w, h);
    c.shadow(
        shape,
        radius,
        10. + 6. * p,
        if t.dark { 1. } else { 0.5 },
        t.shadow,
    );
    if p > 0.5 {
        c.fill(shape, radius, panel_bg(t));
    } else {
        c.fill_gradient(shape, radius, t.surface.mix(t.raised, 0.35), t.bg, true);
    }
    c.stroke(
        shape,
        radius,
        1.,
        t.text.alpha(if t.dark { 0.07 } else { 0.09 }),
    );
    if pulse > 0. {
        let (a, _) = t.usage(s.metric().unwrap_or(90.));
        c.ring(
            w / 2.,
            h / 2.,
            rc + 2.5 + 2. * pulse,
            2.,
            a.alpha(0.6 * pulse),
        );
    }
    if collapsed_alpha > 0.01 {
        c.alpha = collapsed_alpha;
        c.interactive = !s.open;
        if s.settings.circle {
            draw_circle(&mut c, s, w, h, peek);
        } else {
            draw_pill(&mut c, s, w, h, peek);
        }
        c.hit(shape, Action::Toggle);
        c.alpha = 1.;
    }
    if panel_alpha > 0.01 {
        // The clip layer is costly; once fully open all content sits inside
        // the rounded corners, so it is only needed while morphing.
        let clip = (w - PW).abs() > 0.5 || (h - PH).abs() > 0.5;
        if clip {
            c.push_round_clip(r(MARGIN, MARGIN, w, h), radius);
        }
        c.alpha = panel_alpha;
        c.interactive = s.open && p > 0.92;
        c.ox = MARGIN + (w - PW) / 2.;
        c.oy = MARGIN;
        c.rtl = s.rtl();
        c.mirror = PW;
        draw_panel(&mut c, s, m);
        c.rtl = false;
        if clip {
            c.pop_layer();
        }
    }
    c.set_zoom(1., 0., 0.);
    m.hits = c.end();
}

fn draw_circle(c: &mut C, s: &State, w: f32, h: f32, peek: f32) {
    let t = s.theme();
    let d = w.min(h);
    let (cx, cy) = (w / 2., h / 2.);
    let k = d / app::CIRCLE;
    let track = t.text.alpha(0.08);
    let outer = d / 2. - 5.5 * k;
    let inner = d / 2. - 12.8 * k;
    c.ring(cx, cy, outer, 3.2 * k, track);
    c.ring(cx, cy, inner, 5.2 * k, track);
    if let Some(v) = s.ring_weekly {
        let (_, b) = t.usage(v);
        c.arc(
            cx,
            cy,
            outer,
            3.2 * k,
            0.,
            TAU * (v as f32 / 100.).clamp(0., 1.),
            b.alpha(0.92),
        );
    }
    if let Some(v) = s.ring_session {
        let (a, b) = t.usage(v);
        c.arc_gradient(
            cx,
            cy,
            inner,
            5.2 * k,
            0.,
            TAU * (v as f32 / 100.).clamp(0., 1.),
            a,
            b,
        );
    }
    match s.metric().filter(|_| s.settings.circle_numbers) {
        Some(v) => {
            let text = format!("{}", v.round().clamp(0., 100.) as i32);
            let size = if v >= 99.5 { 14.5 } else { 17. } * k;
            let nf = font(size, 750);
            let pf = font(size * 0.56, 700);
            let nw = c.measure(&text, nf);
            let pw = c.measure("%", pf);
            let x0 = cx - (nw + pw) / 2.;
            let y = cy - 4.5 * k - peek * 0.5;
            c.text(
                &text,
                r(x0, y - 11. * k, nw + 2., 22. * k),
                nf,
                t.text,
                Align::Start,
            );
            c.text(
                "%",
                r(x0 + nw, y - 8. * k, pw + 2., 20. * k),
                pf,
                t.text.alpha(0.75),
                Align::Start,
            );
            c.text(
                s.metric_label(),
                r(cx - 20. * k, cy + 6.5 * k, 40. * k, 12. * k),
                font(8. * k, 700),
                t.muted,
                Align::Center,
            );
        }
        None => logo(c, cx, cy, 20. * k, t.text, 2.2 * k),
    }
}
fn draw_pill(c: &mut C, s: &State, w: f32, h: f32, peek: f32) {
    let t = s.theme();
    c.rtl = s.rtl();
    c.mirror = w;
    let (cx, cy) = (h / 2. + 2., h / 2.);
    let rr = h / 2. - 9.;
    c.ring(cx, cy, rr, 4., t.text.alpha(0.08));
    if let Some(v) = s.metric() {
        let (a, b) = t.usage(v);
        c.arc_gradient(
            cx,
            cy,
            rr,
            4.,
            0.,
            TAU * (v as f32 / 100.).clamp(0., 1.),
            a,
            b,
        );
        c.text(
            &format!("{}", v.round() as i32),
            r(cx - rr, cy - rr, rr * 2., rr * 2.),
            font(12., 750),
            t.text,
            Align::Center,
        );
    } else {
        logo(c, cx, cy, 13., t.text, 1.8);
    }
    let account = s.active.and_then(|i| s.settings.accounts.get(i));
    let x = h + 8.;
    let name = account
        .map(|a| a.name.as_str())
        .unwrap_or(s.tr("No linked account"));
    c.text(
        name,
        r(x, cy - 19., w - x - 34., 20.),
        font(14., 650),
        t.text,
        Align::Start,
    );
    let u = s.active_usage();
    let line = match u {
        Some(u) => format!(
            "{} {}  ·  {} {}",
            s.tr(if s.settings.provider == 1 {
                "SES"
            } else {
                "5H"
            }),
            u.session
                .as_ref()
                .map(|w| percent(w.used))
                .unwrap_or("—".into()),
            s.tr(if s.settings.provider == 1 {
                "EXT"
            } else {
                "7D"
            }),
            u.weekly
                .as_ref()
                .map(|w| percent(w.used))
                .unwrap_or("—".into())
        ),
        None => s.tr("Usage not available yet").into(),
    };
    c.text(
        &line,
        r(x, cy + 1., w - x - 34., 17.),
        font(11.5, 500),
        t.muted,
        Align::Start,
    );
    icon(
        c,
        Icon::Expand,
        w - 22.,
        cy,
        14.,
        t.muted.mix(t.text, peek),
        0.,
    );
    c.rtl = false;
}

fn draw_panel(c: &mut C, s: &State, m: &mut Mem) {
    let t = s.theme();
    let base_ox = c.ox;
    let base_alpha = c.alpha;
    let base_interactive = c.interactive;
    // Header
    c.hit(r(0., 0., PW, HEADER), Action::Header);
    c.fill_gradient(r(PAD, 14., 30., 30.), 10., t.accent, t.accent2, false);
    logo(c, PAD + 15., 29., 15., t.on_accent, 2.);
    c.text(
        "Muisland",
        r(56., 12., 200., 19.),
        font(15.5, 700),
        t.text,
        Align::Start,
    );
    let count = s.settings.accounts.iter().filter(|a| !a.draft).count();
    let accounts_text = if count == 1 {
        s.tr("1 account").to_string()
    } else {
        s.tr("{n} accounts").replace("{n}", &count.to_string())
    };
    let sub = if s.rtl() {
        format!("{accounts_text} · {}", s.provider_name())
    } else {
        format!("{} · {accounts_text}", s.provider_name())
    };
    c.text(
        &sub,
        r(56., 31., 196., 15.),
        font(11.5, 450),
        t.muted,
        Align::Start,
    );
    let in_prefs = matches!(s.view, View::Preferences);
    let spin = m.ease(
        6,
        Action::Preferences,
        if in_prefs { 1. } else { 0. },
        10.,
        None,
    );
    icon_button(
        c,
        s,
        m,
        r(PW - 130., 13., 34., 34.),
        Action::ToTray,
        Icon::Tray,
        0.,
        0.,
    );
    icon_button(
        c,
        s,
        m,
        r(PW - 90., 13., 34., 34.),
        Action::Preferences,
        Icon::Gear,
        spin * TAU / 8.,
        spin,
    );
    icon_button(
        c,
        s,
        m,
        r(PW - 50., 13., 34., 34.),
        Action::Toggle,
        Icon::Close,
        0.,
        0.,
    );
    // Provider tabs
    let tabs = r(PAD, 62., PW - 2. * PAD, 36.);
    c.fill(tabs, 12., t.bg.mix(t.raised, 0.55));
    let pos = m.ease(2, Action::ClaudeTab, s.settings.provider as f32, 16., None);
    let half = (tabs.w - 6.) / 2.;
    let ind = r(tabs.x + 3. + pos * half, tabs.y + 3., half, tabs.h - 6.);
    c.shadow(ind, 9., 3., if t.dark { 0.5 } else { 0.3 }, t.shadow);
    c.fill(
        ind,
        9.,
        if t.dark {
            t.raised.mix(t.text, 0.05)
        } else {
            hex(0xFFFFFF)
        },
    );
    for (i, (label, a, dot)) in [
        ("Claude", Action::ClaudeTab, hex(0xD97757)),
        ("Codex", Action::CodexTab, t.text.mix(t.muted, 0.3)),
    ]
    .into_iter()
    .enumerate()
    {
        let cell = r(
            tabs.x + 3. + i as f32 * half,
            tabs.y + 3.,
            half,
            tabs.h - 6.,
        );
        let near = 1. - (pos - i as f32).abs().min(1.);
        let h = m.hover(s, a);
        let f = font(13., 650);
        let lw = c.measure(label, f);
        let x0 = cell.cx() - (lw + 14.) / 2.;
        c.circle(x0 + 4., cell.cy(), 4., dot.alpha(0.45 + 0.55 * near));
        c.text(
            label,
            r(x0 + 14., cell.y, lw + 2., cell.h),
            f,
            t.muted.mix(t.text, near.max(h * 0.7)),
            Align::Start,
        );
        focus_ring(c, s, cell, 9., a);
        c.hit(cell, a);
    }
    // Views with a directional slide + fade.
    let area = r(base_ox, MARGIN + CT - 4., PW, CB - CT + 8.);
    c.push_clip(area);
    c.hit_clip = Some(area);
    let e = ease_out(s.transition);
    let dir = if s.back { -1. } else { 1. };
    let shift = 34.;
    if let Some(prev) = s.previous.as_ref().filter(|_| s.transition < 1.) {
        c.alpha = base_alpha * (1. - e).powf(1.4);
        c.ox = base_ox + if s.rtl() { 1. } else { -1. } * dir * shift * e;
        c.interactive = false;
        draw_view(c, s, m, prev);
    }
    c.alpha = base_alpha * smoothstep(0., 0.85, e);
    c.ox = base_ox + if s.rtl() { -1. } else { 1. } * dir * shift * (1. - e);
    c.interactive = base_interactive && e > 0.85;
    draw_view(c, s, m, &s.view);
    c.ox = base_ox;
    c.alpha = base_alpha;
    c.interactive = base_interactive;
    c.hit_clip = None;
    c.pop_clip();
    draw_footer(c, s, m);
}
fn draw_footer(c: &mut C, s: &State, _m: &mut Mem) {
    let t = s.theme();
    let b = r(PAD, PH - 46., PW - 2. * PAD, 32.);
    c.fill(b, 11., t.surface.alpha(0.65));
    let now = s.now();
    let (dot, text): (Color, String) = if !s.status.is_empty() {
        (if s.error { t.danger } else { t.accent }, s.status.clone())
    } else if s.worker.is_some() {
        (t.warn, s.tr("Working on your account…").into())
    } else if s.preview {
        (t.accent, s.tr("Preview · sample data").into())
    } else if !s.network_allowed {
        (t.muted, s.tr("Usage updates paused").into())
    } else if let Some(u) = s.active_usage() {
        let fresh = s
            .active
            .and_then(|i| s.usage_errors.get(i))
            .is_some_and(|e| e.is_empty());
        (
            if fresh { t.good } else { t.warn },
            s.tr("Live · updated {age}")
                .replace("{age}", &age(s, accounts::now().saturating_sub(u.updated))),
        )
    } else {
        (t.muted, s.tr("Waiting for usage…").into())
    };
    let breathe = if s.worker.is_some() || s.sync.is_some() {
        0.55 + 0.45 * (now * 4.).sin().abs()
    } else {
        1.
    };
    c.circle(b.x + 14., b.cy(), 6., dot.alpha(0.18 * breathe));
    c.circle(b.x + 14., b.cy(), 3.4, dot.alpha(breathe));
    c.text(
        &text,
        r(b.x + 27., b.y, b.w - 27. - 60., b.h),
        font(12., 500),
        if s.error {
            t.danger
        } else {
            t.text.mix(t.muted, 0.35)
        },
        Align::Start,
    );
    c.text(
        &format!("v{}", env!("CARGO_PKG_VERSION")),
        r(b.right() - 62., b.y, 52., b.h),
        font(11., 500),
        t.muted.alpha(0.8),
        Align::End,
    );
}
fn draw_view(c: &mut C, s: &State, m: &mut Mem, view: &View) {
    match view {
        View::Home => home(c, s, m),
        View::Details(i) => details(c, s, m, *i, view == &s.view),
        View::Preferences => preferences(c, s, m),
        View::Notice(title, body) => {
            let t = s.theme();
            let tone = if s.error && view == &s.view {
                t.danger
            } else {
                t.accent
            };
            let icon_kind = if s.error { Icon::Warning } else { Icon::Info };
            dialog(c, s, m, icon_kind, tone, title, body, None);
            button(
                c,
                s,
                m,
                r(PAD, CB - 48., PW - 2. * PAD, 46.),
                Action::Back,
                s.tr("Got it"),
                Style::Primary,
                None,
            );
        }
        View::SwitchConfirm(i) => {
            let body = s.tr(if s.settings.provider == 1 {
                "Switch the Codex file sign-in? New CLI sessions use this account. Restart open sessions to reload. The Desktop host may keep its own login."
            } else {
                "Confirm to open this saved account. If Claude is running, it will close first. Local tasks may stop and unsaved work may be lost."
            });
            dialog(
                c,
                s,
                m,
                Icon::Swap,
                s.theme().accent,
                s.tr("Switch account?"),
                body,
                Some(*i),
            );
            confirm_buttons(c, s, m, s.tr("Confirm switch"), Action::Confirm, false);
        }
        View::Confirm(op) => {
            let saving = matches!(op, app::Op::Save(_));
            // Another slot already holds the signed-in account: this Save adds a new one.
            let adding = s.settings.provider == 0
                && matches!(op, app::Op::Save(i) if s.active.is_some_and(|active| {
                    active != *i && s.identities.get(active) != s.identities.get(*i)
                }));
            let title = s.tr(if adding {
                "Add another account?"
            } else if matches!(op, app::Op::Rollback) {
                "Restore previous sign-in?"
            } else if saving {
                "Close Claude & save?"
            } else {
                "Close Claude & switch?"
            });
            let body = s.tr(if s.settings.provider == 1 {
                "Switch the Codex sign-in cache? New Codex CLI sessions use this account. Restart existing sessions to reload it; the Desktop app may retain its own host login."
            } else if adding {
                "Claude's current account is already saved. Claude will close and reopen at its sign-in screen so you can add another one. The saved account is not signed out."
            } else {
                "Claude Desktop will close. Local tasks may stop and unsaved work may be lost. Nothing closes until you confirm."
            });
            let account = match op {
                app::Op::Save(i) | app::Op::Switch(i) => Some(*i),
                _ => None,
            };
            dialog(c, s, m, Icon::Warning, s.theme().warn, title, body, account);
            let label = s.tr(if adding {
                "Close & sign in"
            } else if matches!(op, app::Op::Rollback) {
                "Close & restore"
            } else if saving {
                "Close & save"
            } else {
                "Close & switch"
            });
            confirm_buttons(c, s, m, label, Action::Confirm, false);
        }
        View::Remove(i, name) => {
            let body = s
                .tr("Remove {name} from Muisland? The current sign-in, chats and files remain unchanged. The saved sign-in is moved to an encrypted local archive.")
                .replace("{name}", name);
            dialog(
                c,
                s,
                m,
                Icon::Trash,
                s.theme().danger,
                s.tr("Remove this account?"),
                &body,
                Some(*i),
            );
            confirm_buttons(c, s, m, s.tr("Remove account"), Action::RemoveConfirm, true);
        }
        View::AutoSwitch(i) => {
            let t = s.theme();
            let name = s
                .settings
                .accounts
                .get(*i)
                .map(|a| a.name.as_str())
                .unwrap_or(s.tr("another account"));
            let body = s.tr("Switch to {name}? Claude may close and running projects or unsaved work may stop. Muisland cannot reliably tell whether a task is idle. Codex sessions need a restart to use the new sign-in.").replace("{name}", name);
            dialog(
                c,
                s,
                m,
                Icon::Swap,
                t.warn,
                s.tr("Usage limit reached"),
                &body,
                Some(*i),
            );
            let y = CB - 48. - 46.;
            match s.auto_due {
                Some(due) => {
                    let left = (due - s.now()).max(0.);
                    let cx = PAD + 16.;
                    c.ring(cx, y + 16., 13., 3., t.text.alpha(0.08));
                    c.arc(
                        cx,
                        y + 16.,
                        13.,
                        3.,
                        0.,
                        TAU * (left / 15.).clamp(0., 1.),
                        t.accent,
                    );
                    c.text(
                        &format!("{}", left.ceil() as i32),
                        r(cx - 13., y + 3., 26., 26.),
                        font(11., 700),
                        t.text,
                        Align::Center,
                    );
                    c.text(
                        &s.tr("Switching in {n} seconds")
                            .replace("{n}", &(left.ceil() as i32).to_string()),
                        r(PAD + 40., y, PW - 2. * PAD - 40., 32.),
                        font(13., 600),
                        t.accent,
                        Align::Start,
                    );
                }
                None => {
                    c.text(
                        s.tr("App may be working · confirmation required"),
                        r(PAD, y, PW - 2. * PAD, 32.),
                        font(13., 600),
                        t.warn,
                        Align::Start,
                    );
                }
            }
            confirm_buttons(c, s, m, s.tr("Switch now"), Action::Confirm, false);
        }
    }
}
fn confirm_buttons(c: &mut C, s: &State, m: &mut Mem, label: &str, a: Action, danger: bool) {
    let y = CB - 48.;
    button(
        c,
        s,
        m,
        r(PAD, y, 128., 46.),
        Action::Cancel,
        s.tr("Cancel"),
        Style::Secondary,
        None,
    );
    button(
        c,
        s,
        m,
        r(PAD + 140., y, PW - 2. * PAD - 140., 46.),
        a,
        label,
        if danger {
            Style::Danger
        } else {
            Style::Primary
        },
        None,
    );
}
#[allow(clippy::too_many_arguments)]
fn dialog(
    c: &mut C,
    s: &State,
    _m: &mut Mem,
    ic: Icon,
    tone: Color,
    title: &str,
    body: &str,
    account: Option<usize>,
) {
    let t = s.theme();
    let y = CT + 6.;
    c.circle(PAD + 26., y + 26., 26., tone.alpha(0.15));
    c.ring(PAD + 26., y + 26., 25.5, 1., tone.alpha(0.25));
    icon(c, ic, PAD + 26., y + 26., 24., tone, 0.);
    c.text(
        title,
        r(PAD, y + 66., PW - 2. * PAD, 30.),
        font(21., 700),
        t.text,
        Align::Start,
    );
    let bh = c.paragraph(
        body,
        r(PAD, y + 104., PW - 2. * PAD, 150.),
        font(13.5, 400),
        t.muted,
    );
    if let Some(a) = account.and_then(|i| s.settings.accounts.get(i).map(|a| (i, a))) {
        let (i, a) = a;
        let b = r(PAD, y + 104. + bh + 18., PW - 2. * PAD, 60.);
        c.fill(b, 16., t.surface);
        c.stroke(b, 16., 1., t.line.alpha(0.6));
        avatar(c, s, &a.name, b.x + 30., b.cy(), 18.);
        c.text(
            &a.name,
            r(b.x + 58., b.y + 11., b.w - 70., 20.),
            font(14.5, 650),
            t.text,
            Align::Start,
        );
        let line = match a.usage.as_ref() {
            Some(u) => format!(
                "{} {}  ·  {} {}",
                s.session_label(),
                u.session
                    .as_ref()
                    .map(|w| percent(w.used))
                    .unwrap_or("—".into()),
                s.weekly_label(),
                u.weekly
                    .as_ref()
                    .map(|w| percent(w.used))
                    .unwrap_or("—".into())
            ),
            None => s.tr("Usage not available yet").into(),
        };
        c.text(
            &line,
            r(b.x + 58., b.y + 32., b.w - 70., 17.),
            font(12., 450),
            t.muted,
            Align::Start,
        );
        if s.active == Some(i) {
            let f = font(11., 650);
            let label = s.tr("Active");
            let w = c.measure(label, f) + 16.;
            chip(
                c,
                b.right() - 12. - w,
                b.cy() - 10.,
                label,
                t.good.alpha(0.15),
                t.good,
            );
        }
    }
}

fn home(c: &mut C, s: &State, m: &mut Mem) {
    let t = s.theme();
    c.text(
        s.tr("Accounts"),
        r(PAD, CT, 200., 30.),
        font(20., 700),
        t.text,
        Align::Start,
    );
    button(
        c,
        s,
        m,
        r(PW - PAD - 96., CT, 96., 32.),
        Action::Add,
        s.tr("Add"),
        Style::Secondary,
        Some(Icon::Plus),
    );
    let top = CT + 44.;
    let region = r(0., top, PW, CB - top);
    let n = s.settings.accounts.len();
    if n == 0 {
        m.scroll_max[0] = 0.;
        let cy = top + 92.;
        c.ring(PW / 2., cy, 38., 7., t.text.alpha(0.06));
        c.arc_gradient(PW / 2., cy, 38., 7., -0.6, TAU * 0.62, t.accent, t.accent2);
        logo(c, PW / 2., cy, 24., t.text, 2.6);
        c.text(
            s.tr("Your island starts here."),
            r(PAD, cy + 58., PW - 2. * PAD, 28.),
            font(19., 700),
            t.text,
            Align::Center,
        );
        c.text(
            s.tr("Add an account, then save its sign-in."),
            r(PAD, cy + 88., PW - 2. * PAD, 20.),
            font(13., 400),
            t.muted,
            Align::Center,
        );
        button(
            c,
            s,
            m,
            r(PW / 2. - 95., cy + 124., 190., 44.),
            Action::Add,
            s.tr("Add account"),
            Style::Primary,
            Some(Icon::Plus),
        );
        return;
    }
    let note = if s.settings.provider == 1 {
        Some(s.tr("Codex switches sign-ins; open sessions keep their work."))
    } else {
        None
    };
    let content = n as f32 * (CARD_H + CARD_GAP) + if note.is_some() { 40. } else { 0. };
    m.finish_scroll(0, content - region.h);
    let scroll = m.step_scroll(0);
    // Keyboard focus keeps rows in view.
    m.rows[0] = (0..n)
        .map(|i| (Action::Switch(i), i as f32 * (CARD_H + CARD_GAP), CARD_H))
        .collect();
    reveal(m, 0, region.h);
    let clip = c.map(region);
    c.push_clip(clip);
    let old_clip = c.hit_clip;
    c.hit_clip = Some(old_clip.map(|o| o.intersect(&clip)).unwrap_or(clip));
    for i in 0..n {
        let y = top + i as f32 * (CARD_H + CARD_GAP) - scroll;
        if y + CARD_H < top - 2. || y > region.bottom() + 2. {
            continue;
        }
        account_card(c, s, m, i, r(PAD, y, PW - 2. * PAD, CARD_H));
    }
    if let Some(note) = note {
        let y = top + n as f32 * (CARD_H + CARD_GAP) - scroll;
        c.text(
            note,
            r(PAD + 4., y, PW - 2. * PAD - 8., 30.),
            font(12., 400),
            t.muted,
            Align::Start,
        );
    }
    edge_fades(c, s, region, scroll, m.scroll_max[0]);
    c.hit_clip = old_clip;
    c.pop_clip();
}
fn reveal(m: &mut Mem, i: usize, height: f32) {
    let Some(a) = m.reveal else { return };
    if let Some((_, y, h)) = m.rows[i].iter().find(|(x, _, _)| *x == a).copied() {
        let target = m.scroll_target[i];
        if y < target {
            m.scroll_target[i] = y;
        } else if y + h > target + height {
            m.scroll_target[i] = y + h - height;
        }
        m.reveal = None;
    }
}
fn edge_fades(c: &C, s: &State, region: R, scroll: f32, max: f32) {
    let t = s.theme();
    let f = 18.;
    let top_bg = panel_bg(t);
    let bottom_bg = top_bg;
    if scroll > 0.5 {
        c.fill_gradient(r(0., region.y, PW, f), 0., top_bg, top_bg.alpha(0.), true);
    }
    if scroll < max - 0.5 {
        c.fill_gradient(
            r(0., region.bottom() - f, PW, f),
            0.,
            bottom_bg.alpha(0.),
            bottom_bg,
            true,
        );
    }
    if max > 0.5 {
        let content = region.h + max;
        let bar_h = (region.h * region.h / content).max(28.);
        let y = region.y + (region.h - bar_h) * (scroll / max);
        c.fill(r(PW - 6., y + 2., 3., bar_h - 4.), 1.5, t.text.alpha(0.16));
    }
}
fn account_card(c: &mut C, s: &State, m: &mut Mem, i: usize, b: R) {
    let t = s.theme();
    let Some(a) = s.settings.accounts.get(i) else {
        return;
    };
    let active = s.active == Some(i);
    let h_card = m.hover(s, Action::Switch(i));
    let h_any = h_card
        .max(m.hover(s, Action::Save(i)) * 0.6)
        .max(m.hover(s, Action::Details(i)) * 0.6);
    let lift = 1.5 * h_any;
    let b = r(b.x, b.y - lift, b.w, b.h);
    if h_any > 0.01 {
        c.shadow(
            b,
            18.,
            8.,
            (if t.dark { 0.6 } else { 0.3 }) * h_any,
            t.shadow,
        );
    }
    c.fill(b, 18., surface_hover(t, h_any));
    if active {
        c.stroke(b, 18., 1.5, t.accent.alpha(0.55));
    } else {
        c.stroke(b, 18., 1., t.line.alpha(0.45));
    }
    focus_ring(c, s, b, 18., Action::Switch(i));
    c.hit(b, Action::Switch(i));
    avatar(c, s, &a.name, b.x + 36., b.y + 36., 20.);
    if active {
        c.circle(b.x + 51., b.y + 51., 7., surface_hover(t, h_any));
        c.circle(b.x + 51., b.y + 51., 4.5, t.good);
    }
    let right_w = 112.;
    let text_x = b.x + 68.;
    let text_w = b.w - 68. - right_w;
    c.text(
        &a.name,
        r(text_x, b.y + 15., text_w, 21.),
        font(15., 650),
        t.text,
        Align::Start,
    );
    let sub = if a.draft {
        s.tr("Draft · enter a name, then save").to_string()
    } else if s.preview {
        if active {
            s.tr("Active · sample data").into()
        } else {
            s.tr("Saved · sample data").into()
        }
    } else if !s.saved.get(i).copied().unwrap_or(false) {
        s.tr("Not saved · Save to link").into()
    } else if let Some(u) = &a.usage {
        let stale = !s.usage_errors.get(i).is_none_or(|e| e.is_empty());
        format!(
            "{} · {}{}",
            s.tr(if active { "Active" } else { "Saved" }),
            age(s, accounts::now().saturating_sub(u.updated)),
            if stale { s.tr(" · stale") } else { "" }
        )
    } else if !s.usage_errors.get(i).is_none_or(|e| e.is_empty()) {
        s.tr("Usage unavailable · see details").into()
    } else if s.network_allowed {
        s.tr("Saved · reading usage…").into()
    } else {
        s.tr("Saved · usage not yet read").into()
    };
    c.text(
        &sub,
        r(text_x, b.y + 37., text_w, 17.),
        font(12., 450),
        if active {
            t.good.mix(t.muted, 0.25)
        } else {
            t.muted
        },
        Align::Start,
    );
    let unsaved = !s.saved.get(i).copied().unwrap_or(false);
    button(
        c,
        s,
        m,
        r(b.right() - 14. - 34. - 6. - 64., b.y + 16., 64., 32.),
        Action::Save(i),
        s.tr("Save"),
        if unsaved {
            Style::Primary
        } else {
            Style::Ghost
        },
        None,
    );
    icon_button(
        c,
        s,
        m,
        r(b.right() - 14. - 34., b.y + 15., 34., 34.),
        Action::Details(i),
        Icon::More,
        0.,
        0.,
    );
    let col_w = (b.w - 28. - 18.) / 2.;
    let u = a.usage.as_ref();
    meter(
        c,
        s,
        r(b.x + 14., b.y + 66., col_w, 44.),
        s.session_label(),
        u.and_then(|u| u.session.as_ref()),
        false,
    );
    meter(
        c,
        s,
        r(b.x + 14. + col_w + 18., b.y + 66., col_w, 44.),
        s.weekly_label(),
        u.and_then(|u| u.weekly.as_ref()),
        false,
    );
}

fn view_title(c: &mut C, s: &State, m: &mut Mem, title: &str) {
    let t = s.theme();
    icon_button(
        c,
        s,
        m,
        r(PAD - 6., CT - 1., 34., 34.),
        Action::Back,
        Icon::Back,
        0.,
        0.,
    );
    c.text(
        title,
        r(PAD + 36., CT, PW - 2. * PAD - 40., 32.),
        font(19., 700),
        t.text,
        Align::Start,
    );
}
fn details(c: &mut C, s: &State, m: &mut Mem, i: usize, current: bool) {
    let t = s.theme();
    let Some(a) = s.settings.accounts.get(i) else {
        return;
    };
    view_title(
        c,
        s,
        m,
        s.tr(if a.draft {
            "New account"
        } else {
            "Account details"
        }),
    );
    let mut y = CT + 46.;
    let label = if current {
        String::from_utf16_lossy(&s.edit)
    } else {
        a.name.clone()
    };
    c.text(
        s.tr("Name"),
        r(PAD, y, 200., 18.),
        font(12., 650),
        t.muted,
        Align::Start,
    );
    c.text(
        &format!("{}/32", label.chars().count()),
        r(PW - PAD - 60., y, 60., 18.),
        font(11.5, 500),
        if label.chars().count() > 32 {
            t.danger
        } else {
            t.muted
        },
        Align::End,
    );
    y += 22.;
    let field = r(PAD, y, PW - 2. * PAD, 48.);
    let focused = s.edit_focus && current;
    let fh = m.hover(s, Action::NameField);
    c.fill(field, 13., t.bg.mix(t.raised, 0.6 + 0.2 * fh));
    let glow = m.ease(
        7,
        Action::NameField,
        if focused { 1. } else { 0. },
        16.,
        None,
    );
    c.stroke(field, 13., 1. + 0.6 * glow, t.line.mix(t.accent, glow));
    let f = font(16., 500);
    let inner = r(field.x + 14., field.y, field.w - 28., field.h);
    let tw = c.measure(&label, f).min(inner.w);
    if focused && s.selected && !label.is_empty() {
        c.fill(
            r(inner.x - 2., inner.y + 12., tw + 4., inner.h - 24.),
            5.,
            t.accent.alpha(0.28),
        );
    }
    c.text(&label, inner, f, t.text, Align::Start);
    if focused && !s.selected && (s.clock.elapsed().as_millis() / 530) % 2 == 0 {
        let utf16: String = String::from_utf16_lossy(&s.edit);
        let cx = c.g.caret_x(&utf16, f, s.caret, s.rtl()).min(inner.w);
        c.fill(
            r(inner.x + cx, inner.y + 13., 1.6, inner.h - 26.),
            1.,
            t.accent,
        );
    }
    c.hit(field, Action::NameField);
    y += 64.;
    let plan = a
        .usage
        .as_ref()
        .map(|u| u.plan.clone())
        .filter(|p| !p.is_empty())
        .unwrap_or_else(|| s.tr("Unavailable").into());
    let w1 = chip(
        c,
        PAD,
        y,
        &format!("{} · {}", s.tr("Plan"), plan.to_uppercase()),
        t.accent.alpha(0.14),
        t.accent,
    );
    chip(c, PAD + w1 + 8., y, s.provider_name(), t.raised, t.muted);
    if s.active == Some(i) {
        let label = s.tr("Active");
        let w = c.measure(label, font(11., 650)) + 16.;
        chip(c, PW - PAD - w, y, label, t.good.alpha(0.15), t.good);
    }
    y += 34.;
    let cw = (PW - 2. * PAD - 12.) / 2.;
    let u = a.usage.as_ref();
    for (k, (label, w)) in [
        (s.session_label(), u.and_then(|u| u.session.as_ref())),
        (s.weekly_label(), u.and_then(|u| u.weekly.as_ref())),
    ]
    .into_iter()
    .enumerate()
    {
        let card = r(PAD + k as f32 * (cw + 12.), y, cw, 134.);
        c.fill(card, 18., t.surface);
        c.stroke(card, 18., 1., t.line.alpha(0.45));
        gauge(
            c,
            s,
            card.cx(),
            card.y + 46.,
            30.,
            7.,
            w.map(|w| w.used),
            15.,
        );
        c.text(
            label,
            r(card.x + 8., card.y + 86., card.w - 16., 18.),
            font(12.5, 650),
            t.text,
            Align::Center,
        );
        c.text(
            &reset_label(s, w),
            r(card.x + 8., card.y + 105., card.w - 16., 16.),
            font(11., 400),
            t.muted,
            Align::Center,
        );
    }
    y += 146.;
    let info = if s.preview {
        s.tr("Preview · sample values")
    } else if s.settings.provider == 1 {
        s.tr("Updates through Codex app-server using file credentials.")
    } else {
        s.tr("Updates automatically from your Claude session.")
    };
    if let Some(error) = s.usage_errors.get(i).filter(|e| !e.is_empty()) {
        c.paragraph(
            error,
            r(PAD, y, PW - 2. * PAD, 40.),
            font(12., 450),
            t.danger,
        );
    } else {
        c.paragraph(info, r(PAD, y, PW - 2. * PAD, 40.), font(12., 400), t.muted);
    }
    let by = CB - 48.;
    button(
        c,
        s,
        m,
        r(PAD, by, 132., 46.),
        Action::Remove,
        s.tr(if a.draft { "Discard" } else { "Remove" }),
        Style::Danger,
        Some(Icon::Trash),
    );
    button(
        c,
        s,
        m,
        r(PAD + 144., by, PW - 2. * PAD - 144., 46.),
        Action::NameSave,
        s.tr(if a.draft { "Save account" } else { "Save name" }),
        Style::Primary,
        Some(Icon::Check),
    );
}

fn section(c: &C, s: &State, y: f32, label: &str) -> f32 {
    let t = s.theme();
    c.text(
        label,
        r(PAD + 2., y, PW - 2. * PAD, 18.),
        font(11.5, 750),
        t.accent.mix(t.muted, 0.35),
        Align::Start,
    );
    y + 26.
}
fn preferences(c: &mut C, s: &State, m: &mut Mem) {
    let t = s.theme();
    view_title(c, s, m, s.tr("Settings"));
    let top = CT + 44.;
    let region = r(0., top, PW, CB - top);
    let scroll = m.step_scroll(1);
    let clip = c.map(region);
    c.push_clip(clip);
    let old_clip = c.hit_clip;
    c.hit_clip = Some(old_clip.map(|o| o.intersect(&clip)).unwrap_or(clip));
    let start = top - scroll;
    let mut y = start + 4.;
    let mut rows: Vec<(Action, f32, f32)> = Vec::new();
    let row = |a: Action, y: f32, h: f32, rows: &mut Vec<(Action, f32, f32)>| {
        rows.push((a, y - start, h))
    };

    // Appearance
    y = section(c, s, y, s.tr("APPEARANCE"));
    let cols = 3.;
    let gap = 10.;
    let cw = (PW - 2. * PAD - gap * (cols - 1.)) / cols;
    for i in 0..theme::COUNT {
        let col = (i % 3) as f32;
        let rowi = (i / 3) as f32;
        let cell = r(PAD + col * (cw + gap), y + rowi * (74. + gap), cw, 74.);
        let a = Action::Theme(i);
        row(a, cell.y, cell.h, &mut rows);
        let h = m.hover(s, a);
        let selected = s.settings.theme == i;
        let sel = m.ease(8, a, if selected { 1. } else { 0. }, 14., None);
        c.fill(cell, 14., surface_hover(t, h));
        c.stroke(cell, 14., 1. + sel, t.line.alpha(0.5).mix(t.accent, sel));
        let preview = r(cell.x + 8., cell.y + 8., cell.w - 16., 36.);
        let draw_preview = |c: &C, th: &Theme, b: R| {
            c.fill(b, 8., th.bg);
            c.fill(r(b.x + 5., b.y + 5., b.w - 10., b.h - 10.), 5., th.surface);
            c.circle_gradient(b.x + 17., b.cy(), 8., th.accent, th.accent2);
            c.fill(
                r(b.x + 30., b.cy() - 5., (b.w - 44.).max(4.), 4.),
                2.,
                th.text.alpha(0.8),
            );
            c.fill(
                r(b.x + 30., b.cy() + 2., ((b.w - 44.) * 0.6).max(4.), 3.),
                1.5,
                th.muted.alpha(0.8),
            );
        };
        if i == theme::AUTO {
            let half = preview.w / 2.;
            c.push_clip(c.map(r(preview.x, preview.y, half, preview.h)));
            draw_preview(c, &theme::THEMES[theme::DEFAULT], preview);
            c.pop_clip();
            c.push_clip(c.map(r(preview.x + half, preview.y, half, preview.h)));
            draw_preview(c, &theme::THEMES[8], preview);
            c.pop_clip();
        } else {
            draw_preview(c, &theme::THEMES[i], preview);
        }
        let name = if i == theme::AUTO {
            s.tr("Match Windows")
        } else if s.rtl() {
            theme::THEMES[i].arabic
        } else {
            theme::THEMES[i].name
        };
        c.text(
            name,
            r(cell.x + 6., cell.y + 48., cell.w - 12., 20.),
            font(11.5, 600),
            t.text.mix(t.muted, 0.3 * (1. - sel)),
            Align::Center,
        );
        focus_ring(c, s, cell, 14., a);
        c.hit(cell, a);
    }
    y += (theme::COUNT as f32 / cols).ceil() * (74. + gap) + 12.;

    // Floating island
    y = section(c, s, y, s.tr("FLOATING ISLAND"));
    let seg_x = PAD + 132.;
    let seg_w = PW - PAD - seg_x;
    let label_row = |c: &C, y: f32, label: &str| {
        c.text(
            label,
            r(PAD, y, 128., 36.),
            font(13., 550),
            t.text,
            Align::Start,
        );
    };
    label_row(c, y, s.tr("Style"));
    segmented(
        c,
        s,
        m,
        r(seg_x, y, seg_w, 36.),
        &[
            (s.tr("Circle"), Action::Style(true)),
            (s.tr("Pill"), Action::Style(false)),
        ],
        if s.settings.circle { 0 } else { 1 },
        Action::Style(true),
    );
    row(Action::Style(true), y, 36., &mut rows);
    y += 46.;
    label_row(c, y, s.tr("Center shows"));
    segmented(
        c,
        s,
        m,
        r(seg_x, y, seg_w, 36.),
        &[
            (s.tr("Percentage"), Action::Center(true)),
            (s.tr("Logo"), Action::Center(false)),
        ],
        if s.settings.circle_numbers { 0 } else { 1 },
        Action::Center(true),
    );
    row(Action::Center(true), y, 36., &mut rows);
    y += 46.;
    label_row(c, y, s.tr("Percentage of"));
    segmented(
        c,
        s,
        m,
        r(seg_x, y, seg_w, 36.),
        &[
            (s.session_label(), Action::Metric(0)),
            (s.weekly_label(), Action::Metric(1)),
            (s.tr("Highest"), Action::Metric(2)),
        ],
        s.settings.circle_metric,
        Action::Metric(0),
    );
    row(Action::Metric(0), y, 36., &mut rows);
    y += 50.;
    for (a, label) in [
        (Action::FloatSlider, s.tr("Island size")),
        (Action::BubbleSlider, s.tr("Hover card size")),
        (Action::MenuSlider, s.tr("Menu size")),
    ] {
        row(a, y, 36., &mut rows);
        y += slider_row(c, s, m, y, a, label) + 6.;
    }
    y += 12.;

    // Language
    y = section(c, s, y, s.tr("LANGUAGE"));
    segmented(
        c,
        s,
        m,
        r(PAD, y, PW - 2. * PAD, 38.),
        &[
            ("English", Action::Language(0)),
            ("العربية", Action::Language(1)),
        ],
        s.settings.language,
        Action::Language(0),
    );
    row(Action::Language(0), y, 38., &mut rows);
    y += 56.;

    // Behaviour
    y = section(c, s, y, s.tr("BEHAVIOR"));
    let toggles: Vec<(Action, &str, Option<&str>, bool)> = vec![
        (
            Action::Motion,
            s.tr("Reduce motion"),
            None,
            s.settings.reduced_motion,
        ),
        (
            Action::LaunchAtLogin,
            s.tr("Start with Windows"),
            None,
            s.launch_at_login,
        ),
        (
            Action::ShowIsland,
            s.tr("Show floating island"),
            Some(s.tr("When off, Muisland stays in the tray")),
            s.settings.show_island,
        ),
        (
            Action::TrayIcon,
            s.tr("Tray icon"),
            Some(s.tr("Live usage ring in the notification area")),
            s.settings.tray,
        ),
        (
            Action::Hotkey,
            s.tr("Keyboard shortcut"),
            Some(s.tr("Ctrl + Alt + M opens Muisland")),
            s.settings.hotkey,
        ),
    ];
    for (a, label, cap, on) in toggles {
        let h = toggle_row(c, s, m, y, a, label, cap, on);
        row(a, y, h, &mut rows);
        y += h + 8.;
    }
    y += 12.;

    // Alerts
    y = section(c, s, y, s.tr("ALERTS & AUTO-SWITCH"));
    let h = toggle_row(
        c,
        s,
        m,
        y,
        Action::UsageAlerts,
        s.tr("Notify at usage thresholds"),
        None,
        s.settings.usage_alerts,
    );
    row(Action::UsageAlerts, y, h, &mut rows);
    y += h + 10.;
    for (a, label) in [
        (Action::SessionThreshold, s.tr("Session limit")),
        (Action::WeeklyThreshold, s.tr("Weekly limit")),
    ] {
        row(a, y, 36., &mut rows);
        y += slider_row(c, s, m, y, a, label) + 6.;
    }
    y += 6.;
    for (a, label, on) in [
        (
            Action::AutoSwitch,
            s.tr("Auto-switch at these limits"),
            s.settings.auto_switch,
        ),
        (
            Action::PauseForWork,
            s.tr("Ask first when work may be open"),
            s.settings.pause_for_work,
        ),
    ] {
        let h = toggle_row(c, s, m, y, a, label, None, on);
        row(a, y, h, &mut rows);
        y += h + 8.;
    }
    y += 2. + c.paragraph(
        s.tr("Auto-switch waits 15 seconds. Cancel or close the Island to stop it. Open apps may have running tasks; idle detection is not guaranteed."),
        r(PAD + 2., y, PW - 2. * PAD - 4., 80.),
        font(11.5, 400),
        t.muted,
    );
    y += 20.;

    // Safety
    y = section(c, s, y, s.tr("SAFETY & RECOVERY"));
    if s.settings.provider == 0 {
        let h = toggle_row(
            c,
            s,
            m,
            y,
            Action::Protect,
            s.tr("Back up local data before switch"),
            Some(s.tr("Encrypted checkpoint while Claude is closed")),
            s.settings.protect_local,
        );
        row(Action::Protect, y, h, &mut rows);
        y += h + 10.;
    }
    button(
        c,
        s,
        m,
        r(PAD, y, PW - 2. * PAD, 42.),
        Action::Rollback,
        s.tr("Restore previous sign-in"),
        Style::Secondary,
        None,
    );
    row(Action::Rollback, y, 42., &mut rows);
    y += 50.;
    if s.settings.provider == 0 {
        button(
            c,
            s,
            m,
            r(PAD, y, PW - 2. * PAD, 42.),
            Action::Recover,
            s.tr("Recover local backup to folder"),
            Style::Secondary,
            None,
        );
        row(Action::Recover, y, 42., &mut rows);
        y += 50.;
    }
    y += 8.;
    button(
        c,
        s,
        m,
        r(PAD, y, PW - 2. * PAD, 42.),
        Action::Quit,
        s.tr("Quit Muisland"),
        Style::Danger,
        None,
    );
    row(Action::Quit, y, 42., &mut rows);
    y += 52.;
    c.text(
        &format!(
            "Muisland {} · {}",
            env!("CARGO_PKG_VERSION"),
            s.tr("Unofficial · not affiliated with Anthropic or OpenAI")
        ),
        r(PAD, y, PW - 2. * PAD, 18.),
        font(11., 400),
        t.muted.alpha(0.8),
        Align::Center,
    );
    y += 30.;
    m.finish_scroll(1, y - start - region.h);
    m.rows[1] = rows;
    reveal(m, 1, region.h);
    edge_fades(c, s, region, scroll, m.scroll_max[1]);
    c.hit_clip = old_clip;
    c.pop_clip();
}

// ------------------------------------------------------------- hover card

pub fn draw_bubble(
    s: &State,
    m: &mut Mem,
    g: &Gfx,
    surface: &Surface,
    scale: f32,
    right: bool,
    t_open: f32,
) {
    let Some(mut c) = Canvas::begin(g, surface, scale, 0., 0.) else {
        return;
    };
    let t = s.theme();
    let e = ease_out(t_open.clamp(0., 1.));
    let zoom = 0.9 + 0.1 * t_open.max(0.);
    let pivot_x = if right { BMARGIN } else { BMARGIN + BW };
    c.set_zoom(zoom, pivot_x, BMARGIN + BH / 2.);
    c.alpha = e;
    c.ox = BMARGIN + if right { -10. } else { 10. } * (1. - e);
    c.oy = BMARGIN;
    let card = r(0., 0., BW, BH);
    c.shadow(card, 20., 14., if t.dark { 1. } else { 0.5 }, t.shadow);
    c.fill(card, 20., panel_bg(t));
    c.stroke(card, 20., 1., t.text.alpha(if t.dark { 0.08 } else { 0.1 }));
    c.rtl = s.rtl();
    c.mirror = BW;
    let account = s.active.and_then(|i| s.settings.accounts.get(i));
    let u = s.active_usage();
    match account {
        Some(a) => avatar(&c, s, &a.name, 36., 36., 20.),
        None => {
            c.circle(36., 36., 20., t.raised);
            logo(&c, 36., 36., 16., t.muted, 2.);
        }
    }
    let name = account
        .map(|a| a.name.as_str())
        .unwrap_or(s.tr("No linked account"));
    c.text(
        name,
        r(66., 16., BW - 66. - 86., 21.),
        font(15.5, 700),
        t.text,
        Align::Start,
    );
    let caption = match u {
        Some(u) if !u.plan.is_empty() => format!("{} · {}", s.provider_name(), u.plan),
        Some(_) => s.provider_name().to_string(),
        None => s.tr("Usage not available yet").into(),
    };
    c.text(
        &caption,
        r(66., 38., BW - 66. - 86., 16.),
        font(12., 450),
        t.muted,
        Align::Start,
    );
    if let Some(v) = s.metric() {
        let (a, _) = t.usage(v);
        c.text(
            &percent(v),
            r(BW - 16. - 80., 12., 80., 28.),
            font(23., 750),
            a.mix(t.text, 0.25),
            Align::End,
        );
        c.text(
            s.metric_label(),
            r(BW - 16. - 80., 40., 80., 14.),
            font(10., 700),
            t.muted,
            Align::End,
        );
    }
    c.fill(r(16., 70., BW - 32., 1.), 0., t.line.alpha(0.7));
    meter(
        &c,
        s,
        r(16., 80., BW - 32., 50.),
        s.session_label(),
        u.and_then(|u| u.session.as_ref()),
        true,
    );
    meter(
        &c,
        s,
        r(16., 134., BW - 32., 50.),
        s.weekly_label(),
        u.and_then(|u| u.weekly.as_ref()),
        true,
    );
    c.fill(r(16., BH - 31., BW - 32., 1.), 0., t.line.alpha(0.5));
    let _ = m;
    let hint = if s.toast_until > s.now() {
        s.tr("Switched · usage refreshes shortly")
    } else {
        s.tr("Click to open")
    };
    c.text(
        hint,
        r(16., BH - 26., BW - 32., 18.),
        font(10.5, 500),
        t.muted.alpha(0.85),
        Align::Start,
    );
    if s.settings.hotkey {
        c.text(
            "Ctrl+Alt+M",
            r(16., BH - 26., BW - 32., 18.),
            font(10.5, 600),
            t.muted.alpha(0.7),
            Align::End,
        );
    }
    let _ = c.end();
}

// --------------------------------------------------------------- tray icon

/// A small usage ring for the notification area, `size` pixels square.
pub fn tray_icon(s: &State, g: &Gfx, size: i32) -> Option<Surface> {
    let surface = Surface::new(size, size)?;
    let c: C = Canvas::begin(g, &surface, size as f32 / 16., 0., 0.)?;
    let t = s.theme();
    c.circle(8., 8., 7.9, t.bg);
    c.ring(8., 8., 5.4, 2.6, t.text.alpha(0.16));
    match s.metric() {
        Some(v) => {
            let (a, b) = t.usage(v);
            c.arc_gradient(
                8.,
                8.,
                5.4,
                2.6,
                0.,
                TAU * (v as f32 / 100.).clamp(0.02, 1.),
                a,
                b,
            );
        }
        None => logo(&c, 8., 8., 7., t.text, 1.3),
    }
    let _ = c.end();
    Some(surface)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reset_labels_are_localised_and_elapsed_resets_are_flagged() {
        let mut s = app::initial(true, 0., 0);
        let now = accounts::now();
        let w = |resets| Window { used: 50., resets };
        assert_eq!(
            reset_label(&s, Some(&w(Some(now - 5)))),
            "Reset due · refreshing"
        );
        assert_eq!(reset_label(&s, Some(&w(None))), "Reset time unavailable");
        assert_eq!(
            reset_label(&s, Some(&w(Some(now + 3 * 3600 + 1)))),
            "Resets in 3h 1m"
        );
        assert_eq!(
            reset_label(&s, Some(&w(Some(now + 2 * 86400 + 60)))),
            "Resets in 2d 0h"
        );
        s.settings.language = 1;
        assert!(reset_label(&s, Some(&w(Some(now + 600)))).starts_with("يتجدد"));
    }
    #[test]
    fn initials_handle_names_in_any_script() {
        assert_eq!(initials("Work"), "W");
        assert_eq!(initials("my work"), "MW");
        assert_eq!(initials("حساب الشغل"), "حا");
        assert_eq!(initials("   "), "?");
    }
}
