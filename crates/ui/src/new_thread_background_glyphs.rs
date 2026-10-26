//! The empty new-thread canvas without a wallpaper: a ringed planet, a small
//! moon and a starfield drawn in monospace glyphs on a faint cell grid.
//!
//! The field lives in its own cached view on the ambient clock, so typing and
//! other shell renders replay its last paint instead of recomputing it, and
//! the clock parks once the canvas unmounts. Reduced motion paints one still
//! frame. Glyphs are shaped once and painted as sprites, never as text runs.

use std::time::Instant;

use gpui::{
    AnyElement, App, AppContext as _, Bounds, Context, Entity, FontId, GlyphId, Hsla, IntoElement,
    Pixels, Render, RenderOnce, SharedString, Styled as _, TextRun, Window, canvas, fill, point,
    px, size,
};

use crate::theme::Theme;

/// Dim to bright. Stars reuse `.`, `+` and `*` from the same set.
const RAMP: &str = ".:-=+o*x%#@";
const STAR_DIM: usize = 0;
const STAR_MID: usize = 4;
const STAR_PEAK: usize = 6;
/// Square cells keep the sphere round and the glyphs airy.
const CELL: f32 = 14.0;
const FONT_SIZE: f32 = 10.5;
/// Below this a cell stays empty: no sprite for invisible ink.
const FLOOR: f32 = 0.04;
/// The pose painted when motion is reduced.
const STILL_TIME: f32 = 24.0;

/// Everything the paint depends on besides time. A change re-renders the
/// cached view; anything else replays its previous paint.
#[derive(Clone, PartialEq)]
pub(crate) struct FieldSpec {
    ink: Hsla,
    highlight: Hsla,
    family: SharedString,
    /// The composer's surface; glyphs recede around it and the greeting above.
    focus: Option<Bounds<Pixels>>,
    opacity: f32,
}

impl FieldSpec {
    pub fn new(theme: &Theme, focus: Option<Bounds<Pixels>>, opacity: f32) -> Self {
        let light = theme.appearance.is_light();
        Self {
            ink: theme.text,
            highlight: if light {
                theme.glyph.deep
            } else {
                theme.glyph.light
            },
            family: theme.font_mono.clone(),
            focus,
            opacity: opacity.clamp(0.0, 1.0),
        }
    }
}

pub(crate) fn glyph_field(spec: FieldSpec) -> AnyElement {
    GlyphField { spec }.into_any_element()
}

#[derive(IntoElement)]
struct GlyphField {
    spec: FieldSpec,
}

impl RenderOnce for GlyphField {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let spec = self.spec;
        let view = window.with_global_id("new-thread-glyph-field".into(), |id, window| {
            window.with_element_state(id, |previous: Option<Entity<FieldView>>, _| {
                let view = previous.unwrap_or_else(|| {
                    cx.new(|_| FieldView {
                        spec: spec.clone(),
                        epoch: Instant::now(),
                        glyphs: None,
                    })
                });
                view.update(cx, |view, cx| {
                    if view.spec != spec {
                        view.spec = spec.clone();
                        cx.notify();
                    }
                });
                (view.clone(), view)
            })
        });
        view.cached(gpui::StyleRefinement::default().size_full())
    }
}

struct FieldView {
    spec: FieldSpec,
    epoch: Instant,
    glyphs: Option<GlyphSet>,
}

impl Render for FieldView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let time = if crate::motion::reduced_motion(cx) {
            STILL_TIME
        } else {
            crate::motion::ambient_lease(cx.entity_id(), cx);
            self.epoch.elapsed().as_secs_f32()
        };
        if self
            .glyphs
            .as_ref()
            .is_none_or(|glyphs| glyphs.family != self.spec.family)
        {
            self.glyphs = Some(GlyphSet::shape(&self.spec.family, window));
        }
        let glyphs = self.glyphs.clone();
        let spec = self.spec.clone();
        canvas(
            |_, _, _| (),
            move |bounds, _, window, _| {
                if let Some(glyphs) = &glyphs {
                    paint(bounds, &spec, glyphs, time, window);
                }
            },
        )
        .size_full()
    }
}

#[derive(Clone)]
struct GlyphSet {
    family: SharedString,
    ids: Vec<Option<(FontId, GlyphId)>>,
    advance: f32,
    ascent: f32,
    descent: f32,
}

impl GlyphSet {
    fn shape(family: &SharedString, window: &Window) -> Self {
        let run = TextRun {
            len: RAMP.len(),
            font: gpui::Font {
                family: family.clone(),
                features: gpui::FontFeatures::default(),
                fallbacks: None,
                weight: gpui::FontWeight::NORMAL,
                style: gpui::FontStyle::Normal,
            },
            color: gpui::black(),
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let line = window
            .text_system()
            .shape_line(RAMP.into(), px(FONT_SIZE), &[run], None);
        let mut ids = vec![None; RAMP.len()];
        for shaped in &line.runs {
            for glyph in &shaped.glyphs {
                if let Some(slot) = ids.get_mut(glyph.index) {
                    *slot = Some((shaped.font_id, glyph.id));
                }
            }
        }
        Self {
            family: family.clone(),
            ids,
            advance: f32::from(line.width) / RAMP.len() as f32,
            ascent: f32::from(line.ascent),
            descent: f32::from(line.descent),
        }
    }
}

fn paint(
    bounds: Bounds<Pixels>,
    spec: &FieldSpec,
    glyphs: &GlyphSet,
    time: f32,
    window: &mut Window,
) {
    if spec.opacity <= 0.001 {
        return;
    }
    let left = f32::from(bounds.origin.x);
    let top = f32::from(bounds.origin.y);
    let width = f32::from(bounds.size.width);
    let height = f32::from(bounds.size.height);
    let cols = (width / CELL).ceil() as i32;
    let rows = (height / CELL).ceil() as i32;
    if cols <= 0 || rows <= 0 {
        return;
    }
    let scale = window.scale_factor();
    let hairline = 1.0 / scale;
    let grid = spec.ink.opacity(0.022 * spec.opacity);
    for col in 1..cols {
        let x = ((left + col as f32 * CELL) * scale).round() / scale;
        window.paint_quad(fill(
            Bounds::new(
                point(px(x), bounds.origin.y),
                size(px(hairline), bounds.size.height),
            ),
            grid,
        ));
    }
    for row in 1..rows {
        let y = ((top + row as f32 * CELL) * scale).round() / scale;
        window.paint_quad(fill(
            Bounds::new(
                point(bounds.origin.x, px(y)),
                size(bounds.size.width, px(hairline)),
            ),
            grid,
        ));
    }

    let scene = Scene::new(width, height, time);
    let baseline = (CELL + glyphs.ascent - glyphs.descent) * 0.5;
    let inset = (CELL - glyphs.advance) * 0.5;
    for row in 0..rows {
        for col in 0..cols {
            let x = (col as f32 + 0.5) * CELL;
            let y = (row as f32 + 0.5) * CELL;
            let Some(cell) = scene.sample(col, row, x, y) else {
                continue;
            };
            let presence = focus_mask(spec.focus, left + x, top + y) * title_mask(y) * spec.opacity;
            if presence <= 0.01 {
                continue;
            }
            let cell_left = left + col as f32 * CELL;
            let cell_top = top + row as f32 * CELL;
            if cell.lit {
                window.paint_quad(fill(
                    Bounds::new(point(px(cell_left), px(cell_top)), size(px(CELL), px(CELL))),
                    spec.ink.opacity(0.035 * presence),
                ));
            }
            let Some((font_id, glyph_id)) = glyphs.ids[cell.glyph] else {
                continue;
            };
            let color = if cell.accent {
                spec.highlight.opacity(0.7 * presence)
            } else {
                spec.ink.opacity((0.05 + 0.24 * cell.intensity) * presence)
            };
            let _ = window.paint_glyph(
                point(px(cell_left + inset), px(cell_top + baseline)),
                font_id,
                glyph_id,
                px(FONT_SIZE),
                color,
            );
        }
    }
}

/// Glyphs recede to a trace around the composer and the greeting above it,
/// then return over a soft margin.
fn focus_mask(focus: Option<Bounds<Pixels>>, x: f32, y: f32) -> f32 {
    let Some(focus) = focus else {
        return 1.0;
    };
    let left = f32::from(focus.left()) - 48.0;
    let right = f32::from(focus.right()) + 48.0;
    let top = f32::from(focus.top()) - 150.0;
    let bottom = f32::from(focus.bottom()) + 56.0;
    let dx = (left - x).max(x - right).max(0.0);
    let dy = (top - y).max(y - bottom).max(0.0);
    let distance = (dx * dx + dy * dy).sqrt();
    0.15 + 0.85 * smoothstep(0.0, 140.0, distance)
}

/// Clear the overlaid titlebar band.
fn title_mask(y: f32) -> f32 {
    smoothstep(
        Theme::TITLEBAR_HEIGHT * 0.5,
        Theme::TITLEBAR_HEIGHT + 40.0,
        y,
    )
}

fn smoothstep(from: f32, to: f32, value: f32) -> f32 {
    let t = ((value - from) / (to - from)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Cell {
    glyph: usize,
    intensity: f32,
    accent: bool,
    /// The brightest planet cells also tint their cell, like a lit pixel.
    lit: bool,
}

impl Cell {
    fn shade(intensity: f32, accent: bool) -> Option<Self> {
        let intensity = intensity.clamp(0.0, 1.0);
        (intensity >= FLOOR).then(|| Self {
            glyph: ((intensity.powf(0.9) * (RAMP.len() - 1) as f32).round() as usize)
                .min(RAMP.len() - 1),
            intensity,
            accent,
            lit: false,
        })
    }
}

/// One frame's geometry, in canvas pixels.
struct Scene {
    time: f32,
    planet: (f32, f32),
    radius: f32,
    tilt: (f32, f32),
    light: [f32; 3],
    spin: f32,
    moon: (f32, f32),
    moon_radius: f32,
}

/// Two ringlets either side of a dark gap, in planet radii.
const RING_INNER: f32 = 1.4;
const RING_GAP: (f32, f32) = (1.6, 1.7);
const RING_OUTER: f32 = 1.9;
const RING_FLATTEN: f32 = 0.24;

impl Scene {
    fn new(width: f32, height: f32, time: f32) -> Self {
        let tau = std::f32::consts::TAU;
        let unit = width.min(height).max(1.0);
        let wave = |period: f32, phase: f32| (time * tau / period + phase).sin();
        let tilt = 0.32 + 0.03 * wave(53.0, 0.0);
        let sway = 0.25 * wave(120.0, 0.7);
        let light = normalize([-0.45 + sway, -0.55, 0.7]);
        let orbit = time * tau / 140.0;
        Self {
            time,
            planet: (
                width * 0.80 + unit * 0.02 * wave(97.0, 0.0),
                height * 0.76 + unit * 0.015 * wave(71.0, 1.3),
            ),
            radius: unit * 0.23,
            tilt: (tilt.cos(), tilt.sin()),
            light,
            spin: time * tau / 50.0,
            moon: (
                width * 0.15 + unit * 0.07 * orbit.cos(),
                height * 0.25 + unit * 0.025 * orbit.sin(),
            ),
            moon_radius: unit * 0.09,
        }
    }

    fn sample(&self, col: i32, row: i32, x: f32, y: f32) -> Option<Cell> {
        let dx = (x - self.planet.0) / self.radius;
        let dy = (y - self.planet.1) / self.radius;
        // Ring plane frame: u along the ring, v across it (positive = near side).
        let u = dx * self.tilt.0 + dy * self.tilt.1;
        let v = -dx * self.tilt.1 + dy * self.tilt.0;
        let on_planet = dx * dx + dy * dy < 1.0;
        let ring = self.ring(u, v);
        if on_planet {
            if v > 0.0
                && let Some(ring) = ring
            {
                return Some(ring);
            }
            return self.planet(dx, dy, u, v);
        }
        if ring.is_some() {
            return ring;
        }
        let mx = (x - self.moon.0) / self.moon_radius;
        let my = (y - self.moon.1) / self.moon_radius;
        if mx * mx + my * my < 1.0 {
            return self.moon(mx, my);
        }
        self.star(col, row)
    }

    fn planet(&self, dx: f32, dy: f32, u: f32, v: f32) -> Option<Cell> {
        let dz = (1.0 - dx * dx - dy * dy).max(0.0).sqrt();
        let lambert = dot([dx, dy, dz], self.light).max(0.0);
        let longitude = u.atan2(dz) + self.spin;
        let band = 0.5 + 0.5 * (v * 6.0 + 0.9 * (longitude * 1.7 + v * 3.0).sin()).sin();
        let storm_lon = wrap(longitude - 0.6);
        let storm = (-(storm_lon * storm_lon * 3.0 + (v - 0.35) * (v - 0.35) * 60.0)).exp();
        let intensity = 0.05 + lambert.powf(1.2) * (0.35 + 0.65 * band);
        let mut cell = Cell::shade(intensity, storm > 0.5 && lambert > 0.2)?;
        cell.lit = intensity > 0.9;
        Some(cell)
    }

    fn ring(&self, u: f32, v: f32) -> Option<Cell> {
        let r = (u * u + (v / RING_FLATTEN).powi(2)).sqrt();
        let band = if (RING_INNER..RING_GAP.0).contains(&r) {
            0.6
        } else if (RING_GAP.1..RING_OUTER).contains(&r) {
            0.42
        } else {
            return None;
        };
        let shade = 0.8 - 0.2 * self.light[0].signum() * u.signum();
        Cell::shade(band * shade, false)
    }

    fn moon(&self, mx: f32, my: f32) -> Option<Cell> {
        let mz = (1.0 - mx * mx - my * my).max(0.0).sqrt();
        let lambert = dot([mx, my, mz], self.light).max(0.0);
        let crater = |cx: f32, cy: f32, r: f32| {
            let d = ((mx - cx).powi(2) + (my - cy).powi(2)).sqrt();
            if d < r { 0.55 } else { 1.0 }
        };
        let relief = crater(-0.3, 0.2, 0.28) * crater(0.35, -0.25, 0.2);
        Cell::shade(0.05 + 0.75 * lambert * relief, false)
    }

    fn star(&self, col: i32, row: i32) -> Option<Cell> {
        let seed = hash(col, row);
        if unit(seed) > 0.018 {
            return None;
        }
        let rate = 3.0 + 4.0 * unit(seed.rotate_left(7));
        let twinkle = 0.5
            + 0.5
                * (self.time * std::f32::consts::TAU / rate + unit(seed.rotate_left(13)) * 6.3)
                    .sin();
        let brightness = (0.2 + 0.3 * unit(seed.rotate_left(19))) * (0.4 + 0.6 * twinkle);
        let glyph = if twinkle > 0.92 {
            STAR_PEAK
        } else if brightness > 0.3 {
            STAR_MID
        } else {
            STAR_DIM
        };
        Some(Cell {
            glyph,
            intensity: brightness,
            accent: unit(seed.rotate_left(23)) < 0.08 && twinkle > 0.75,
            lit: false,
        })
    }
}

fn hash(col: i32, row: i32) -> u32 {
    let mut h = (col as u32).wrapping_mul(0x8da6_b343) ^ (row as u32).wrapping_mul(0xd816_3841);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2c1b_3c6d);
    h ^= h >> 12;
    h = h.wrapping_mul(0x297a_2d39);
    h ^ (h >> 15)
}

fn unit(seed: u32) -> f32 {
    (seed & 0xffff) as f32 / 65535.0
}

fn wrap(angle: f32) -> f32 {
    let tau = std::f32::consts::TAU;
    (angle + std::f32::consts::PI).rem_euclid(tau) - std::f32::consts::PI
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn normalize(v: [f32; 3]) -> [f32; 3] {
    let length = dot(v, v).sqrt().max(f32::EPSILON);
    [v[0] / length, v[1] / length, v[2] / length]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(width: f32, height: f32, time: f32) -> Vec<Option<Cell>> {
        let scene = Scene::new(width, height, time);
        let cols = (width / CELL).ceil() as i32;
        let rows = (height / CELL).ceil() as i32;
        (0..rows)
            .flat_map(|row| (0..cols).map(move |col| (col, row)))
            .map(|(col, row)| {
                scene.sample(
                    col,
                    row,
                    (col as f32 + 0.5) * CELL,
                    (row as f32 + 0.5) * CELL,
                )
            })
            .collect()
    }

    #[test]
    fn ramp_glyphs_are_single_byte_and_star_slots_point_at_their_glyphs() {
        assert!(RAMP.is_ascii());
        assert_eq!(&RAMP[STAR_DIM..=STAR_DIM], ".");
        assert_eq!(&RAMP[STAR_MID..=STAR_MID], "+");
        assert_eq!(&RAMP[STAR_PEAK..=STAR_PEAK], "*");
    }

    #[test]
    fn brighter_cells_never_pick_a_lighter_glyph() {
        let mut last = 0;
        for step in 0..=100 {
            if let Some(cell) = Cell::shade(step as f32 / 100.0, false) {
                assert!(cell.glyph >= last);
                last = cell.glyph;
            }
        }
        assert_eq!(last, RAMP.len() - 1);
        assert!(Cell::shade(FLOOR * 0.5, false).is_none());
    }

    #[test]
    fn a_frame_is_a_pure_function_of_time_and_size() {
        assert_eq!(frame(1200.0, 800.0, 7.5), frame(1200.0, 800.0, 7.5));
        assert_ne!(frame(1200.0, 800.0, 7.5), frame(1200.0, 800.0, 9.0));
    }

    #[test]
    fn the_field_stays_sparse_across_window_shapes() {
        for (width, height) in [(1200.0, 800.0), (700.0, 900.0), (1900.0, 600.0)] {
            for time in [0.0, STILL_TIME, 300.0] {
                let cells = frame(width, height, time);
                let inked = cells.iter().flatten().count() as f32 / cells.len() as f32;
                assert!(
                    (0.08..0.45).contains(&inked),
                    "{width}x{height}@{time}: {inked}"
                );
                let accents = cells.iter().flatten().filter(|cell| cell.accent).count();
                assert!(accents as f32 / cells.len() as f32 <= 0.02);
            }
        }
    }

    #[test]
    fn the_composer_and_greeting_area_recede() {
        let composer = Bounds::new(point(px(300.0), px(400.0)), size(px(600.0), px(120.0)));
        let inside = focus_mask(Some(composer), 600.0, 460.0);
        let greeting = focus_mask(Some(composer), 600.0, 300.0);
        let far = focus_mask(Some(composer), 50.0, 780.0);
        assert!(inside < 0.2 && greeting < 0.2);
        assert_eq!(far, 1.0);
        assert_eq!(focus_mask(None, 600.0, 460.0), 1.0);
        assert_eq!(title_mask(0.0), 0.0);
        assert_eq!(title_mask(200.0), 1.0);
    }
}
