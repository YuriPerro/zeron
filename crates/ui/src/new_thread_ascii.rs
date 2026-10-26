//! The empty new-chat canvas without a wallpaper: slow clouds drawn out of
//! monospace glyphs on a faint grid, kept clear around the composer. It
//! repaints from its own cached view on the ambient clock, so nothing around
//! it re-renders for it, and the clock parks once the empty state unmounts.
use crate::motion;
use crate::new_thread_background_mask::SurfaceBounds;
use crate::theme::Theme;
use gpui::{
    App, AppContext, Bounds, Context, Entity, FontId, GlyphId, Hsla, IntoElement, ParentElement,
    Pixels, Render, RenderOnce, SharedString, Styled, TextRun, Window, canvas, div, fill, point,
    px, size,
};
use std::f32::consts::{PI, TAU};

/// Lightest to heaviest ink. Plain ASCII, so every mono face carries it.
const RAMP: &str = ".:-=+tox%S#@";
const RAMP_LEN: usize = RAMP.len();
const FONT_SIZE: f32 = 11.0;
const LINE_HEIGHT: f32 = 15.0;
/// Bounds the per-frame glyph work on very large windows: cells grow instead.
const MAX_CELLS: f32 = 12_000.0;
/// Grid lines fall every few cells so the lattice stays a texture, not a hatch.
const GRID_EVERY: (u32, u32) = (4, 2);
/// Starts the clock mid-drift so the first frame is already composed.
const TIME_OFFSET: f32 = 140.0;
const SPARK_PERIOD: f32 = 7.0;

#[derive(Clone, Copy)]
struct Drifter {
    /// Rest position: x in widths, y in heights.
    home: (f32, f32),
    sway: (f32, f32),
    period: (f32, f32),
    phase: f32,
    /// Radius in heights.
    size: f32,
}

const DRIFTERS: [Drifter; 5] = [
    Drifter {
        home: (0.14, 0.26),
        sway: (0.07, 0.07),
        period: (97.0, 71.0),
        phase: 0.0,
        size: 0.12,
    },
    Drifter {
        home: (0.80, 0.20),
        sway: (0.06, 0.06),
        period: (83.0, 109.0),
        phase: 1.7,
        size: 0.14,
    },
    Drifter {
        home: (0.88, 0.66),
        sway: (0.05, 0.09),
        period: (121.0, 89.0),
        phase: 3.1,
        size: 0.11,
    },
    Drifter {
        home: (0.20, 0.82),
        sway: (0.08, 0.05),
        period: (101.0, 77.0),
        phase: 4.4,
        size: 0.13,
    },
    Drifter {
        home: (0.52, 1.02),
        sway: (0.12, 0.03),
        period: (133.0, 95.0),
        phase: 5.6,
        size: 0.10,
    },
];

/// The ellipse kept empty around the composer, in heights.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Calm {
    center: (f32, f32),
    radius: (f32, f32),
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Ink {
    Body,
    Dust,
    Spark,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Cell {
    glyph: usize,
    strength: f32,
    ink: Ink,
}

/// One frame of the cloud field. Coordinates are in heights, so shapes keep
/// their proportions at any window aspect.
struct Field {
    t: f32,
    calm: Calm,
    centers: [(f32, f32, f32); DRIFTERS.len()],
}

impl Field {
    fn new(aspect: f32, time: f32, calm: Calm) -> Self {
        let t = time + TIME_OFFSET;
        let centers = DRIFTERS.map(|drifter| {
            let x = drifter.home.0
                + drifter.sway.0 * (t * TAU / drifter.period.0 + drifter.phase).sin();
            let y = drifter.home.1
                + drifter.sway.1 * (t * TAU / drifter.period.1 + drifter.phase * 1.3).cos();
            let breath = 1.0 + 0.08 * (t * TAU / 9.0 + drifter.phase).sin();
            (x * aspect, y, drifter.size * breath)
        });
        Self { t, calm, centers }
    }

    fn clearance(&self, x: f32, y: f32) -> f32 {
        let dx = (x - self.calm.center.0) / self.calm.radius.0;
        let dy = (y - self.calm.center.1) / self.calm.radius.1;
        smoothstep(1.0, 2.3, dx * dx + dy * dy)
    }

    fn density(&self, x: f32, y: f32) -> f32 {
        let warp_x = value_noise(x * 3.4, y * 3.4 + self.t * 0.021) - 0.5;
        let warp_y = value_noise(x * 3.4 + 17.3, y * 3.4 - self.t * 0.017) - 0.5;
        let (wx, wy) = (x + warp_x * 0.18, y + warp_y * 0.18);
        let field: f32 = self
            .centers
            .iter()
            .map(|&(cx, cy, r)| {
                let (dx, dy) = (wx - cx, (wy - cy) * 1.15);
                r * r / (dx * dx + dy * dy + 1e-4)
            })
            .sum();
        let body = smoothstep(0.5, 3.2, field);
        if body <= 0.0 {
            return 0.0;
        }
        let grain = 0.65 * value_noise(x * 9.0 + self.t * 0.05, y * 9.0)
            + 0.35 * value_noise(x * 23.0 - self.t * 0.08, y * 23.0 + 5.1);
        (body * (0.35 + 0.9 * grain)).clamp(0.0, 1.0)
    }

    fn cell(&self, col: u32, row: u32, x: f32, y: f32) -> Option<Cell> {
        let clearance = self.clearance(x, y);
        if clearance <= 0.0 {
            return None;
        }
        let density = self.density(x, y) * clearance;
        if density < 0.06 {
            let seed = hash(col, row, 0);
            if seed < 0.985 {
                return None;
            }
            let twinkle = 0.5 + 0.5 * (self.t * 0.7 + seed * 977.0).sin();
            return Some(Cell {
                glyph: usize::from(seed > 0.995),
                strength: twinkle * clearance,
                ink: Ink::Dust,
            });
        }
        let flicker = hash(col, row, (self.t * 0.8) as u32);
        if flicker < 0.22 * (1.0 - density) {
            return None;
        }
        let shimmer = hash(col, row, (self.t * 0.8) as u32 ^ 0x7f4a) - 0.5;
        let level = density.powf(1.25) * RAMP_LEN as f32 + shimmer * 0.9;
        if level < 0.75 {
            return None;
        }
        let glyph = (level as usize).min(RAMP_LEN - 1);
        let epoch = (self.t / SPARK_PERIOD) as u32;
        if density > 0.5 && hash(col, row, epoch ^ 0x9e37) > 0.988 {
            return Some(Cell {
                glyph,
                strength: (PI * (self.t / SPARK_PERIOD).fract()).sin(),
                ink: Ink::Spark,
            });
        }
        Some(Cell {
            glyph,
            strength: density,
            ink: Ink::Body,
        })
    }
}

/// Cell metrics for a canvas, growing the type when the window would need
/// more than [`MAX_CELLS`] cells.
#[derive(Clone, Copy, Debug, PartialEq)]
struct CellGrid {
    font_size: f32,
    cell: (f32, f32),
    cols: u32,
    rows: u32,
}

impl CellGrid {
    fn fit(width: f32, height: f32, advance_per_px: f32) -> Option<Self> {
        if !(width >= 1.0 && height >= 1.0 && advance_per_px > 0.0) {
            return None;
        }
        let base = (FONT_SIZE * advance_per_px, LINE_HEIGHT);
        let natural = (width / base.0) * (height / base.1);
        let scale = (natural / MAX_CELLS).sqrt().max(1.0);
        let cell = (base.0 * scale, base.1 * scale);
        Some(Self {
            font_size: FONT_SIZE * scale,
            cell,
            cols: (width / cell.0).ceil() as u32,
            rows: (height / cell.1).ceil() as u32,
        })
    }
}

fn calm_for(canvas: Bounds<Pixels>, composer: Option<Bounds<Pixels>>) -> Calm {
    let height = f32::from(canvas.size.height);
    let aspect = f32::from(canvas.size.width) / height;
    let Some(composer) = composer.filter(|bounds| !bounds.is_empty()) else {
        return Calm {
            center: (aspect * 0.5, 0.55),
            radius: (0.4, 0.2),
        };
    };
    let center = composer.center();
    Calm {
        center: (
            f32::from(center.x - canvas.left()) / height,
            f32::from(center.y - canvas.top()) / height,
        ),
        radius: (
            f32::from(composer.size.width) * 0.5 / height + 0.06,
            f32::from(composer.size.height) * 0.5 / height + 0.1,
        ),
    }
}

#[derive(Clone, PartialEq)]
struct Palette {
    ink: Hsla,
    spark: Hsla,
    dark: bool,
    font: SharedString,
}

impl Palette {
    fn of(theme: &Theme) -> Self {
        Self {
            ink: theme.text,
            spark: theme.accent,
            dark: theme.appearance.is_dark(),
            font: theme.font_mono.clone(),
        }
    }

    fn color(&self, cell: Cell) -> Hsla {
        let (floor, range) = if self.dark {
            (0.04, 0.17)
        } else {
            (0.04, 0.15)
        };
        match cell.ink {
            Ink::Body => self.ink.opacity(floor + range * cell.strength),
            Ink::Dust => self.ink.opacity(0.12 * cell.strength),
            Ink::Spark => self.spark.opacity(0.75 * cell.strength),
        }
    }

    fn grid(&self) -> Hsla {
        self.ink.opacity(if self.dark { 0.03 } else { 0.035 })
    }
}

/// The animated glyph field, filling its positioned parent.
pub(crate) fn backdrop(
    key: impl Into<SharedString>,
    theme: &Theme,
    composer: SurfaceBounds,
) -> impl IntoElement {
    AsciiBackdrop {
        key: key.into(),
        palette: Palette::of(theme),
        composer,
    }
}

#[derive(IntoElement)]
struct AsciiBackdrop {
    key: SharedString,
    palette: Palette,
    composer: SurfaceBounds,
}

impl RenderOnce for AsciiBackdrop {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let Self {
            key,
            palette,
            composer,
        } = self;
        let view = window.with_global_id(key.into(), |id, window| {
            window.with_element_state(id, |previous: Option<Entity<AsciiBackdropView>>, _| {
                let view = previous.unwrap_or_else(|| {
                    cx.new(|_| AsciiBackdropView {
                        palette: palette.clone(),
                        composer: composer.clone(),
                        time: 0.0,
                    })
                });
                view.update(cx, |view, cx| {
                    view.composer = composer;
                    if view.palette != palette {
                        view.palette = palette;
                        cx.notify();
                    }
                });
                (view.clone(), view)
            })
        });
        view.cached(gpui::StyleRefinement::default().size_full())
    }
}

struct AsciiBackdropView {
    palette: Palette,
    composer: SurfaceBounds,
    /// Last animated time; reduced or paused motion holds this frame.
    time: f32,
}

impl Render for AsciiBackdropView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(time) = motion::ambient_time(cx.entity_id(), cx) {
            self.time = time;
        }
        let time = self.time;
        let palette = self.palette.clone();
        let composer = self.composer.clone();
        div().size_full().child(
            canvas(
                |_, _, _| (),
                move |bounds, _, window, _| paint(bounds, time, &palette, composer.get(), window),
            )
            .size_full(),
        )
    }
}

fn paint(
    bounds: Bounds<Pixels>,
    time: f32,
    palette: &Palette,
    composer: Option<Bounds<Pixels>>,
    window: &mut Window,
) {
    let mut mono = gpui::font(palette.font.clone());
    mono.features = gpui::FontFeatures(std::sync::Arc::new(vec![
        ("liga".into(), 0),
        ("calt".into(), 0),
        ("dlig".into(), 0),
    ]));
    let text = window.text_system();
    let font_id = text.resolve_font(&mono);
    let Ok(advance) = text.em_advance(font_id, px(FONT_SIZE)) else {
        return;
    };
    let width = f32::from(bounds.size.width);
    let height = f32::from(bounds.size.height);
    let Some(grid) = CellGrid::fit(width, height, f32::from(advance) / FONT_SIZE) else {
        return;
    };
    let layout = text.layout_line(
        RAMP,
        px(grid.font_size),
        &[TextRun {
            len: RAMP.len(),
            font: mono,
            color: palette.ink,
            background_color: None,
            underline: None,
            strikethrough: None,
        }],
        None,
    );
    let mut glyphs: [Option<(FontId, GlyphId)>; RAMP_LEN] = [None; RAMP_LEN];
    for run in &layout.runs {
        for glyph in &run.glyphs {
            if let Some(slot) = glyphs.get_mut(glyph.index) {
                *slot = Some((run.font_id, glyph.id));
            }
        }
    }
    let ascent = f32::from(layout.ascent);
    let baseline = (grid.cell.1 - ascent - f32::from(layout.descent)) * 0.5 + ascent;

    let line = palette.grid();
    for col in (0..=grid.cols).step_by(GRID_EVERY.0 as usize) {
        let x = bounds.left() + px(col as f32 * grid.cell.0);
        window.paint_quad(fill(
            Bounds::new(point(x, bounds.top()), size(px(1.0), bounds.size.height)),
            line,
        ));
    }
    for row in (0..=grid.rows).step_by(GRID_EVERY.1 as usize) {
        let y = bounds.top() + px(row as f32 * grid.cell.1);
        window.paint_quad(fill(
            Bounds::new(point(bounds.left(), y), size(bounds.size.width, px(1.0))),
            line,
        ));
    }

    let field = Field::new(width / height, time, calm_for(bounds, composer));
    let font_size = px(grid.font_size);
    for row in 0..grid.rows {
        let top = row as f32 * grid.cell.1;
        let y = (top + grid.cell.1 * 0.5) / height;
        for col in 0..grid.cols {
            let left = col as f32 * grid.cell.0;
            let Some(cell) = field.cell(col, row, (left + grid.cell.0 * 0.5) / height, y) else {
                continue;
            };
            let Some((font_id, glyph_id)) = glyphs[cell.glyph] else {
                continue;
            };
            let origin = point(bounds.left() + px(left), bounds.top() + px(top + baseline));
            let _ = window.paint_glyph(origin, font_id, glyph_id, font_size, palette.color(cell));
        }
    }
}

fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn hash(x: u32, y: u32, z: u32) -> f32 {
    let mut h =
        x.wrapping_mul(0x8da6_b343) ^ y.wrapping_mul(0xd816_3841) ^ z.wrapping_mul(0xcb1a_b31f);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2c1b_3c6d);
    h ^= h >> 12;
    h = h.wrapping_mul(0x297a_2d39);
    h ^= h >> 15;
    (h >> 8) as f32 / (1u32 << 24) as f32
}

fn value_noise(x: f32, y: f32) -> f32 {
    let (fx, fy) = (x.floor(), y.floor());
    let (ix, iy) = (fx as i32 as u32, fy as i32 as u32);
    let (tx, ty) = (x - fx, y - fy);
    let (sx, sy) = (tx * tx * (3.0 - 2.0 * tx), ty * ty * (3.0 - 2.0 * ty));
    let corner = |dx: u32, dy: u32| hash(ix.wrapping_add(dx), iy.wrapping_add(dy), 0x51ed);
    let top = corner(0, 0) + (corner(1, 0) - corner(0, 0)) * sx;
    let bottom = corner(0, 1) + (corner(1, 1) - corner(0, 1)) * sx;
    top + (bottom - top) * sy
}

#[cfg(test)]
mod tests {
    use super::*;

    const ASPECT: f32 = 1.6;

    fn calm() -> Calm {
        Calm {
            center: (ASPECT * 0.5, 0.55),
            radius: (0.4, 0.15),
        }
    }

    fn cells(field: &Field, cols: u32, rows: u32) -> Vec<Option<Cell>> {
        (0..rows)
            .flat_map(|row| {
                (0..cols).map(move |col| {
                    let x = (col as f32 + 0.5) / cols as f32 * ASPECT;
                    let y = (row as f32 + 0.5) / rows as f32;
                    field.cell(col, row, x, y)
                })
            })
            .collect()
    }

    #[test]
    fn the_composer_ellipse_stays_empty() {
        for time in [0.0, 13.0, 300.0] {
            let field = Field::new(ASPECT, time, calm());
            for (dx, dy) in [(0.0, 0.0), (0.3, 0.0), (-0.3, 0.1), (0.0, -0.12)] {
                let (x, y) = (calm().center.0 + dx, calm().center.1 + dy);
                assert_eq!(field.cell(7, 9, x, y), None, "{time} {dx} {dy}");
            }
        }
    }

    #[test]
    fn the_field_is_sparse_and_draws_from_the_ramp() {
        let field = Field::new(ASPECT, 42.0, calm());
        let cells = cells(&field, 160, 54);
        let inked = cells.iter().flatten().count();
        let coverage = inked as f32 / cells.len() as f32;
        assert!((0.08..0.45).contains(&coverage), "coverage {coverage}");
        for cell in cells.iter().flatten() {
            assert!(cell.glyph < RAMP_LEN);
            assert!((0.0..=1.0).contains(&cell.strength), "{cell:?}");
        }
        assert!(
            cells
                .iter()
                .flatten()
                .any(|cell| cell.ink == Ink::Body && cell.glyph >= 6)
        );
    }

    #[test]
    fn frames_are_deterministic_and_drift_over_time() {
        let a = cells(&Field::new(ASPECT, 10.0, calm()), 120, 40);
        let b = cells(&Field::new(ASPECT, 10.0, calm()), 120, 40);
        let later = cells(&Field::new(ASPECT, 30.0, calm()), 120, 40);
        assert_eq!(a, b);
        assert_ne!(a, later);
    }

    #[test]
    fn large_windows_grow_cells_instead_of_adding_work() {
        let small = CellGrid::fit(900.0, 600.0, 0.6).unwrap();
        assert_eq!(small.font_size, FONT_SIZE);
        let huge = CellGrid::fit(3800.0, 2100.0, 0.6).unwrap();
        assert!(huge.font_size > FONT_SIZE);
        let cells = huge.cols as f32 * huge.rows as f32;
        assert!(cells <= MAX_CELLS * 1.05, "{cells}");
        assert!(CellGrid::fit(0.0, 600.0, 0.6).is_none());
        assert!(CellGrid::fit(f32::NAN, 600.0, 0.6).is_none());
    }

    #[test]
    fn the_calm_zone_follows_the_composer() {
        let canvas = Bounds::new(point(px(100.0), px(0.0)), size(px(1000.0), px(800.0)));
        let composer = Bounds::new(point(px(300.0), px(400.0)), size(px(600.0), px(120.0)));
        let calm = calm_for(canvas, Some(composer));
        assert!((calm.center.0 - 0.625).abs() < 1e-4);
        assert!((calm.center.1 - 0.575).abs() < 1e-4);
        assert!(calm.radius.0 > 600.0 * 0.5 / 800.0);
        let fallback = calm_for(canvas, None);
        assert!((fallback.center.0 - 0.625).abs() < 1e-4);
    }
}
