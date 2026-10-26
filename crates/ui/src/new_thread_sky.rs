//! The new-thread sky: a banded, ringed planet and a cratered moon drawn out
//! of monospace glyphs on a faint cell grid, under a field of twinkling stars.
//! It sits behind the greeting and composer when no wallpaper is installed.
//!
//! The scene is a pure function of time, sampled once per cell. Its view is
//! cached and repaints on the ambient loader clock (~10Hz) only while mounted,
//! so typing replays the last frame and a hidden sky schedules nothing.
//! Reduced motion rests the scene on one frame. Glyphs are shaped once and
//! painted directly at fixed cell origins, so frames never reshape text.

use std::f64::consts::TAU;

use gpui::{
    App, AppContext as _, Bounds, Context, Entity, FontId, GlyphId, Hsla, IntoElement,
    ParentElement as _, Pixels, Render, RenderOnce, SharedString, Styled as _, TextRun, Window,
    canvas, div, point, px, size,
};

use crate::motion;
use crate::theme::{Appearance, Theme};

/// Glyphs from sparse to dense; a sample's brightness picks its glyph.
const RAMP: [char; 14] = [
    '.', '\'', ':', '-', '+', 't', 'o', 'x', '%', 'S', 'X', '8', '#', '@',
];
/// Ramp entries a star twinkles through.
const STAR_LEVELS: [u8; 4] = [0, 1, 2, 4];
const CELL: f32 = 13.0;
const GLYPH_SIZE: f32 = 10.0;
/// The resting frame shown when motion is reduced.
const REST_SECONDS: f64 = 24.0;
const STAR_DENSITY: f32 = 0.012;
const ACCENT_STAR_DENSITY: f32 = 0.0015;
const RING_TILT: f32 = 0.3;
const RING_SQUASH: f32 = 0.26;
const RING_INNER: f32 = 1.3;
const RING_OUTER: f32 = 2.15;
const RING_GAP: (f32, f32) = (1.68, 1.76);
const HALO: f32 = 0.09;
/// Brightness kept at the composer's edge and the cells it takes to recover.
const QUIET_FLOOR: f32 = 0.1;
const QUIET_FEATHER: f32 = 9.0;
/// Cells of quiet kept around the composer: its pickers and greeting sit above.
const QUIET_MARGIN: (f32, f32, f32) = (2.0, 6.0, 2.0);
/// Crater directions on the moon (normalized at use) and the cosine of each
/// crater's angular radius.
const CRATERS: [(f32, f32, f32, f32); 6] = [
    (0.3, -0.4, 0.85, 0.94),
    (-0.5, 0.2, 0.8, 0.96),
    (0.1, 0.55, 0.8, 0.95),
    (0.7, 0.1, 0.7, 0.97),
    (-0.2, -0.75, 0.6, 0.97),
    (-0.85, -0.2, 0.5, 0.95),
];

/// One painted cell: a ramp index, a 0..1 strength before theme ink, and
/// whether it is one of the few highlighted glyphs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Mark {
    pub level: u8,
    pub alpha: f32,
    pub accent: bool,
}

/// A rectangle in cell units where the art stays dim.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Quiet {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

#[derive(Debug, Clone, Copy)]
struct Sphere {
    x: f32,
    y: f32,
    radius: f32,
}

/// The scene at one instant, laid out in cell units for a `cols × rows` grid.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Sky {
    cols: usize,
    rows: usize,
    seconds: f64,
    quiet: Option<Quiet>,
    planet: Sphere,
    moon: Sphere,
    light: [f32; 3],
    spin: f32,
    moon_spin: f32,
    breath: f32,
    glint: f32,
}

fn wave(seconds: f64, period: f64) -> f32 {
    (seconds / period * TAU).sin() as f32
}

fn turn(seconds: f64, period: f64) -> f32 {
    ((seconds / period).fract() * TAU) as f32
}

fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn wrap_angle(angle: f32) -> f32 {
    let tau = std::f32::consts::TAU;
    (angle + std::f32::consts::PI).rem_euclid(tau) - std::f32::consts::PI
}

fn hash(col: usize, row: usize, salt: u32) -> f32 {
    let mut h = (col as u32).wrapping_mul(0x9E37_79B1)
        ^ (row as u32).wrapping_mul(0x85EB_CA77)
        ^ salt.wrapping_mul(0xC2B2_AE3D);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2C1B_3C6D);
    h ^= h >> 12;
    h = h.wrapping_mul(0x297A_2D39);
    h ^= h >> 15;
    (h >> 8) as f32 / (1u32 << 24) as f32
}

/// Dims the middle of the canvas until the composer has been measured.
pub(crate) fn center_focus(x: f32, y: f32) -> f32 {
    let nx = (x - 0.5) / 0.34;
    let ny = (y - 0.5) / 0.3;
    0.12 + 0.88 * smoothstep(0.7, 1.6, (nx * nx + ny * ny).sqrt())
}

/// How much of the sky shows beside the wallpaper: none while a wallpaper is
/// installed or loading, crossfading against the artwork as it comes and goes.
pub(crate) fn presence(wallpaper: bool, previous: bool, current: bool, mix: f32) -> f32 {
    match (previous, current) {
        (false, false) if !wallpaper => 1.0,
        (false, true) => 1.0 - mix,
        (true, false) if !wallpaper => mix,
        _ => 0.0,
    }
}

fn level(intensity: f32) -> u8 {
    (intensity.clamp(0.0, 1.0) * (RAMP.len() - 1) as f32).round() as u8
}

impl Sky {
    pub fn new(cols: usize, rows: usize, seconds: f64, quiet: Option<Quiet>) -> Self {
        let (w, h) = (cols as f32, rows as f32);
        let radius = (h * 0.24).min(w * 0.16).max(5.0);
        let moon_radius = (radius * 0.45).max(5.0);
        let light = {
            let (x, y, z) = (-0.55 + 0.18 * wave(seconds, 37.0), -0.35, 0.75);
            let len = (x * x + y * y + z * z).sqrt();
            [x / len, y / len, z / len]
        };
        Self {
            cols,
            rows,
            seconds,
            quiet,
            planet: Sphere {
                x: w - radius * 1.5 + 1.2 * wave(seconds, 71.0),
                y: (h * 0.26).max(radius * 1.25) + 0.8 * wave(seconds, 53.0),
                radius,
            },
            moon: Sphere {
                x: moon_radius * 2.2 + 1.5 * wave(seconds, 83.0),
                y: h - moon_radius * 2.0 + wave(seconds + 17.0, 67.0),
                radius: moon_radius,
            },
            light,
            spin: turn(seconds, 140.0),
            moon_spin: turn(seconds, 97.0),
            breath: 0.5 + 0.5 * wave(seconds, 9.0),
            glint: turn(seconds, 41.0),
        }
    }

    pub fn mark(&self, col: usize, row: usize) -> Option<Mark> {
        let (x, y) = (col as f32 + 0.5, row as f32 + 0.5);
        let focus = self.focus(x, y);
        let body = self
            .planet_sample(
                (x - self.planet.x) / self.planet.radius,
                (y - self.planet.y) / self.planet.radius,
            )
            .or_else(|| {
                self.moon_sample(
                    (x - self.moon.x) / self.moon.radius,
                    (y - self.moon.y) / self.moon.radius,
                )
                .map(|intensity| (intensity, false))
            });
        if let Some((intensity, accent)) = body {
            return Some(Mark {
                level: level(intensity),
                alpha: focus * (0.3 + 0.7 * intensity),
                accent,
            });
        }
        self.star(col, row, focus)
    }

    /// Dims the art around the composer so it frames the greeting and input
    /// instead of competing with them.
    pub fn focus(&self, x: f32, y: f32) -> f32 {
        let Some(quiet) = self.quiet else {
            return center_focus(x / self.cols as f32, y / self.rows as f32);
        };
        let dx = (quiet.left - x).max(x - quiet.right).max(0.0);
        let dy = (quiet.top - y).max(y - quiet.bottom).max(0.0);
        let distance = (dx * dx + dy * dy).sqrt();
        QUIET_FLOOR + (1.0 - QUIET_FLOOR) * smoothstep(0.0, QUIET_FEATHER, distance)
    }

    fn planet_sample(&self, dx: f32, dy: f32) -> Option<(f32, bool)> {
        let (sin, cos) = RING_TILT.sin_cos();
        let (u, v) = (dx * cos + dy * sin, -dx * sin + dy * cos);
        let ring = self.ring_sample(u, v);
        let d2 = dx * dx + dy * dy;
        if d2 < 1.0 {
            if v > 0.0
                && let Some(ring) = ring
            {
                return Some(ring);
            }
            let nz = (1.0 - d2).sqrt();
            let [lx, ly, lz] = self.light;
            let lambert = (dx * lx + dy * ly + nz * lz).max(0.0);
            let lon = u.atan2(nz) + self.spin;
            let band = 0.5 + 0.5 * (v * 10.0 + 0.8 * (lon * 2.0 + v * 3.0).sin()).sin();
            let spot_lon = wrap_angle(lon - 0.6);
            let spot = (-(spot_lon * spot_lon * 5.0 + (v - 0.32).powi(2) * 80.0)).exp();
            let rim = (1.0 - nz).powi(3) * 0.3 * self.breath;
            let limb = 0.3 * smoothstep(0.84, 0.97, d2.sqrt());
            let intensity = (lambert * (0.5 + 0.4 * band + 0.35 * spot) + rim).max(limb);
            return Some((intensity.clamp(0.05, 1.0), false));
        }
        if ring.is_some() {
            return ring;
        }
        let d = d2.sqrt();
        (d < 1.0 + HALO).then(|| (0.04 + 0.1 * self.breath * (1.0 - (d - 1.0) / HALO), false))
    }

    fn ring_sample(&self, u: f32, v: f32) -> Option<(f32, bool)> {
        let squashed = v / RING_SQUASH;
        let r = (u * u + squashed * squashed).sqrt();
        if !(RING_INNER..RING_OUTER).contains(&r) || (RING_GAP.0..RING_GAP.1).contains(&r) {
            return None;
        }
        let edge = smoothstep(RING_INNER, RING_INNER + 0.12, r)
            * (1.0 - smoothstep(RING_OUTER - 0.15, RING_OUTER, r));
        let glint = (squashed.atan2(u) - self.glint).cos() > 0.992 && r > RING_GAP.1;
        if glint {
            return Some((0.95, true));
        }
        let depth = if v > 0.0 { 1.0 } else { 0.7 };
        Some((
            (0.5 + 0.18 * (r * 29.0).sin()) * (0.4 + 0.6 * edge) * depth,
            false,
        ))
    }

    fn moon_sample(&self, dx: f32, dy: f32) -> Option<f32> {
        let d2 = dx * dx + dy * dy;
        if d2 >= 1.0 {
            return None;
        }
        let nz = (1.0 - d2).sqrt();
        let [lx, ly, lz] = self.light;
        let lambert = (dx * lx + dy * ly + nz * lz).max(0.0);
        let (sin, cos) = self.moon_spin.sin_cos();
        let (x, z) = (dx * cos + nz * sin, -dx * sin + nz * cos);
        let shade = CRATERS.iter().fold(1.0, |shade, &(cx, cy, cz, reach)| {
            let len = (cx * cx + cy * cy + cz * cz).sqrt();
            let dot = (x * cx + dy * cy + z * cz) / len;
            if dot > reach + 0.015 {
                shade * 0.55
            } else if dot > reach {
                shade * 1.15
            } else {
                shade
            }
        });
        let limb = 0.28 * smoothstep(0.8, 0.95, d2.sqrt());
        Some((lambert * shade * 0.95).max(limb).clamp(0.04, 1.0))
    }

    fn star(&self, col: usize, row: usize, focus: f32) -> Option<Mark> {
        let seed = hash(col, row, 1);
        if seed >= STAR_DENSITY {
            return None;
        }
        let rate = 0.5 + hash(col, row, 2) as f64 * 0.9;
        let phase = hash(col, row, 3) as f64 * TAU;
        let twinkle = 0.5 + 0.5 * (self.seconds * rate + phase).sin() as f32;
        let step = ((twinkle * STAR_LEVELS.len() as f32) as usize).min(STAR_LEVELS.len() - 1);
        Some(Mark {
            level: STAR_LEVELS[step],
            alpha: focus * (0.25 + 0.6 * twinkle),
            accent: seed < ACCENT_STAR_DENSITY && twinkle > 0.8,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Ink {
    glyph: Hsla,
    accent: Hsla,
    grid: Hsla,
}

impl Ink {
    fn of(theme: &Theme) -> Self {
        match theme.appearance {
            Appearance::Dark => Self {
                glyph: theme.text.opacity(0.2),
                accent: theme.glyph.light.opacity(0.6),
                grid: theme.text.opacity(0.035),
            },
            Appearance::Light => Self {
                glyph: theme.text.opacity(0.17),
                accent: theme.glyph.deep.opacity(0.55),
                grid: theme.text.opacity(0.028),
            },
        }
    }
}

/// The sky behind the new-thread composition, kept quiet around `composer`
/// (window coordinates). `presence` fades it as a whole; at zero it paints
/// nothing and stops scheduling frames.
pub(crate) fn new_thread_sky(
    theme: &Theme,
    presence: f32,
    composer: Option<Bounds<Pixels>>,
) -> impl IntoElement {
    NewThreadSky {
        style: SkyStyle {
            ink: Ink::of(theme),
            font: theme.font_mono.clone(),
            presence: presence.clamp(0.0, 1.0),
            composer,
        },
    }
}

#[derive(Clone, PartialEq)]
struct SkyStyle {
    ink: Ink,
    font: SharedString,
    presence: f32,
    composer: Option<Bounds<Pixels>>,
}

#[derive(IntoElement)]
struct NewThreadSky {
    style: SkyStyle,
}

impl RenderOnce for NewThreadSky {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let style = self.style;
        let view = window.with_global_id("new-thread-sky".into(), |id, window| {
            window.with_element_state(id, |previous: Option<Entity<SkyView>>, _| {
                let view = previous.unwrap_or_else(|| {
                    cx.new(|_| SkyView {
                        style: style.clone(),
                        glyphs: None,
                    })
                });
                view.update(cx, |view, cx| {
                    if view.style != style {
                        view.style = style;
                        cx.notify();
                    }
                });
                (view.clone(), view)
            })
        });
        view.cached(gpui::StyleRefinement::default().size_full())
    }
}

struct Glyphs {
    family: SharedString,
    cells: Vec<Option<(FontId, GlyphId, f32)>>,
    ascent: f32,
    descent: f32,
}

impl Glyphs {
    fn shape(family: SharedString, window: &mut Window) -> Self {
        let font = gpui::font(family.clone());
        let font_size = px(GLYPH_SIZE);
        let (mut ascent, mut descent) = (GLYPH_SIZE * 0.8, GLYPH_SIZE * 0.2);
        let cells = RAMP
            .iter()
            .map(|glyph| {
                let text = SharedString::from(glyph.to_string());
                let run = TextRun {
                    len: text.len(),
                    font: font.clone(),
                    color: gpui::black(),
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                };
                let line = window
                    .text_system()
                    .shape_line(text, font_size, &[run], None);
                ascent = f32::from(line.ascent);
                descent = f32::from(line.descent);
                let run = line.runs.first()?;
                let shaped = run.glyphs.first()?;
                Some((run.font_id, shaped.id, f32::from(line.width)))
            })
            .collect();
        Self {
            family,
            cells,
            ascent,
            descent,
        }
    }
}

struct SkyView {
    style: SkyStyle,
    glyphs: Option<std::rc::Rc<Glyphs>>,
}

impl Render for SkyView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let style = self.style.clone();
        if style.presence <= 0.0 {
            return div().size_full();
        }
        let seconds = motion::ambient_seconds(cx.entity_id(), cx).unwrap_or(REST_SECONDS);
        let glyphs = match &self.glyphs {
            Some(glyphs) if glyphs.family == style.font => glyphs.clone(),
            _ => {
                let glyphs = std::rc::Rc::new(Glyphs::shape(style.font.clone(), window));
                self.glyphs = Some(glyphs.clone());
                glyphs
            }
        };
        div().size_full().child(
            canvas(
                |_, _, _| {},
                move |bounds, _, window, _| paint(bounds, seconds, &glyphs, &style, window),
            )
            .size_full(),
        )
    }
}

/// A grid line from `start` to `end`, broken where it would run under the
/// composer so its glass stays clean.
fn grid_spans(start: f32, end: f32, hole: Option<(f32, f32)>) -> Vec<(f32, f32)> {
    match hole {
        Some((from, to)) => [(start, from.max(start)), (to.min(end), end)]
            .into_iter()
            .filter(|(a, b)| b > a)
            .collect(),
        None => vec![(start, end)],
    }
}

fn paint(
    bounds: Bounds<Pixels>,
    seconds: f64,
    glyphs: &Glyphs,
    style: &SkyStyle,
    window: &mut Window,
) {
    let (ink, presence) = (style.ink, style.presence);
    let (width, height) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
    let cols = (width / CELL).ceil() as usize;
    let rows = (height / CELL).ceil() as usize;
    if cols == 0 || rows == 0 {
        return;
    }
    let left = f32::from(bounds.origin.x) + (width - cols as f32 * CELL) / 2.0;
    let top = f32::from(bounds.origin.y) + (height - rows as f32 * CELL) / 2.0;
    let grid = ink.grid.opacity(presence);
    let composer = style.composer.map(|composer| {
        (
            (f32::from(composer.left()), f32::from(composer.right())),
            (f32::from(composer.top()), f32::from(composer.bottom())),
        )
    });
    let (y0, y1) = (f32::from(bounds.top()), f32::from(bounds.bottom()));
    for col in 0..=cols {
        let x = left + col as f32 * CELL;
        let hole = composer.and_then(|((l, r), ys)| (l..=r).contains(&x).then_some(ys));
        for (from, to) in grid_spans(y0, y1, hole) {
            window.paint_quad(gpui::fill(
                Bounds::new(point(px(x), px(from)), size(px(1.0), px(to - from))),
                grid,
            ));
        }
    }
    let (x0, x1) = (f32::from(bounds.left()), f32::from(bounds.right()));
    for row in 0..=rows {
        let y = top + row as f32 * CELL;
        let hole = composer.and_then(|(xs, (t, b))| (t..=b).contains(&y).then_some(xs));
        for (from, to) in grid_spans(x0, x1, hole) {
            window.paint_quad(gpui::fill(
                Bounds::new(point(px(from), px(y)), size(px(to - from), px(1.0))),
                grid,
            ));
        }
    }
    let (side, above, below) = QUIET_MARGIN;
    let quiet = style.composer.map(|composer| Quiet {
        left: (f32::from(composer.left()) - left) / CELL - side,
        top: (f32::from(composer.top()) - top) / CELL - above,
        right: (f32::from(composer.right()) - left) / CELL + side,
        bottom: (f32::from(composer.bottom()) - top) / CELL + below,
    });
    let sky = Sky::new(cols, rows, seconds, quiet);
    let baseline = (CELL - glyphs.ascent - glyphs.descent) / 2.0 + glyphs.ascent;
    for row in 0..rows {
        for col in 0..cols {
            let Some(mark) = sky.mark(col, row) else {
                continue;
            };
            let Some((font_id, glyph_id, advance)) = glyphs.cells[mark.level as usize] else {
                continue;
            };
            let color = if mark.accent { ink.accent } else { ink.glyph };
            let origin = point(
                px(left + col as f32 * CELL + (CELL - advance) / 2.0),
                px(top + row as f32 * CELL + baseline),
            );
            let _ = window.paint_glyph(
                origin,
                font_id,
                glyph_id,
                px(GLYPH_SIZE),
                color.opacity(mark.alpha * presence),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const COLS: usize = 96;
    const ROWS: usize = 64;

    #[test]
    fn same_instant_draws_the_same_frame() {
        let a = Sky::new(COLS, ROWS, 12.5, None);
        let b = Sky::new(COLS, ROWS, 12.5, None);
        for row in 0..ROWS {
            for col in 0..COLS {
                assert_eq!(a.mark(col, row), b.mark(col, row));
            }
        }
    }

    #[test]
    fn planet_center_is_dense_and_the_frame_moves() {
        let sky = Sky::new(COLS, ROWS, 0.0, None);
        let (col, row) = (sky.planet.x as usize, sky.planet.y as usize);
        let mark = sky.mark(col, row).expect("planet paints its center");
        assert!(mark.level >= 4, "lit planet glyph, got {}", mark.level);
        let later = Sky::new(COLS, ROWS, 30.0, None);
        let changed = (0..ROWS)
            .flat_map(|row| (0..COLS).map(move |col| (col, row)))
            .filter(|&(col, row)| sky.mark(col, row) != later.mark(col, row))
            .count();
        assert!(changed > 50, "only {changed} cells changed in 30s");
    }

    #[test]
    fn the_middle_stays_quiet() {
        assert!(center_focus(0.5, 0.5) < 0.2);
        assert!(center_focus(0.9, 0.1) > 0.8);
        let sky = Sky::new(COLS, ROWS, 0.0, None);
        let busiest_middle = (ROWS * 2 / 5..ROWS * 3 / 5)
            .flat_map(|row| (COLS * 2 / 5..COLS * 3 / 5).map(move |col| (col, row)))
            .filter_map(|(col, row)| sky.mark(col, row))
            .map(|mark| mark.alpha)
            .fold(0.0, f32::max);
        assert!(busiest_middle < 0.2, "middle reaches {busiest_middle}");
    }

    #[test]
    fn art_dims_around_the_composer_and_recovers_away_from_it() {
        let quiet = Quiet {
            left: 20.0,
            top: 30.0,
            right: 76.0,
            bottom: 44.0,
        };
        let sky = Sky::new(COLS, ROWS, 0.0, Some(quiet));
        assert_eq!(sky.focus(40.0, 36.0), QUIET_FLOOR);
        assert!(sky.focus(80.0, 36.0) > QUIET_FLOOR);
        assert!(sky.focus(80.0, 36.0) < 1.0);
        assert_eq!(sky.focus(90.0, 10.0), 1.0);
    }

    #[test]
    fn the_planet_hides_the_far_ring_and_the_near_ring_crosses_it() {
        let sky = Sky::new(COLS, ROWS, 0.0, None);
        let (sin, cos) = RING_TILT.sin_cos();
        let at = |v: f32| (-v * sin, v * cos);
        let (fx, fy) = at(-1.9 * RING_SQUASH);
        let (nx, ny) = at(1.9 * RING_SQUASH);
        let ring = sky.ring_sample(0.0, 1.9 * RING_SQUASH).unwrap().0;
        let far = sky.planet_sample(fx, fy).unwrap().0;
        let near = sky.planet_sample(nx, ny).unwrap().0;
        assert!((near - ring).abs() < 1e-4, "near {near} vs ring {ring}");
        assert!((far - ring).abs() > 1e-3, "far {far} vs ring {ring}");
    }

    #[test]
    fn stars_are_sparse_and_few_are_highlighted() {
        let sky = Sky::new(COLS, ROWS, 5.0, None);
        let stars = (0..ROWS)
            .flat_map(|row| (0..COLS).map(move |col| (col, row)))
            .filter_map(|(col, row)| sky.star(col, row, 1.0))
            .collect::<Vec<_>>();
        let cells = (COLS * ROWS) as f32;
        assert!((stars.len() as f32) < cells * 0.03);
        assert!(!stars.is_empty());
        assert!(stars.iter().filter(|star| star.accent).count() <= stars.len() / 4);
    }

    #[test]
    fn grid_lines_break_under_the_composer() {
        assert_eq!(grid_spans(0.0, 100.0, None), vec![(0.0, 100.0)]);
        assert_eq!(
            grid_spans(0.0, 100.0, Some((40.0, 60.0))),
            vec![(0.0, 40.0), (60.0, 100.0)]
        );
        assert_eq!(
            grid_spans(0.0, 100.0, Some((-5.0, 30.0))),
            vec![(30.0, 100.0)]
        );
    }

    #[test]
    fn presence_yields_to_the_wallpaper() {
        assert_eq!(presence(false, false, false, 1.0), 1.0);
        assert_eq!(presence(true, false, false, 0.0), 0.0);
        assert_eq!(presence(true, false, true, 0.25), 0.75);
        assert_eq!(presence(false, true, false, 0.25), 0.25);
        assert_eq!(presence(true, true, true, 0.5), 0.0);
    }
}
