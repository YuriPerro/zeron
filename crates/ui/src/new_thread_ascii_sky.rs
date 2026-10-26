//! The new-chat sky: a ringed planet and a small moon shaded from a monospace
//! density ramp over a faint cell grid, turning slowly behind the composer.
//!
//! It repaints from its own cached view on the shared pulse clock at 5Hz. Each
//! tick still re-renders the shell (gpui dirties a view's ancestors), which is
//! the dominant cost, so the rate stays this low. The lease lapses as soon as
//! the view stops painting (a chat opens, a wallpaper is set, the window is
//! occluded), and reduced motion holds the last frame without scheduling.
use crate::new_thread_background_mask::SurfaceBounds;
use crate::theme::Theme;
use gpui::{
    App, AppContext, Bounds, Context, Entity, FontId, GlyphId, Hsla, IntoElement, ParentElement,
    Pixels, Render, RenderOnce, SharedString, Styled, TextRun, Window, canvas, div, fill, point,
    px, size,
};
use std::f32::consts::TAU;
use std::time::Instant;

/// Sparse to dense. Every glyph is plain ASCII so one shaped run covers it.
const RAMP: &str = ".:-=+tox%SX8#@";
const FONT_SIZE: f32 = 11.0;
const CELL_WIDTH: f32 = 10.0;
const CELL_HEIGHT: f32 = 16.0;
/// Pulse-clock ticks per repaint: 30Hz / 6. The motion is slow and quantized
/// to cells, so a faster clock would only spend frames.
const TICK_STRIDE: u64 = 6;
/// A paused or hidden sky resumes where it stopped instead of jumping ahead.
const MAX_STEP_SECS: f32 = 0.25;
/// Opens on a composed frame (moon lit, ring texture settled) rather than t=0.
const START_SECS: f32 = 48.0;
/// Below this a cell stays empty; the grid alone carries the texture there.
const MIN_DENSITY: f32 = 0.05;
/// Clearance around the composer over which glyphs return to full strength.
const FOCUS_FALLOFF: f32 = 140.0;
const FOCUS_FLOOR: f32 = 0.18;

const PLANET_SPIN: f32 = TAU / 150.0;
const MOON_SPIN: f32 = TAU / 90.0;
const RING_SPIN: f32 = TAU / 240.0;
const BREATH_PERIOD: f32 = 7.0;
const RING_INNER: f32 = 1.45;
const RING_OUTER: f32 = 2.05;
const RING_FLATTEN: f32 = 0.2;
const RING_ANGLE: f32 = -0.3;
/// Atmosphere thickness past the limb, in body radii.
const HALO: f32 = 0.09;
const STAR_CHANCE: f32 = 0.007;

#[derive(Clone, Copy, Debug, PartialEq)]
struct Body {
    x: f32,
    y: f32,
    radius: f32,
    tilt: f32,
    spin: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Layout {
    planet: Body,
    moon: Body,
}

impl Layout {
    fn new(width: f32, height: f32, t: f32) -> Self {
        let radius = (width.min(height) * 0.23).clamp(100.0, 280.0);
        Self {
            planet: Body {
                x: width * 0.72,
                y: height * 0.8,
                radius,
                tilt: 0.38,
                spin: PLANET_SPIN,
            },
            moon: Body {
                x: width * 0.18 + (t * 0.05).sin() * 14.0,
                y: height * 0.2 + (t * 0.037).cos() * 10.0,
                radius: radius * 0.34,
                tilt: -0.2,
                spin: MOON_SPIN,
            },
        }
    }
}

/// What one cell shows: how dense its glyph is (0..1) and whether it is one
/// of the few highlighted glyphs drawn in the accent.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Cell {
    density: f32,
    accent: bool,
}

fn hash3(x: i32, y: i32, z: i32) -> f32 {
    let mut h = (x as u32).wrapping_mul(0x8da6_b343)
        ^ (y as u32).wrapping_mul(0xd816_3841)
        ^ (z as u32).wrapping_mul(0xcb1a_b31f);
    h ^= h >> 13;
    h = h.wrapping_mul(0x5bd1_e995);
    h ^= h >> 15;
    (h & 0x00ff_ffff) as f32 / 16_777_216.0
}

fn smooth(t: f32) -> f32 {
    t * t * (3.0 - 2.0 * t)
}

fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    smooth(((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0))
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn value_noise([x, y, z]: [f32; 3]) -> f32 {
    let (ix, iy, iz) = (x.floor() as i32, y.floor() as i32, z.floor() as i32);
    let (fx, fy, fz) = (
        smooth(x - x.floor()),
        smooth(y - y.floor()),
        smooth(z - z.floor()),
    );
    let corner = |dx, dy, dz| hash3(ix + dx, iy + dy, iz + dz);
    let face = |dz| {
        lerp(
            lerp(corner(0, 0, dz), corner(1, 0, dz), fx),
            lerp(corner(0, 1, dz), corner(1, 1, dz), fx),
            fy,
        )
    };
    lerp(face(0), face(1), fz)
}

fn fbm(p: [f32; 3]) -> f32 {
    let mut sum = 0.0;
    let mut amplitude = 0.5;
    let mut p = p;
    for _ in 0..3 {
        sum += amplitude * value_noise(p);
        p = [p[0] * 2.03 + 1.7, p[1] * 2.03 - 0.9, p[2] * 2.03 + 3.1];
        amplitude *= 0.5;
    }
    sum / 0.875
}

/// Lit from the upper left and slightly in front, like the composer's shadow.
fn light() -> [f32; 3] {
    let [x, y, z] = [-0.55_f32, -0.5, 0.67];
    let len = (x * x + y * y + z * z).sqrt();
    [x / len, y / len, z / len]
}

/// A point on `body`'s visible hemisphere, as screen-facing normal and the
/// same normal carried into the body's tilted, spinning frame.
struct SurfacePoint {
    normal: [f32; 3],
    local: [f32; 3],
}

fn surface(body: &Body, x: f32, y: f32, t: f32) -> Option<SurfacePoint> {
    let dx = (x - body.x) / body.radius;
    let dy = (y - body.y) / body.radius;
    let d2 = dx * dx + dy * dy;
    if d2 > 1.0 {
        return None;
    }
    let dz = (1.0 - d2).sqrt();
    let (st, ct) = body.tilt.sin_cos();
    let (bx, by) = (ct * dx + st * dy, -st * dx + ct * dy);
    let (sa, ca) = (body.spin * t).sin_cos();
    Some(SurfacePoint {
        normal: [dx, dy, dz],
        local: [ca * bx + sa * dz, by, -sa * bx + ca * dz],
    })
}

fn diffuse(normal: [f32; 3]) -> f32 {
    let l = light();
    (normal[0] * l[0] + normal[1] * l[1] + normal[2] * l[2]).max(0.0)
}

fn planet_cell(body: &Body, x: f32, y: f32, t: f32) -> Option<Cell> {
    let point = surface(body, x, y, t)?;
    let [lx, ly, lz] = point.local;
    let turbulence = fbm([lx * 2.2 + 4.0, ly * 2.2, lz * 2.2]);
    let bands = 0.5 + 0.5 * (ly * 9.0 + turbulence * 4.0).sin();
    let lit = diffuse(point.normal);
    // A dim limb keeps the night side's outline readable.
    let limb = smoothstep(0.6, 0.0, point.normal[2]) * 0.14;
    // One storm, fixed to the surface so it rides the rotation into view.
    let storm = {
        let [sx, sy, sz] = [0.62_f32, 0.32, 0.72];
        let (ex, ey, ez) = (lx - sx, ly - sy, lz - sz);
        (ex * ex + ey * ey * 4.0 + ez * ez) < 0.03
    };
    Some(Cell {
        density: (lit.powf(0.7) * (0.4 + 0.6 * bands)).max(limb).min(1.0),
        accent: storm && lit > 0.25,
    })
}

/// The atmosphere: one breathing band of sparse glyphs just past the limb.
fn halo_cell(body: &Body, x: f32, y: f32, t: f32) -> Option<Cell> {
    let dx = (x - body.x) / body.radius;
    let dy = (y - body.y) / body.radius;
    let past = (dx * dx + dy * dy).sqrt() - 1.0;
    if !(0.0..HALO).contains(&past) {
        return None;
    }
    let breath = 0.5 + 0.5 * (t * TAU / BREATH_PERIOD).sin();
    Some(Cell {
        density: (0.08 + 0.1 * breath) * (1.0 - past / HALO),
        accent: false,
    })
}

fn moon_cell(body: &Body, x: f32, y: f32, t: f32) -> Option<Cell> {
    let point = surface(body, x, y, t)?;
    let [lx, ly, lz] = point.local;
    let craters = fbm([lx * 3.5, ly * 3.5 + 9.0, lz * 3.5]);
    let pits = smoothstep(0.55, 0.7, craters);
    let limb = smoothstep(0.6, 0.0, point.normal[2]) * 0.14;
    Some(Cell {
        density: (diffuse(point.normal) * (0.95 - 0.55 * pits)).max(limb),
        accent: false,
    })
}

/// The ring in the planet's frame: `Some(density)` where the ring covers
/// `(x, y)`, plus whether that stretch is the near half that passes in
/// front of the planet.
fn ring_cell(body: &Body, x: f32, y: f32, t: f32) -> Option<(f32, bool)> {
    let dx = (x - body.x) / body.radius;
    let dy = (y - body.y) / body.radius;
    let (sa, ca) = RING_ANGLE.sin_cos();
    let u = ca * dx + sa * dy;
    let v = (-sa * dx + ca * dy) / RING_FLATTEN;
    let r = (u * u + v * v).sqrt();
    if !(RING_INNER..=RING_OUTER).contains(&r) {
        return None;
    }
    let edge = smoothstep(RING_INNER, RING_INNER + 0.08, r)
        * (1.0 - smoothstep(RING_OUTER - 0.12, RING_OUTER, r));
    let gap = 1.0 - 0.85 * (1.0 - smoothstep(0.0, 0.05, (r - 1.78).abs()));
    let phi = v.atan2(u) + t * RING_SPIN;
    let grain = value_noise([phi.cos() * 5.0, phi.sin() * 5.0, r * 7.0]);
    Some((edge * gap * (0.18 + 0.4 * grain), v > 0.0))
}

fn star_cell(col: i32, row: i32, t: f32) -> Option<Cell> {
    if hash3(col, row, 7) >= STAR_CHANCE {
        return None;
    }
    let phase = hash3(col, row, 11) * TAU;
    let rate = 0.6 + hash3(col, row, 13) * 1.2;
    let twinkle = (0.5 + 0.5 * (t * rate + phase).sin()).powi(3);
    Some(Cell {
        density: 0.12 + 0.4 * twinkle,
        accent: hash3(col, row, 17) < 0.2,
    })
}

fn sample(layout: &Layout, col: i32, row: i32, x: f32, y: f32, t: f32) -> Option<Cell> {
    let planet = planet_cell(&layout.planet, x, y, t);
    if let Some((density, near)) = ring_cell(&layout.planet, x, y, t)
        && (near || planet.is_none())
    {
        return Some(Cell {
            density: density.max(planet.map_or(0.0, |cell| cell.density * 0.35)),
            accent: false,
        });
    }
    planet
        .or_else(|| halo_cell(&layout.planet, x, y, t))
        .or_else(|| moon_cell(&layout.moon, x, y, t))
        .or_else(|| star_cell(col, row, t))
}

/// Glyphs over the composer (and just around it) step back so the prompt
/// stays the brightest thing on screen.
fn focus_factor(focus: Option<Bounds<Pixels>>, x: f32, y: f32) -> f32 {
    let Some(focus) = focus else {
        return 1.0;
    };
    let dx = (f32::from(focus.left()) - x)
        .max(x - f32::from(focus.right()))
        .max(0.0);
    let dy = (f32::from(focus.top()) - y)
        .max(y - f32::from(focus.bottom()))
        .max(0.0);
    lerp(
        FOCUS_FLOOR,
        1.0,
        smoothstep(0.0, FOCUS_FALLOFF, (dx * dx + dy * dy).sqrt()),
    )
}

fn ramp_index(density: f32) -> usize {
    let last = RAMP.len() - 1;
    ((density.clamp(0.0, 1.0) * last as f32).round() as usize).min(last)
}

/// Advances only while animating, and by at most [`MAX_STEP_SECS`] per paint.
#[derive(Debug)]
struct SceneClock {
    elapsed: f32,
    last: Option<Instant>,
}

impl SceneClock {
    fn new() -> Self {
        Self {
            elapsed: START_SECS,
            last: None,
        }
    }

    fn advance(&mut self, now: Instant, running: bool) -> f32 {
        if running {
            if let Some(last) = self.last {
                self.elapsed += now
                    .saturating_duration_since(last)
                    .as_secs_f32()
                    .min(MAX_STEP_SECS);
            }
            self.last = Some(now);
        } else {
            self.last = None;
        }
        self.elapsed
    }
}

#[derive(Clone, Debug, PartialEq)]
struct Style {
    family: SharedString,
    ink: Hsla,
    accent: Hsla,
    grid: Hsla,
    strength: f32,
}

impl Style {
    fn new(theme: &Theme, opacity: f32, cx: &App) -> Self {
        let dark = theme.appearance.is_dark();
        let opacity = opacity.clamp(0.0, 1.0);
        Self {
            family: crate::typography::code_effective_family_name(cx),
            ink: theme.text,
            accent: theme.accent,
            grid: theme
                .text
                .opacity(if dark { 0.03 } else { 0.035 } * opacity),
            strength: if dark { 0.3 } else { 0.24 } * opacity,
        }
    }
}

/// The animated sky. `opacity` folds in the dock dissolve, so a departing
/// sky fades with the composer it surrounds.
pub(crate) fn ascii_sky(composer: SurfaceBounds, opacity: f32) -> impl IntoElement {
    AsciiSky { composer, opacity }
}

#[derive(IntoElement)]
struct AsciiSky {
    composer: SurfaceBounds,
    opacity: f32,
}

impl RenderOnce for AsciiSky {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let style = Style::new(Theme::of(cx), self.opacity, cx);
        let view = window.with_global_id("new-thread-ascii-sky".into(), |id, window| {
            window.with_element_state(id, |previous: Option<Entity<AsciiSkyView>>, _| {
                let view = previous.unwrap_or_else(|| {
                    cx.new(|_| AsciiSkyView {
                        clock: SceneClock::new(),
                        style: style.clone(),
                        composer: self.composer.clone(),
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
        div()
            .absolute()
            .inset_0()
            .child(view.cached(gpui::StyleRefinement::default().size_full()))
    }
}

struct AsciiSkyView {
    clock: SceneClock,
    style: Style,
    composer: SurfaceBounds,
}

impl Render for AsciiSkyView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let running = !cx.reduce_motion();
        if running {
            crate::motion::pulse_lease_every(cx.entity_id(), TICK_STRIDE, cx);
        }
        let t = self.clock.advance(Instant::now(), running);
        let style = self.style.clone();
        let composer = self.composer.clone();
        canvas(
            |_, _, _| {},
            move |bounds, _, window, _| {
                if style.strength > 0.0 {
                    paint(bounds, t, &style, composer.get(), window);
                }
            },
        )
        .size_full()
    }
}

fn paint(
    bounds: Bounds<Pixels>,
    t: f32,
    style: &Style,
    focus: Option<Bounds<Pixels>>,
    window: &mut Window,
) {
    let width = f32::from(bounds.size.width);
    let height = f32::from(bounds.size.height);
    if width <= 0.0 || height <= 0.0 {
        return;
    }
    let run = TextRun {
        len: RAMP.len(),
        font: gpui::font(style.family.clone()),
        color: style.ink,
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let line = window
        .text_system()
        .shape_line(RAMP.into(), px(FONT_SIZE), &[run], None);
    let mut glyphs: [Option<(FontId, GlyphId)>; RAMP.len()] = [None; RAMP.len()];
    for run in &line.runs {
        for glyph in &run.glyphs {
            if let Some(slot) = glyphs.get_mut(glyph.index) {
                *slot = Some((run.font_id, glyph.id));
            }
        }
    }
    let advance = f32::from(line.width) / RAMP.len() as f32;
    let inset_x = ((CELL_WIDTH - advance) / 2.0).max(0.0);
    let baseline =
        (CELL_HEIGHT - f32::from(line.ascent + line.descent)) / 2.0 + f32::from(line.ascent);

    let cols = (width / CELL_WIDTH).ceil() as i32;
    let rows = (height / CELL_HEIGHT).ceil() as i32;
    let hairline = px(1.0 / window.scale_factor().max(1.0));
    for col in 0..=cols {
        let x = bounds.origin.x + px(col as f32 * CELL_WIDTH);
        window.paint_quad(fill(
            Bounds::new(
                point(x, bounds.origin.y),
                size(hairline, bounds.size.height),
            ),
            style.grid,
        ));
    }
    for row in 0..=rows {
        let y = bounds.origin.y + px(row as f32 * CELL_HEIGHT);
        window.paint_quad(fill(
            Bounds::new(point(bounds.origin.x, y), size(bounds.size.width, hairline)),
            style.grid,
        ));
    }

    let layout = Layout::new(width, height, t);
    let font_size = px(FONT_SIZE);
    for row in 0..rows {
        let y = (row as f32 + 0.5) * CELL_HEIGHT;
        for col in 0..cols {
            let x = (col as f32 + 0.5) * CELL_WIDTH;
            let Some(cell) = sample(&layout, col, row, x, y, t) else {
                continue;
            };
            if cell.density < MIN_DENSITY {
                continue;
            }
            let Some((font_id, glyph_id)) = glyphs[ramp_index(cell.density)] else {
                continue;
            };
            let window_x = f32::from(bounds.origin.x) + x;
            let window_y = f32::from(bounds.origin.y) + y;
            let alpha = style.strength
                * (0.35 + 0.65 * cell.density)
                * focus_factor(focus, window_x, window_y);
            let color = if cell.accent {
                style.accent.opacity((alpha * 2.2).min(1.0))
            } else {
                style.ink.opacity(alpha)
            };
            let origin = point(
                bounds.origin.x + px(col as f32 * CELL_WIDTH + inset_x),
                bounds.origin.y + px(row as f32 * CELL_HEIGHT + baseline),
            );
            let _ = window.paint_glyph(origin, font_id, glyph_id, font_size, color);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn layout() -> Layout {
        Layout::new(1200.0, 800.0, START_SECS)
    }

    #[test]
    fn planet_is_lit_from_the_upper_left() {
        let layout = layout();
        let p = layout.planet;
        let offset = p.radius * 0.6;
        let lit = planet_cell(&p, p.x - offset, p.y - offset, 0.0).unwrap();
        let dark = planet_cell(&p, p.x + offset, p.y + offset, 0.0).unwrap();
        assert!(lit.density > dark.density + 0.3, "{lit:?} vs {dark:?}");
    }

    #[test]
    fn ring_hides_behind_the_planet_on_its_far_half() {
        let layout = layout();
        let p = layout.planet;
        let (sa, ca) = RING_ANGLE.sin_cos();
        // Walk the ring's middle radius and pair each far-half point inside
        // the disk with its mirrored near-half point.
        let mut checked = 0;
        for step in 0..360 {
            let angle = step as f32 / 360.0 * TAU;
            let (u, v) = (1.6 * angle.cos(), 1.6 * angle.sin());
            let (dx, dy) = (
                ca * u - sa * v * RING_FLATTEN,
                sa * u + ca * v * RING_FLATTEN,
            );
            if dx * dx + dy * dy >= 0.9 {
                continue;
            }
            let (x, y) = (p.x + dx * p.radius, p.y + dy * p.radius);
            let (_, near) = ring_cell(&p, x, y, 0.0).unwrap();
            let shown = sample(&layout, -1, -1, x, y, 0.0).unwrap();
            let planet = planet_cell(&p, x, y, 0.0).unwrap();
            if near {
                assert!(!shown.accent);
            } else {
                assert_eq!(shown, planet);
            }
            checked += 1;
        }
        assert!(checked > 0);
    }

    #[test]
    fn open_sky_stays_mostly_empty() {
        let layout = layout();
        let filled = (0..60)
            .flat_map(|row| (0..40).map(move |col| (col, row)))
            .filter(|&(col, row)| {
                let (x, y) = (500.0 + col as f32 * CELL_WIDTH, row as f32 * CELL_HEIGHT);
                planet_cell(&layout.planet, x, y, 0.0).is_none()
                    && ring_cell(&layout.planet, x, y, 0.0).is_none()
                    && moon_cell(&layout.moon, x, y, 0.0).is_none()
                    && star_cell(col, row, 0.0).is_some()
            })
            .count();
        assert!(filled < 2400 / 40, "{filled} stars in 2400 cells");
    }

    #[test]
    fn composer_dims_the_cells_behind_it() {
        let composer = Some(Bounds::new(
            point(px(400.0), px(300.0)),
            size(px(500.0), px(120.0)),
        ));
        assert_eq!(focus_factor(composer, 600.0, 350.0), FOCUS_FLOOR);
        assert!(focus_factor(composer, 600.0, 450.0) > FOCUS_FLOOR);
        assert_eq!(focus_factor(composer, 100.0, 50.0), 1.0);
        assert_eq!(focus_factor(None, 600.0, 350.0), 1.0);
    }

    #[test]
    fn ramp_covers_every_density() {
        assert_eq!(ramp_index(0.0), 0);
        assert_eq!(ramp_index(1.0), RAMP.len() - 1);
        assert_eq!(ramp_index(4.0), RAMP.len() - 1);
        assert!((1..=10).all(|i| ramp_index(i as f32 / 10.0) >= ramp_index((i - 1) as f32 / 10.0)));
    }

    #[test]
    fn clock_holds_still_when_paused_and_never_jumps() {
        let start = Instant::now();
        let mut clock = SceneClock::new();
        assert_eq!(clock.advance(start, true), START_SECS);
        let t = clock.advance(start + Duration::from_millis(100), true);
        assert!((t - START_SECS - 0.1).abs() < 1e-4);
        assert_eq!(clock.advance(start + Duration::from_secs(5), false), t);
        assert_eq!(clock.advance(start + Duration::from_secs(9), true), t);
        let resumed = clock.advance(start + Duration::from_secs(60), true);
        assert!((resumed - t - MAX_STEP_SECS).abs() < 1e-4);
    }
}
