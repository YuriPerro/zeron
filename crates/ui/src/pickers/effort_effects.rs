//! The effort slider's effects as pure geometry and color: the Ultrathink
//! prism, the Ultracode typing, the fast mode exhaust and the color ramps the
//! ultra level names wear. The compact card and the composer chip paint
//! them; nothing here touches gpui state.

use std::f32::consts::{PI, TAU};

use gpui::Hsla;
use zeron_proto::ReasoningLevel;

use crate::motion;
use crate::roll_text::Tint;

/// The effect the selected effort level carries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum LevelEffect {
    Plain,
    Ultracode,
    Ultrathink,
}

impl LevelEffect {
    pub(super) fn of(level: ReasoningLevel) -> Self {
        match level {
            ReasoningLevel::Ultrathink => Self::Ultrathink,
            ReasoningLevel::Ultracode => Self::Ultracode,
            _ => Self::Plain,
        }
    }
}

/// The rainbow front's sweep from the rail's start to the thumb.
const ULTRATHINK_SWEEP_SECS: f32 = 0.6;
/// When the eased sweep reaches the thumb, which flashes and throws sparks.
const ULTRATHINK_ARRIVAL_SECS: f32 = 0.27;
const SPRAY_SECS: f32 = 0.55;
pub(super) const SPRAY_COUNT: usize = 12;
const RAINBOW_CYCLE_SECS: f32 = 18.0;
/// The share of the hue wheel laid across the fill.
const RAINBOW_SPREAD: f32 = 0.6;
const RAINBOW_ALPHA: f32 = 0.95;
/// The rail rainbow's OKLCH lightness and chroma: one even brightness for
/// every hue, so no color stands out as a band, at the most saturation sRGB
/// holds there.
const RAINBOW_LIGHTNESS: f32 = 0.75;
const RAINBOW_CHROMA: f32 = 0.2;
/// The cursor's run from the thumb to the rail's start.
const TYPE_SECS: f32 = 0.5;
const FLASH_RISE_SECS: f32 = 0.03;
const TYPED_DECAY_SECS: f32 = 0.25;
const THUMB_DECAY_SECS: f32 = 0.18;
/// One monospaced code cell, glyph plus gap.
pub(super) const CODE_CELL: f32 = 7.5;
/// How fast the code drifts away from the thumb at the calm clock rate.
const CODE_SPEED: f32 = 26.0;
/// How much faster the code clock runs with fast mode fully on.
const CODE_FAST_BOOST: f32 = 1.5;
const CODE_GLYPHS: &[char] = &[
    '{', '}', '<', '>', '/', ';', '=', '(', ')', '0', '1', '[', ']', '*', '+', ':', '#', '&', '|',
    '_',
];
/// How often, per second of code clock, a cell retypes its glyph.
const RETYPE_RATE: f32 = 0.6;
const CARET_HZ: f32 = 1.1;
const EXHAUST_COUNT: usize = 36;

/// Tracks the level the slider rests on and fires a burst when it settles on
/// a new effect. The first level seen (the card opening) never fires, and
/// neither does a level the thumb only passes while dragged.
pub(super) fn settle_effect(
    settled: Option<LevelEffect>,
    current: LevelEffect,
    dragging: bool,
) -> (Option<LevelEffect>, Option<LevelEffect>) {
    if dragging {
        return (settled, None);
    }
    let fire = settled
        .filter(|prev| *prev != current && current != LevelEffect::Plain)
        .map(|_| current);
    (Some(current), fire)
}

/// Seconds into a running burst, `None` once it has played out.
pub(super) fn burst_elapsed(effect: LevelEffect, elapsed: f32) -> Option<f32> {
    let run = match effect {
        LevelEffect::Plain => return None,
        LevelEffect::Ultrathink => {
            (ULTRATHINK_ARRIVAL_SECS + SPRAY_SECS).max(ULTRATHINK_SWEEP_SECS)
        }
        LevelEffect::Ultracode => TYPE_SECS + FLASH_RISE_SECS + 4.0 * THUMB_DECAY_SECS,
    };
    (elapsed < run).then_some(elapsed)
}

/// Effects only move while something is on, motion is allowed and the card
/// is open in the active window.
pub(super) fn effects_alive(
    any_effect: bool,
    reduced: bool,
    window_active: bool,
    open: bool,
) -> bool {
    any_effect && !reduced && window_active && open
}

/// An OKLCH color (`hue` in turns). Equal lightness reads as equal
/// brightness whatever the hue, unlike HSL. Where sRGB can't hold `chroma`,
/// it gives up saturation rather than lightness, so the brightness stays.
fn oklch(lightness: f32, chroma: f32, hue: f32) -> Hsla {
    let in_gamut = |rgb: [f32; 3]| rgb.iter().all(|c| (-1e-4..=1.0 + 1e-4).contains(c));
    let mut rgb = oklch_linear(lightness, chroma, hue);
    if !in_gamut(rgb) {
        let (mut low, mut high) = (0.0, chroma);
        rgb = oklch_linear(lightness, 0.0, hue);
        for _ in 0..16 {
            let mid = 0.5 * (low + high);
            let candidate = oklch_linear(lightness, mid, hue);
            if in_gamut(candidate) {
                (low, rgb) = (mid, candidate);
            } else {
                high = mid;
            }
        }
    }
    let encode = |linear: f32| {
        let linear = linear.clamp(0.0, 1.0);
        if linear <= 0.003_130_8 {
            12.92 * linear
        } else {
            1.055 * linear.powf(1.0 / 2.4) - 0.055
        }
    };
    gpui::Rgba {
        r: encode(rgb[0]),
        g: encode(rgb[1]),
        b: encode(rgb[2]),
        a: 1.0,
    }
    .into()
}

/// Linear sRGB for an OKLCH color, unclipped.
fn oklch_linear(lightness: f32, chroma: f32, hue: f32) -> [f32; 3] {
    let (a, b) = (chroma * (TAU * hue).cos(), chroma * (TAU * hue).sin());
    let l = (lightness + 0.396_337_8 * a + 0.215_803_76 * b).powi(3);
    let m = (lightness - 0.105_561_35 * a - 0.063_854_17 * b).powi(3);
    let s = (lightness - 0.089_484_18 * a - 1.291_485_5 * b).powi(3);
    [
        4.076_741_7 * l - 3.307_711_6 * m + 0.230_969_94 * s,
        -1.268_438 * l + 2.609_757_4 * m - 0.341_319_38 * s,
        -0.004_196_086 * l - 0.703_418_6 * m + 1.707_614_7 * s,
    ]
}

/// The rail rainbow's opaque color at `hue` (turns).
pub(super) fn rainbow_color(hue: f32) -> Hsla {
    oklch(RAINBOW_LIGHTNESS, RAINBOW_CHROMA, hue)
}

/// The rainbow's hue `along` the fill (0 at its start, 1 at the thumb).
fn rainbow_hue(along: f32, time: f32) -> f32 {
    (along * RAINBOW_SPREAD - time / RAINBOW_CYCLE_SECS).rem_euclid(1.0)
}

/// The rainbow at `x` over a fill `span` wide. A burst reveals it behind a
/// bright front sweeping toward the thumb; `burst` is its elapsed seconds.
pub(super) fn ultrathink_color(x: f32, span: f32, time: f32, burst: Option<f32>) -> Hsla {
    let color = rainbow_color(rainbow_hue(x / span.max(1.0), time));
    let Some(elapsed) = burst.filter(|elapsed| *elapsed < ULTRATHINK_SWEEP_SECS) else {
        return color.opacity(RAINBOW_ALPHA);
    };
    let progress = motion::EASE_OUT_QUINT.eval((elapsed / ULTRATHINK_SWEEP_SECS).clamp(0.0, 1.0));
    let front = progress * span;
    let shown = ((front - x) / 16.0).clamp(0.0, 1.0);
    let glow = (-((x - front) / 10.0).powi(2)).exp() * (1.0 - progress.powi(3));
    motion::mix(color, gpui::white(), 0.8 * glow).opacity((RAINBOW_ALPHA * shown).max(0.9 * glow))
}

/// The color the rainbow carries into the thumb, for its halo.
pub(super) fn ultrathink_thumb_color(time: f32) -> Hsla {
    rainbow_color(rainbow_hue(1.0, time))
}

/// The thumb's flash as the rainbow front arrives, 0–1.
pub(super) fn ultrathink_thumb_flash(elapsed: f32) -> f32 {
    flash(elapsed - ULTRATHINK_ARRIVAL_SECS, THUMB_DECAY_SECS)
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Spark {
    pub x: f32,
    pub y: f32,
    pub radius: f32,
    /// Its rainbow hue, in turns.
    pub hue: f32,
    pub alpha: f32,
}

/// The sparks the thumb at `(x, y)` throws back over the fill when the
/// rainbow front arrives, one hue each, fanned top to bottom.
pub(super) fn prism_spray(elapsed: f32, x: f32, y: f32) -> Vec<Spark> {
    let u = (elapsed - ULTRATHINK_ARRIVAL_SECS) / SPRAY_SECS;
    if !(0.0..1.0).contains(&u) {
        return Vec::new();
    }
    let travel = 1.0 - (1.0 - u).powi(3);
    (0..SPRAY_COUNT as u32)
        .map(|i| {
            let spread = (i as f32 + 0.5) / SPRAY_COUNT as f32 * 2.0 - 1.0;
            let angle = PI + spread * 0.9 + (hash01(i, 0, 11) - 0.5) * 0.25;
            let reach = (52.0 + 36.0 * hash01(i, 0, 12)) * travel;
            Spark {
                x: x + angle.cos() * reach,
                y: y - angle.sin() * reach * 0.5,
                radius: 2.6 - 1.4 * u,
                hue: i as f32 / SPRAY_COUNT as f32,
                alpha: (1.0 - u).powi(2),
            }
        })
        .collect()
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Glyph {
    /// The left edge of its cell.
    pub x: f32,
    pub ch: char,
    pub alpha: f32,
}

/// How fast the code clock runs: fast mode speeds the whole stream up.
pub(super) fn code_clock_rate(fast: f32) -> f32 {
    1.0 + CODE_FAST_BOOST * fast.clamp(0.0, 1.0)
}

/// The code streaming through the Ultracode fill: one lane of glyphs born at
/// the thumb (`to`) that drift toward the rail's start (`from`), gaps between
/// tokens, each cell retyping itself now and then. `fast` (0–1) packs the
/// gaps and brightens the glyphs; a burst types the code in behind the
/// cursor, `burst` being its elapsed seconds.
pub(super) fn code_glyphs(
    clock: f32,
    from: f32,
    to: f32,
    burst: Option<f32>,
    fast: f32,
) -> Vec<Glyph> {
    let cursor = burst.and_then(|elapsed| ultracode_cursor(elapsed, from, to));
    let shift = clock * CODE_SPEED;
    let gaps = 0.22 * (1.0 - 0.7 * fast);
    let mut glyphs = Vec::new();
    // Cell `k` starts `k` cells left of the thumb and slides on by `shift`.
    for k in (-shift / CODE_CELL).ceil() as i64.. {
        let x = to - CODE_CELL * (k + 1) as f32 - shift;
        if x < from {
            break;
        }
        let cell = k as u32;
        if hash01(cell, 0, 44) < gaps || cursor.is_some_and(|cursor| x < cursor) {
            continue;
        }
        let (epoch, _) = cycle(clock, 1.0 / RETYPE_RATE, hash01(cell, 0, 42));
        let pick = hash01(cell, epoch, 43);
        let ch = CODE_GLYPHS[(pick * CODE_GLYPHS.len() as f32) as usize % CODE_GLYPHS.len()];
        let edge =
            ((x - from) / 12.0).clamp(0.0, 1.0) * ((to - CODE_CELL - x) / 8.0).clamp(0.0, 1.0);
        let typed = burst.map_or(0.0, |elapsed| typed_flash(x, elapsed, from, to));
        let calm = (0.42 + 0.38 * hash01(cell, 0, 45)) * (1.0 + 0.35 * fast);
        glyphs.push(Glyph {
            x,
            ch,
            alpha: (calm.max(typed) * edge.max(typed)).min(1.0),
        });
    }
    glyphs
}

/// The block cursor's place while a burst types the code in, stepping cell
/// by cell from the thumb at `to` toward `from`; `None` once it is done.
pub(super) fn ultracode_cursor(elapsed: f32, from: f32, to: f32) -> Option<f32> {
    (elapsed < TYPE_SECS).then(|| {
        let reached = (to - from).max(0.0) * (elapsed / TYPE_SECS).max(0.0);
        to - (reached / CODE_CELL).floor() * CODE_CELL
    })
}

/// Whether a burst's cursor is still typing `elapsed` seconds in.
pub(super) fn ultracode_typing(elapsed: f32) -> bool {
    elapsed < TYPE_SECS
}

/// The glow of the cell at `x` as the cursor types it, 0–1: full the moment
/// it is typed, then fading.
fn typed_flash(x: f32, elapsed: f32, from: f32, to: f32) -> f32 {
    let typed_at = TYPE_SECS * ((to - x) / (to - from).max(1.0)).clamp(0.0, 1.0);
    let since = elapsed - typed_at;
    if since < 0.0 {
        0.0
    } else {
        (-since / TYPED_DECAY_SECS).exp()
    }
}

/// The thumb's flash once the cursor has typed the whole fill, 0–1.
pub(super) fn ultracode_thumb_flash(elapsed: f32) -> f32 {
    flash(elapsed - TYPE_SECS, THUMB_DECAY_SECS)
}

/// The caret on the thumb, blinking with crisp edges, 0–1.
pub(super) fn ultracode_caret(time: f32) -> f32 {
    (((TAU * CARET_HZ * time).cos() + 0.2) * 3.0).clamp(0.0, 1.0)
}

/// The color ramp an ultra level's name wears in the composer chip and the
/// card's title: the rainbow for Ultrathink, a cool cyan-to-violet run for
/// Ultracode. Lighter on dark themes, darker on light ones, so it reads.
pub(super) fn label_tint(effect: LevelEffect, dark: bool) -> Option<Tint> {
    let hues: &[f32] = match effect {
        LevelEffect::Plain => return None,
        LevelEffect::Ultrathink => &[0.06, 0.16, 0.3, 0.45, 0.62, 0.78, 0.9],
        LevelEffect::Ultracode => &[0.55, 0.68, 0.82],
    };
    let (lightness, chroma) = if dark { (0.78, 0.2) } else { (0.58, 0.25) };
    Some(Tint::new(
        hues.iter()
            .map(|hue| oklch(lightness, chroma, *hue))
            .collect(),
    ))
}

/// A quick rise, then an exponential fade, `since` seconds after it fired.
fn flash(since: f32, decay: f32) -> f32 {
    if since < 0.0 {
        0.0
    } else if since < FLASH_RISE_SECS {
        since / FLASH_RISE_SECS
    } else {
        (-(since - FLASH_RISE_SECS) / decay).exp()
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Mote {
    pub x: f32,
    /// 0–1 across the rail's height.
    pub lane: f32,
    pub radius: f32,
    /// How far the mote smears behind itself, as a multiple of its width.
    pub stretch: f32,
    pub alpha: f32,
}

/// Fast mode's exhaust: motes leave the thumb's back edge at `emit` and
/// drift toward the rail's start at `from`. Most die close to the thumb and
/// the few that make it far arrive faded and small.
pub(super) fn exhaust(time: f32, from: f32, emit: f32) -> Vec<Mote> {
    let room = (emit - from).max(0.0);
    (0..EXHAUST_COUNT as u32)
        .map(|i| {
            let (pass, u) = cycle(time, 0.5 + 0.7 * hash01(i, 0, 31), hash01(i, 0, 32));
            let reach = room * (0.12 + 0.88 * hash01(i, pass, 33).powi(2));
            let lane = 0.5 + 0.35 * (hash01(i, pass, 34) - 0.5);
            let drift = 0.8 * (hash01(i, pass, 35) - 0.5);
            Mote {
                x: emit - reach * exhaust_travel(u),
                lane: (lane + drift * u).clamp(0.08, 0.92),
                radius: (0.7 + 1.2 * hash01(i, pass, 36)) * (1.0 - 0.55 * u),
                stretch: 1.0 + 1.2 * (1.0 - u).powi(2),
                alpha: (u / 0.08).min(1.0) * (1.0 - u).powf(1.5),
            }
        })
        .collect()
}

/// A mote's share of its reach after `u` of its life: thrown out fast, it
/// slows as it fades.
fn exhaust_travel(u: f32) -> f32 {
    1.0 - (1.0 - u.clamp(0.0, 1.0)).powi(2)
}

/// Which pass of a repeating `life`-second cycle `time` falls in, and how
/// far into it (0–1); `phase` staggers particles sharing a life.
fn cycle(time: f32, life: f32, phase: f32) -> (u32, f32) {
    let clock = time.max(0.0) / life + phase;
    (clock.floor() as u32, clock.fract())
}

/// A stable 0–1 value per particle, pass and property.
fn hash01(a: u32, b: u32, salt: u32) -> f32 {
    let mut x =
        a.wrapping_mul(0x9E37_79B9) ^ b.wrapping_mul(0x85EB_CA6B) ^ salt.wrapping_mul(0xC2B2_AE35);
    x ^= x >> 16;
    x = x.wrapping_mul(0x7FEB_352D);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846C_A68B);
    x ^= x >> 16;
    x as f32 / u32::MAX as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hue_gap(a: f32, b: f32) -> f32 {
        let d = (a - b).abs().fract();
        d.min(1.0 - d)
    }

    /// Relative luminance: how bright the eye reads a color.
    fn luminance(color: Hsla) -> f32 {
        let rgba = gpui::Rgba::from(color);
        let linear = |c: f32| {
            if c <= 0.04045 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * linear(rgba.r) + 0.7152 * linear(rgba.g) + 0.0722 * linear(rgba.b)
    }

    #[test]
    fn levels_map_to_their_effects() {
        assert_eq!(
            LevelEffect::of(ReasoningLevel::Ultrathink),
            LevelEffect::Ultrathink
        );
        assert_eq!(
            LevelEffect::of(ReasoningLevel::Ultracode),
            LevelEffect::Ultracode
        );
        assert_eq!(LevelEffect::of(ReasoningLevel::Max), LevelEffect::Plain);
    }

    #[test]
    fn a_burst_fires_only_when_the_slider_settles_on_a_new_effect() {
        use LevelEffect::*;
        assert_eq!(
            settle_effect(None, Ultrathink, false),
            (Some(Ultrathink), None)
        );
        assert_eq!(
            settle_effect(Some(Plain), Ultrathink, true),
            (Some(Plain), None)
        );
        assert_eq!(
            settle_effect(Some(Plain), Ultrathink, false),
            (Some(Ultrathink), Some(Ultrathink))
        );
        assert_eq!(
            settle_effect(Some(Ultrathink), Ultracode, false),
            (Some(Ultracode), Some(Ultracode))
        );
        assert_eq!(
            settle_effect(Some(Ultracode), Plain, false),
            (Some(Plain), None)
        );
        assert_eq!(
            settle_effect(Some(Ultracode), Ultracode, false),
            (Some(Ultracode), None)
        );
    }

    #[test]
    fn bursts_play_out() {
        use LevelEffect::*;
        assert_eq!(burst_elapsed(Ultrathink, 0.2), Some(0.2));
        let spray_done = ULTRATHINK_ARRIVAL_SECS + SPRAY_SECS;
        assert_eq!(
            burst_elapsed(Ultrathink, spray_done - 0.01),
            Some(spray_done - 0.01)
        );
        assert_eq!(burst_elapsed(Ultrathink, spray_done + 0.01), None);
        assert!(burst_elapsed(Ultracode, TYPE_SECS + 0.1).is_some());
        assert_eq!(burst_elapsed(Ultracode, TYPE_SECS + 1.0), None);
        assert_eq!(burst_elapsed(Plain, 0.0), None);
    }

    #[test]
    fn reduced_motion_holds_every_effect_still() {
        assert!(effects_alive(true, false, true, true));
        assert!(!effects_alive(true, true, true, true));
        assert!(!effects_alive(false, false, true, true));
        assert!(!effects_alive(true, false, false, true));
        assert!(!effects_alive(true, false, true, false));
    }

    #[test]
    fn the_rainbow_is_soft_with_no_bright_or_dark_bands() {
        let span = 200.0;
        let colors: Vec<Hsla> = (0..=50)
            .map(|i| ultrathink_color(i as f32 * span / 50.0, span, 0.0, None))
            .collect();
        assert!(
            hue_gap(colors[0].h, colors[50].h) > 0.1,
            "the ends must differ"
        );
        let lums: Vec<f32> = colors.iter().map(|c| luminance(*c)).collect();
        let (low, high) = lums
            .iter()
            .fold((f32::MAX, f32::MIN), |(lo, hi), l| (lo.min(*l), hi.max(*l)));
        assert!(high - low < 0.08, "uneven brightness: {low}..{high}");
        // Vivid: every hue as saturated as the screen can show it.
        let spread = |c: &Hsla| {
            let rgba = gpui::Rgba::from(*c);
            rgba.r.max(rgba.g).max(rgba.b) - rgba.r.min(rgba.g).min(rgba.b)
        };
        let mean = colors.iter().map(spread).sum::<f32>() / colors.len() as f32;
        assert!(mean > 0.6, "washed out: {mean}");
        // Translucent hues mix with the accent under them into mud.
        assert!(
            colors[0].a > 0.9,
            "the accent tints the rainbow: {}",
            colors[0].a
        );
    }

    #[test]
    fn the_rainbow_drifts_slowly_toward_the_thumb() {
        let span = 200.0;
        // A hue reappears a little further along a moment later.
        let now = ultrathink_color(50.0, span, 1.0, None).h;
        let later: Vec<f32> = (50..90)
            .map(|x| ultrathink_color(x as f32, span, 2.0, None).h)
            .collect();
        assert!(later.iter().any(|h| hue_gap(*h, now) < 0.01));
        // Slow: a full second moves the colors well under a tenth of the wheel.
        assert!(RAINBOW_CYCLE_SECS >= 15.0);
    }

    #[test]
    fn the_ultrathink_burst_reveals_the_rainbow_toward_the_thumb() {
        let span = 200.0;
        let early = 0.25 * ULTRATHINK_SWEEP_SECS;
        assert!(ultrathink_color(5.0, span, 0.0, Some(early)).a > 0.3);
        assert!(ultrathink_color(195.0, span, 0.0, Some(early)).a < 0.05);
        let done = ultrathink_color(120.0, span, 0.0, Some(ULTRATHINK_SWEEP_SECS));
        assert_eq!(done, ultrathink_color(120.0, span, 0.0, None));
    }

    #[test]
    fn the_thumb_glows_in_the_color_the_rainbow_carries_into_it() {
        for (span, time) in [(120.0, 0.0), (240.0, 1.3), (180.0, 7.9)] {
            let arriving = ultrathink_color(span, span, time, None);
            assert_eq!(ultrathink_thumb_color(time), Hsla { a: 1.0, ..arriving });
        }
    }

    #[test]
    fn the_thumb_flashes_when_the_rainbow_front_arrives() {
        let front = motion::EASE_OUT_QUINT.eval(ULTRATHINK_ARRIVAL_SECS / ULTRATHINK_SWEEP_SECS);
        assert!(front > 0.94, "the front is still on its way: {front}");
        assert_eq!(ultrathink_thumb_flash(ULTRATHINK_ARRIVAL_SECS - 0.01), 0.0);
        assert!(ultrathink_thumb_flash(ULTRATHINK_ARRIVAL_SECS + FLASH_RISE_SECS) > 0.95);
        assert!(ultrathink_thumb_flash(ULTRATHINK_ARRIVAL_SECS + SPRAY_SECS) < 0.1);
    }

    #[test]
    fn the_prism_spray_fans_every_hue_back_from_the_thumb() {
        let (x, y) = (200.0, 18.0);
        assert!(prism_spray(ULTRATHINK_ARRIVAL_SECS - 0.01, x, y).is_empty());
        let mid = prism_spray(ULTRATHINK_ARRIVAL_SECS + 0.4 * SPRAY_SECS, x, y);
        assert_eq!(mid.len(), SPRAY_COUNT);
        assert!(
            mid.iter()
                .all(|s| s.x < x && s.alpha > 0.0 && s.radius > 0.0)
        );
        assert!(mid.iter().any(|s| s.y < y - 4.0) && mid.iter().any(|s| s.y > y + 4.0));
        let mut hues: Vec<f32> = mid.iter().map(|s| s.hue).collect();
        hues.sort_by(f32::total_cmp);
        assert!(hues.windows(2).all(|w| w[1] - w[0] > 0.05), "{hues:?}");
        assert!(prism_spray(ULTRATHINK_ARRIVAL_SECS + SPRAY_SECS, x, y).is_empty());
    }

    #[test]
    fn the_cursor_types_the_code_in_from_the_thumb_leftward() {
        let (from, to) = (14.0, 190.0);
        let early = ultracode_cursor(0.2 * TYPE_SECS, from, to).unwrap();
        let late = ultracode_cursor(0.8 * TYPE_SECS, from, to).unwrap();
        assert!(from <= late && late < early && early <= to);
        assert_eq!(
            (to - early) % CODE_CELL,
            0.0,
            "the cursor steps cell by cell"
        );
        assert_eq!(ultracode_cursor(TYPE_SECS, from, to), None);
        assert!(ultracode_typing(0.9 * TYPE_SECS) && !ultracode_typing(TYPE_SECS));
        // Nothing shows ahead of the cursor; what it just typed glows.
        let typing = code_glyphs(3.0, from, to, Some(0.5 * TYPE_SECS), 0.0);
        let cursor = ultracode_cursor(0.5 * TYPE_SECS, from, to).unwrap();
        assert!(typing.iter().all(|g| g.x >= cursor));
        let fresh = typing.iter().filter(|g| g.x < cursor + 2.0 * CODE_CELL);
        assert!(fresh.clone().count() > 0 && fresh.clone().all(|g| g.alpha > 0.7));
        assert_eq!(ultracode_thumb_flash(TYPE_SECS - 0.01), 0.0);
        assert!(ultracode_thumb_flash(TYPE_SECS + FLASH_RISE_SECS) > 0.95);
    }

    #[test]
    fn code_streams_away_from_the_thumb_and_retypes_itself() {
        let (from, to) = (14.0, 190.0);
        let at = |clock: f32| code_glyphs(clock, from, to, None, 0.0);
        let now = at(2.0);
        assert!(!now.is_empty());
        for g in &now {
            assert!(g.x >= from && g.x + CODE_CELL <= to + 0.001, "{g:?}");
            assert!(CODE_GLYPHS.contains(&g.ch) && (0.0..=1.0).contains(&g.alpha));
        }
        // One lane, a glyph per cell at most, with gaps between tokens.
        let cells = ((to - from) / CODE_CELL) as usize;
        assert!(now.len() < cells, "{} of {cells}", now.len());
        // A moment later the same glyphs sit further left.
        let dt = 0.01;
        let soon = at(2.0 + dt);
        let middle: Vec<&Glyph> = now
            .iter()
            .filter(|g| g.x > from + 20.0 && g.x < to - 20.0)
            .collect();
        let moved = middle
            .iter()
            .filter(|g| {
                soon.iter()
                    .any(|h| h.ch == g.ch && (h.x - (g.x - CODE_SPEED * dt)).abs() < 0.01)
            })
            .count();
        assert!(
            moved as f32 >= 0.9 * middle.len() as f32,
            "{moved}/{}",
            middle.len()
        );
        // Over a few seconds cells change their glyph in place.
        let text = |glyphs: &[Glyph]| glyphs.iter().map(|g| g.ch).collect::<String>();
        assert_ne!(text(&at(2.0)), text(&at(4.0)));
    }

    #[test]
    fn fast_mode_speeds_the_code_up_and_packs_it_brighter() {
        assert_eq!(code_clock_rate(0.0), 1.0);
        assert!(code_clock_rate(1.0) >= 2.0);
        let (from, to) = (14.0, 190.0);
        let (mut calm, mut fast) = (Vec::new(), Vec::new());
        for step in 0..50 {
            calm.extend(code_glyphs(step as f32 * 0.7, from, to, None, 0.0));
            fast.extend(code_glyphs(step as f32 * 0.7, from, to, None, 1.0));
        }
        let mean =
            |glyphs: &[Glyph]| glyphs.iter().map(|g| g.alpha).sum::<f32>() / glyphs.len() as f32;
        assert!(fast.len() > calm.len(), "{} vs {}", fast.len(), calm.len());
        assert!(mean(&fast) > mean(&calm) + 0.1);
    }

    #[test]
    fn the_caret_blinks() {
        let samples: Vec<f32> = (0..100).map(|i| ultracode_caret(i as f32 * 0.02)).collect();
        assert!(samples.iter().any(|a| *a > 0.99));
        assert!(samples.iter().any(|a| *a < 0.01));
        assert!(samples.iter().all(|a| (0.0..=1.0).contains(a)));
    }

    #[test]
    fn exhaust_leaves_the_thumb_toward_the_rail_start() {
        let (from, emit) = (14.0, 170.0);
        let mut moving_left = 0;
        let mut total = 0;
        for step in 0..400 {
            let time = step as f32 * 0.031;
            let now = exhaust(time, from, emit);
            let next = exhaust(time + 0.004, from, emit);
            assert_eq!(now.len(), EXHAUST_COUNT);
            for (a, b) in now.iter().zip(&next) {
                assert!(a.x >= from && a.x <= emit, "{a:?}");
                assert!((0.0..=1.0).contains(&a.lane) && (0.0..=1.0).contains(&a.alpha));
                assert!(a.radius > 0.0 && a.stretch >= 1.0);
                total += 1;
                moving_left += (b.x < a.x) as usize;
            }
        }
        assert!(
            moving_left as f32 > 0.9 * total as f32,
            "{moving_left}/{total}"
        );
    }

    #[test]
    fn exhaust_crowds_the_thumb_and_thins_out_toward_the_start() {
        let (from, emit) = (14.0, 194.0);
        let third = (emit - from) / 3.0;
        let (mut near, mut far) = (0.0, 0.0);
        for step in 0..2000 {
            for m in exhaust(step as f32 * 0.013, from, emit) {
                if m.x > emit - third {
                    near += m.alpha;
                } else if m.x < from + third {
                    far += m.alpha;
                }
            }
        }
        assert!(near > 3.0 * far, "near {near} far {far}");
        assert!(far > 0.0, "a few still make it far");
    }

    #[test]
    fn ultra_level_names_carry_a_color_ramp_that_reads_on_either_theme() {
        assert!(label_tint(LevelEffect::Plain, true).is_none());
        for effect in [LevelEffect::Ultrathink, LevelEffect::Ultracode] {
            let dark = label_tint(effect, true).unwrap();
            let light = label_tint(effect, false).unwrap();
            for i in 0..=10 {
                let t = i as f32 / 10.0;
                assert!(luminance(dark.at(t)) > 0.4, "{effect:?} dim on dark at {t}");
                assert!(
                    luminance(light.at(t)) < 0.3,
                    "{effect:?} faint on light at {t}"
                );
            }
            assert!(
                hue_gap(dark.at(0.0).h, dark.at(1.0).h) > 0.1,
                "{effect:?} has no ramp"
            );
        }
        // The rainbow name runs the whole wheel; Ultracode's stays cool.
        let rainbow = label_tint(LevelEffect::Ultrathink, true).unwrap();
        let hues: Vec<f32> = (0..=20).map(|i| rainbow.at(i as f32 / 20.0).h).collect();
        assert!(hues.iter().any(|h| hue_gap(*h, 0.0) < 0.06), "no red");
        assert!(
            hues.iter().any(|h| hue_gap(*h, 1.0 / 3.0) < 0.06),
            "no green"
        );
        assert!(
            hues.iter().any(|h| hue_gap(*h, 2.0 / 3.0) < 0.06),
            "no blue"
        );
        for dark in [true, false] {
            let rainbow = label_tint(LevelEffect::Ultrathink, dark).unwrap();
            let mean = (0..=20)
                .map(|i| {
                    let rgba = gpui::Rgba::from(rainbow.at(i as f32 / 20.0));
                    rgba.r.max(rgba.g).max(rgba.b) - rgba.r.min(rgba.g).min(rgba.b)
                })
                .sum::<f32>()
                / 21.0;
            assert!(mean > 0.55, "washed out name on dark={dark}: {mean}");
        }
    }
}
