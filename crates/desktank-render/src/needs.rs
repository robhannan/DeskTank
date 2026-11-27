//! The Needs section of a creature's live inspect card (SPEC §2.5): one
//! labelled horizontal meter per need the species supports, in the manner of
//! the Sims Needs panel.
//!
//! Each need is a short label centred over an equal-width meter whose fill
//! is how well the need is met (empty is unmet, full is satisfied) and whose
//! colour is the satisfaction level: green comfortable, yellow middling,
//! amber low, red critically unmet. A bright cap marks the fill's end at
//! every level, so the length reads without colour. Beside each meter is a
//! reserved slot for pixel chevrons that say which way it is moving: green
//! and rightward for improving, red and leftward for declining, one to three
//! of them as the change is faster, nothing when steady. The slot is always
//! there, so labels and meters never shift as trends come and go.
//!
//! Two columns are used where the card has room and one wider column on a
//! narrow card ([`crate::inspect::InspectCard::set_narrow`]). Everything is
//! drawn at the card's own pixel scale in the shared HUD palette and font.
//!
//! The simulation keeps only each need's current satisfaction, so the
//! trend is read here from a short history of it ([`Trends`]) that exists
//! only while a creature is inspected.

use std::collections::VecDeque;

use desktank_sim::{CreatureId, NeedLevel, NeedReading};

use crate::card::{LABEL, PAD, PARA_GAP, Page};
use crate::font;
use crate::hud::{ICON_BG, PANEL};

/// The most needs a card shows; a species with more draws the first few.
pub(crate) const MAX_NEEDS: usize = 4;

/// Distinct fill lengths a meter can show: half a percent point per step,
/// so a card is rebuilt at most every other percent, never per frame.
pub(crate) const LEVELS: u8 = 50;

/// Outer width of a meter in two columns, frame included.
const METER_W: u32 = 56;
/// Outer width of a meter alone in a narrow card's single column.
const WIDE_METER_W: u32 = 112;
/// Outer height of a meter: a pixel of frame around five rows of track.
const METER_H: u32 = 7;
/// Pixels between a meter and its chevron slot.
const SLOT_GAP: u32 = 3;
/// Most chevrons a trend shows.
pub(crate) const MAX_CHEVRONS: u8 = 3;
/// A chevron's width and height, and the pixels between one chevron's left
/// edge and the next's (they nest, so the slot is narrower).
const CHEVRON_W: u32 = 3;
const CHEVRON_H: u32 = 5;
const CHEVRON_PITCH: u32 = 2;
/// Width of the chevron slot: room for the most chevrons.
const SLOT_W: u32 = CHEVRON_W + CHEVRON_PITCH * (MAX_CHEVRONS as u32 - 1);
/// Pixels between the two columns.
const COLUMN_GAP: u32 = 4;
/// Pixels between rows of needs.
const ROW_GAP: u32 = 4;

/// Satisfaction colours, from the HUD palette's hues. Their lightness
/// differs too (yellow brightest, red darkest), so no two levels sit
/// together as one grey.
const GREEN: [u8; 3] = [0x4c, 0xc0, 0x6e];
const YELLOW: [u8; 3] = [0xf0, 0xe0, 0x60];
const AMBER: [u8; 3] = [0xf2, 0x9a, 0x2e];
const RED: [u8; 3] = [0xd8, 0x40, 0x3c];
/// The unfilled track, muted against the card's background.
const TRACK: [u8; 3] = ICON_BG;
/// The meter's frame.
const METER_FRAME: [u8; 3] = PANEL;

/// Change in satisfaction points over [`TREND_WINDOW`] that counts as one,
/// two and three chevrons. Below the first the need is steady: slow hunger
/// alone moves about four points in that time.
const CHEVRON_STEPS: [u8; 3] = [8, 20, 40];
/// How far back a trend looks, in ticks (five seconds).
const TREND_WINDOW: u64 = 100;
/// Ticks between history samples.
const SAMPLE_EVERY: u64 = 5;

/// One meter as drawn: what changes any of these rebuilds the card.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct NeedBar {
    /// The label above it.
    pub(crate) label: &'static str,
    /// Fill length, 0 (unmet) to [`LEVELS`] (satisfied).
    pub(crate) fill: u8,
    /// The satisfaction level, which picks the colour.
    pub(crate) level: NeedLevel,
    /// Which way and how fast it moves: negative falls, positive improves,
    /// by up to [`MAX_CHEVRONS`] chevrons, 0 steady.
    pub(crate) trend: i8,
}

impl NeedBar {
    /// The bar for `reading`, steady until [`Trends`] says otherwise.
    pub(crate) fn of(reading: NeedReading) -> Self {
        Self {
            label: reading.need.label(),
            fill: u8::try_from(u16::from(reading.satisfaction.min(100)).div_ceil(2))
                .unwrap_or(LEVELS),
            level: reading.level(),
            trend: 0,
        }
    }
}

fn colour(level: NeedLevel) -> [u8; 3] {
    match level {
        NeedLevel::Critical => RED,
        NeedLevel::Low => AMBER,
        NeedLevel::Middling => YELLOW,
        NeedLevel::Comfortable => GREEN,
    }
}

/// `c` a share (out of 100) of the way to white.
fn lighter(c: [u8; 3], share: u32) -> [u8; 3] {
    c.map(|v| u8::try_from(u32::from(v) + (255 - u32::from(v)) * share / 100).unwrap_or(255))
}

/// `c` a share (out of 100) of the way to black.
fn darker(c: [u8; 3], share: u32) -> [u8; 3] {
    c.map(|v| u8::try_from(u32::from(v) * (100 - share) / 100).unwrap_or(0))
}

/// The columns' geometry for the page's width.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Columns {
    count: u32,
    meter: u32,
}

impl Columns {
    fn new(narrow: bool) -> Self {
        if narrow {
            Self {
                count: 1,
                meter: WIDE_METER_W,
            }
        } else {
            Self {
                count: 2,
                meter: METER_W,
            }
        }
    }

    /// Width of one column: its meter, the gap and the chevron slot.
    fn width(self) -> u32 {
        self.meter + SLOT_GAP + SLOT_W
    }

    /// Left edge of column `i`, the columns centred in the text area.
    fn left(self, i: u32) -> u32 {
        let total = self.count * self.width() + (self.count - 1) * COLUMN_GAP;
        PAD + Page::inner().saturating_sub(total) / 2 + i * (self.width() + COLUMN_GAP)
    }
}

/// Draws the Needs section: its heading and `bars` in rows. Draws nothing
/// for no bars (a species without supported needs has no section).
pub(crate) fn draw(page: &mut Page, bars: &[NeedBar], narrow: bool) {
    if bars.is_empty() {
        return;
    }
    page.section("Needs");
    let columns = Columns::new(narrow);
    let per_row = usize::try_from(columns.count).unwrap_or(1);
    let rows = bars.chunks(per_row).count();
    for (n, row) in bars.chunks(per_row).enumerate() {
        let top = page.bottom();
        for (i, bar) in (0..).zip(row) {
            draw_bar(page, bar, columns, columns.left(i), top);
        }
        let gap = if n + 1 == rows { PARA_GAP } else { ROW_GAP };
        page.skip(font::LINE + METER_H + gap);
    }
}

/// One need at column left edge `left`, its label's top at `top`.
fn draw_bar(page: &mut Page, bar: &NeedBar, columns: Columns, left: u32, top: u32) {
    let label_x = left + columns.meter.saturating_sub(font::width(bar.label)) / 2;
    page.text_at(label_x, top, bar.label, LABEL);
    let meter_top = top + font::LINE;
    draw_meter(page, bar, columns.meter, left, meter_top);
    draw_chevrons(
        page,
        bar.trend,
        left + columns.meter + SLOT_GAP,
        meter_top + (METER_H - CHEVRON_H) / 2,
    );
}

fn draw_meter(page: &mut Page, bar: &NeedBar, width: u32, left: u32, top: u32) {
    let track = width - 2;
    let fill = u32::from(bar.fill.min(LEVELS)) * track / u32::from(LEVELS);
    let base = colour(bar.level);
    for dy in 0..METER_H {
        for dx in 0..width {
            let edge = dx == 0 || dx == width - 1 || dy == 0 || dy == METER_H - 1;
            let c = if edge {
                METER_FRAME
            } else {
                let x = dx - 1;
                if x >= fill {
                    TRACK
                } else if x + 1 == fill {
                    // The cap: the fill's end, brightest, at every level.
                    lighter(base, 65)
                } else {
                    match dy {
                        1 => lighter(base, 30),
                        5 => darker(base, 30),
                        _ => base,
                    }
                }
            };
            page.put(left + dx, top + dy, c);
        }
    }
}

/// The chevrons for `trend`, the first against `left`.
fn draw_chevrons(page: &mut Page, trend: i8, left: u32, top: u32) {
    let count = u32::from(trend.unsigned_abs().min(MAX_CHEVRONS));
    if count == 0 {
        return;
    }
    let (ink, shape) = if trend > 0 {
        (GREEN, RIGHT)
    } else {
        (RED, LEFT)
    };
    for n in 0..count {
        for (dy, row) in (0..).zip(shape) {
            for (dx, c) in (0..).zip(row.bytes()) {
                if c == b'#' {
                    page.put(left + n * CHEVRON_PITCH + dx, top + dy, ink);
                }
            }
        }
    }
}

/// A rightward and a leftward chevron, `#` = ink.
const RIGHT: [&str; 5] = ["#..", ".#.", "..#", ".#.", "#.."];
const LEFT: [&str; 5] = ["..#", ".#.", "#..", ".#.", "..#"];

/// The trend of a change of `delta` satisfaction points over the window.
fn trend_of(delta: i16) -> i8 {
    let size = delta.unsigned_abs();
    let chevrons = CHEVRON_STEPS
        .iter()
        .filter(|&&step| size >= u16::from(step))
        .count();
    let chevrons = i8::try_from(chevrons).unwrap_or(0);
    if delta < 0 { -chevrons } else { chevrons }
}

/// A short history of the inspected creature's satisfactions, from which
/// each need's trend is read. Empty (and free) while nothing is inspected;
/// it starts afresh for a different creature, so a trend is never carried
/// from one fish to the next.
#[derive(Debug, Default)]
pub(crate) struct Trends {
    who: Option<CreatureId>,
    /// `(tick, satisfaction of each need)`, oldest first, within the window.
    samples: VecDeque<(u64, [u8; MAX_NEEDS])>,
}

impl Trends {
    /// Whether nothing is remembered (nothing is inspected).
    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        self.who.is_none() && self.samples.is_empty()
    }

    /// Forgets everything.
    pub(crate) fn reset(&mut self) {
        self.who = None;
        self.samples.clear();
    }

    /// Records `readings` of creature `id` at `tick` and returns each
    /// need's trend: the change since the start of the window (steady with
    /// no history yet).
    pub(crate) fn observe(
        &mut self,
        id: CreatureId,
        tick: u64,
        readings: &[NeedReading],
    ) -> [i8; MAX_NEEDS] {
        let mut now = [0; MAX_NEEDS];
        for (slot, r) in now.iter_mut().zip(readings) {
            *slot = r.satisfaction;
        }
        let stale = self
            .samples
            .back()
            .is_some_and(|&(t, _)| t > tick || tick - t > TREND_WINDOW);
        if self.who != Some(id) || stale {
            self.reset();
            self.who = Some(id);
        }
        while self
            .samples
            .front()
            .is_some_and(|&(t, _)| t + TREND_WINDOW < tick)
        {
            self.samples.pop_front();
        }
        let mut trends = [0; MAX_NEEDS];
        if let Some(&(_, then)) = self.samples.front() {
            for ((trend, &a), &b) in trends.iter_mut().zip(&now).zip(&then) {
                *trend = trend_of(i16::from(a) - i16::from(b));
            }
        }
        if self
            .samples
            .back()
            .is_none_or(|&(t, _)| tick - t >= SAMPLE_EVERY)
        {
            self.samples.push_back((tick, now));
        }
        trends
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::hud::FRAME;
    use desktank_sim::Need;

    fn reading(need: Need, satisfaction: u8) -> NeedReading {
        NeedReading { need, satisfaction }
    }

    fn bar(satisfaction: u8, trend: i8) -> NeedBar {
        NeedBar {
            trend,
            ..NeedBar::of(reading(Need::Hunger, satisfaction))
        }
    }

    #[test]
    fn a_bar_shows_the_reading_in_whole_steps() {
        let of = |s| NeedBar::of(reading(Need::Social, s));
        assert_eq!(of(0).fill, 0);
        assert_eq!(of(1).fill, 1, "a little is still visible");
        assert_eq!(of(49).fill, 25);
        assert_eq!(of(100).fill, LEVELS);
        assert_eq!(of(200).fill, LEVELS, "past full is full");
        assert_eq!(
            (of(0).label, of(100).level),
            ("Socialisation", NeedLevel::Comfortable)
        );
        assert_eq!(NeedBar::of(reading(Need::Hunger, 5)).label, "Hunger");
        assert!(of(0).fill < of(30).fill && of(30).fill < of(100).fill);
    }

    #[test]
    fn trends_grow_with_the_change_and_keep_their_direction() {
        let steps: Vec<_> = [0, 7, 8, 19, 20, 39, 40, 100]
            .into_iter()
            .map(trend_of)
            .collect();
        assert_eq!(steps, [0, 0, 1, 1, 2, 2, 3, 3]);
        assert_eq!(trend_of(-7), 0);
        assert_eq!(trend_of(-8), -1);
        assert_eq!(trend_of(-25), -2);
        assert_eq!(trend_of(-100), -3);
        assert_eq!(MAX_CHEVRONS, 3);
    }

    #[test]
    fn the_history_reads_a_trend_from_the_window_and_forgets_it() {
        let id = CreatureId(1);
        let mut trends = Trends::default();
        let at = |s| [reading(Need::Hunger, s), reading(Need::Social, 100)];
        // No history: steady, whatever the value.
        assert_eq!(trends.observe(id, 1000, &at(10))[..2], [0, 0]);
        // A bite raises it: improving, and the other need is still steady.
        assert_eq!(trends.observe(id, 1005, &at(35))[..2], [2, 0]);
        // Slow hunger alone is steady.
        let mut slow = Trends::default();
        for tick in 0..400 {
            let level = u8::try_from(100 - tick / 24).unwrap();
            let t = slow.observe(id, 2000 + tick, &at(level));
            assert_eq!(t[0], 0, "tick {tick}");
        }
        // It stays a trend for the window, then becomes steady again.
        assert_eq!(trends.observe(id, 1090, &at(35))[0], 2);
        assert_eq!(trends.observe(id, 1100, &at(35))[0], 2);
        assert_eq!(trends.observe(id, 1101, &at(35))[0], 0);
        // Falling shows as a negative trend.
        assert_eq!(trends.observe(id, 1115, &at(20))[0], -1);
        // Another creature starts with no history, as does a jump in time
        // in either direction.
        assert_eq!(trends.observe(CreatureId(2), 1116, &at(100))[..2], [0, 0]);
        assert_eq!(trends.observe(CreatureId(2), 1121, &at(0))[0], -3);
        assert_eq!(trends.observe(CreatureId(2), 5000, &at(100))[0], 0);
        assert_eq!(trends.observe(CreatureId(2), 4000, &at(0))[0], 0);
        trends.reset();
        assert_eq!(trends.observe(CreatureId(2), 4001, &at(100))[0], 0);
        // More readings than a card shows are ignored, not a panic.
        let many = [reading(Need::Hunger, 1); MAX_NEEDS + 2];
        let mut fresh = Trends::default();
        assert_eq!(fresh.observe(id, 10, &many), [0; MAX_NEEDS]);
        assert_eq!(fresh.observe(id, 20, &many), [0; MAX_NEEDS]);
    }

    #[test]
    fn the_columns_fit_the_card_and_the_labels_fit_the_meters() {
        let two = Columns::new(false);
        let one = Columns::new(true);
        let card = Page::inner();
        assert!(2 * two.width() + COLUMN_GAP <= card);
        assert!(one.width() <= card);
        assert_eq!(two.left(0), PAD);
        assert!(two.left(1) + two.width() <= PAD + card);
        assert!(one.left(0) >= PAD && one.left(0) + one.width() <= PAD + card);
        for need in [Need::Hunger, Need::Social] {
            assert!(font::width(need.label()) <= two.meter, "{}", need.label());
        }
    }

    #[test]
    fn colour_helpers_stay_in_range_and_order_by_level() {
        assert_eq!(lighter([0, 128, 255], 100), [255, 255, 255]);
        assert_eq!(lighter([10, 20, 30], 0), [10, 20, 30]);
        assert_eq!(darker([200, 100, 50], 50), [100, 50, 25]);
        let colours = [
            colour(NeedLevel::Critical),
            colour(NeedLevel::Low),
            colour(NeedLevel::Middling),
            colour(NeedLevel::Comfortable),
        ];
        for (i, a) in colours.iter().enumerate() {
            for b in &colours[i + 1..] {
                assert_ne!(a, b);
            }
        }
        // Distinct in greyscale too, for the fill and its cap.
        let grey = |c: [u8; 3]| {
            (299 * u32::from(c[0]) + 587 * u32::from(c[1]) + 114 * u32::from(c[2])) / 1000
        };
        let mut greys: Vec<_> = colours.iter().map(|&c| grey(c)).collect();
        greys.sort_unstable();
        assert!(greys.windows(2).all(|w| w[1] - w[0] >= 15), "{greys:?}");
        // The track never looks like a fill, the frame never like the track.
        assert!(colours.iter().all(|&c| grey(c) > grey(TRACK) + 60));
        assert_ne!(METER_FRAME, TRACK);
        assert_ne!(METER_FRAME, FRAME);
    }

    #[test]
    fn a_page_without_bars_draws_no_section() {
        let mut page = Page::new(None);
        let before = page.bottom();
        draw(&mut page, &[], false);
        assert_eq!(page.bottom(), before);
        draw(&mut page, &[bar(50, 0)], false);
        assert!(page.bottom() > before + font::LINE + METER_H);
    }

    // ---- Pixel tests: draw bars on a page and read the image back. ----

    /// A card with `bars` drawn in the needs section.
    fn card(bars: &[NeedBar], narrow: bool) -> crate::card::CardImage {
        let mut page = Page::new(None);
        draw(&mut page, bars, narrow);
        page.finish()
    }

    fn rgb(img: &crate::card::CardImage, x: u32, y: u32) -> [u8; 3] {
        let i = ((y * img.size.0 + x) * 4) as usize;
        [img.pixels[i], img.pixels[i + 1], img.pixels[i + 2]]
    }

    /// Where each meter is: `(left, top, width)` of every horizontal run of
    /// the meter frame colour exactly a meter wide that has track below it.
    fn meters(img: &crate::card::CardImage) -> Vec<(u32, u32, u32)> {
        let mut found = Vec::new();
        for y in 0..img.size.1 {
            let mut x = 0;
            while x < img.size.0 {
                if rgb(img, x, y) == METER_FRAME {
                    let start = x;
                    while x < img.size.0 && rgb(img, x, y) == METER_FRAME {
                        x += 1;
                    }
                    let width = x - start;
                    // A meter's top edge has background above and track below.
                    let above = y == 0 || rgb(img, start + 1, y - 1) == crate::hud::BACKGROUND;
                    let below = rgb(img, start + 1, y + 1);
                    if (width == METER_W || width == WIDE_METER_W) && above && below != METER_FRAME
                    {
                        found.push((start, y, width));
                    }
                } else {
                    x += 1;
                }
            }
        }
        found
    }

    /// The fill's length in pixels along a meter's middle track row, and
    /// whether the pixel at its end is the brightest (the cap).
    fn fill_length(img: &crate::card::CardImage, (left, top, width): (u32, u32, u32)) -> u32 {
        let y = top + METER_H / 2;
        (1..width - 1)
            .filter(|&dx| rgb(img, left + dx, y) != TRACK)
            .count()
            .try_into()
            .unwrap()
    }

    /// The pixels of a meter's chevron slot that are not background, as
    /// rows of `#` and `.`.
    fn slot(img: &crate::card::CardImage, (left, top, width): (u32, u32, u32)) -> Vec<String> {
        let x0 = left + width + SLOT_GAP;
        (0..METER_H)
            .map(|dy| {
                (0..SLOT_W)
                    .map(|dx| {
                        if rgb(img, x0 + dx, top + dy) == crate::hud::BACKGROUND {
                            '.'
                        } else {
                            '#'
                        }
                    })
                    .collect()
            })
            .collect()
    }

    fn slot_colours(
        img: &crate::card::CardImage,
        (left, top, width): (u32, u32, u32),
    ) -> Vec<[u8; 3]> {
        let x0 = left + width + SLOT_GAP;
        let mut seen = Vec::new();
        for dy in 0..METER_H {
            for dx in 0..SLOT_W {
                let c = rgb(img, x0 + dx, top + dy);
                if c != crate::hud::BACKGROUND && !seen.contains(&c) {
                    seen.push(c);
                }
            }
        }
        seen
    }

    fn luminance(c: [u8; 3]) -> u32 {
        (299 * u32::from(c[0]) + 587 * u32::from(c[1]) + 114 * u32::from(c[2])) / 1000
    }

    /// Four needs covering every satisfaction colour, a low need that is
    /// improving and a high one that is declining.
    fn four_levels() -> [NeedBar; 4] {
        [
            bar(15, 2),  // red, a short fill, improving (a first bite)
            bar(40, 1),  // amber, low but improving
            bar(60, 0),  // yellow, steady
            bar(90, -2), // green, high but declining
        ]
    }

    #[test]
    fn all_four_levels_are_drawn_in_their_colours_with_their_trends() {
        let bars = four_levels();
        let img = card(&bars, false);
        let at = meters(&img);
        assert_eq!(at.len(), 4);
        for (&place, (bar, want)) in at.iter().zip(bars.iter().zip([RED, AMBER, YELLOW, GREEN])) {
            // The fill's body is the level's colour, the track beyond it
            // muted.
            let (left, top, _) = place;
            let fill = fill_length(&img, place);
            assert_eq!(rgb(&img, left + 2, top + 3), want, "{bar:?}");
            if fill < METER_W - 2 {
                assert_eq!(rgb(&img, left + 1 + fill, top + 3), TRACK, "{bar:?}");
            }
            // The cap, the last pixel of the fill, is lighter than the body.
            assert!(
                luminance(rgb(&img, left + fill, top + 3)) > luminance(want),
                "{bar:?}"
            );
        }
        // Row-major: the first two are the first row.
        assert_eq!((at[0].1, at[1].1), (at[0].1, at[0].1));
        assert_eq!(at[2].1, at[3].1);
        assert!(at[2].1 > at[0].1);
        // The trends sit beside them: green rightward, green rightward,
        // none, red leftward ×2.
        assert_eq!(slot_colours(&img, at[0]), [GREEN]);
        assert_eq!(slot_colours(&img, at[1]), [GREEN]);
        assert_eq!(slot_colours(&img, at[2]), [] as [[u8; 3]; 0]);
        assert_eq!(slot_colours(&img, at[3]), [RED]);
        let rows = |shape: [&str; 5], n: usize| -> Vec<String> {
            let blank = ".".repeat(SLOT_W as usize);
            let mut rows = vec![blank.clone()];
            for row in shape {
                let mut line = vec![b'.'; SLOT_W as usize];
                for k in 0..n {
                    for (dx, c) in row.bytes().enumerate() {
                        if c == b'#' {
                            line[k * CHEVRON_PITCH as usize + dx] = b'#';
                        }
                    }
                }
                rows.push(String::from_utf8(line).unwrap());
            }
            rows.push(blank);
            rows
        };
        assert_eq!(slot(&img, at[0]), rows(RIGHT, 2));
        assert_eq!(slot(&img, at[1]), rows(RIGHT, 1));
        assert_eq!(slot(&img, at[3]), rows(LEFT, 2));
        // The fills order the same way the readings do.
        let fills: Vec<_> = at.iter().map(|&m| fill_length(&img, m)).collect();
        assert!(fills[0] < fills[1] && fills[1] < fills[2] && fills[2] < fills[3]);
    }

    #[test]
    fn low_partly_filled_and_full_meters_show_a_boundary_at_every_level() {
        let levels = [0_u8, 1, 30, 50, 99, 100];
        let bars: Vec<_> = levels.iter().map(|&s| bar(s, 0)).collect();
        let img = card(&bars, true);
        let at = meters(&img);
        assert_eq!(at.len(), levels.len());
        let track = WIDE_METER_W - 2;
        let mut last = None;
        for (&place, &s) in at.iter().zip(&levels) {
            let (left, top, width) = place;
            let fill = fill_length(&img, place);
            assert_eq!(width, WIDE_METER_W);
            // Proportional to the satisfaction, empty at 0 and the whole
            // track at 100.
            let expected = u32::from(NeedBar::of(reading(Need::Hunger, s)).fill) * track / 50;
            assert_eq!(fill, expected, "{s}");
            assert!(last.is_none_or(|l| l <= fill));
            last = Some(fill);
            let mid = top + 3;
            if fill == 0 {
                // Empty: only the muted track, and the frame still marks it.
                assert_eq!(rgb(&img, left + 1, mid), TRACK);
            } else {
                // The fill ends in a lighter cap, then track or the frame.
                let cap = rgb(&img, left + fill, mid);
                let body = rgb(&img, left + 1, mid);
                assert!(luminance(cap) > luminance(body), "{s}");
                let next = rgb(&img, left + fill + 1, mid);
                assert!(next == TRACK || next == METER_FRAME, "{s}");
            }
            // Top and bottom frame rows and both ends are always drawn.
            for dx in 0..width {
                assert_eq!(rgb(&img, left + dx, top), METER_FRAME);
                assert_eq!(rgb(&img, left + dx, top + METER_H - 1), METER_FRAME);
            }
            assert_eq!(rgb(&img, left, mid), METER_FRAME);
            assert_eq!(rgb(&img, left + width - 1, mid), METER_FRAME);
        }
        assert_eq!(fill_length(&img, at[5]), track);
        assert_eq!(fill_length(&img, at[0]), 0);
    }

    #[test]
    fn fill_length_and_chevron_direction_read_in_greyscale() {
        let bars = four_levels();
        let img = card(&bars, false);
        let at = meters(&img);
        let grey = |x: u32, y: u32| luminance(rgb(&img, x, y));
        let background = luminance(crate::hud::BACKGROUND);
        let track = luminance(TRACK);
        for (&(left, top, width), bar) in at.iter().zip(&bars) {
            // Fill versus track: every fill colour is brighter than the
            // track by a clear margin, so its length reads.
            let fill = (1..width - 1)
                .filter(|&dx| grey(left + dx, top + 3) > track + 40)
                .count();
            assert_eq!(
                u32::try_from(fill).unwrap(),
                fill_length(&img, (left, top, width)),
                "{bar:?}"
            );
        }
        // A chevron points right when its middle row starts further right
        // than its top row, and left when further left: read it from where
        // the ink sits, not from its colour.
        let points_right = |place: (u32, u32, u32)| {
            let x0 = place.0 + place.2 + SLOT_GAP;
            let first =
                |dy: u32| (0..SLOT_W).find(|&dx| grey(x0 + dx, place.1 + 1 + dy) > background + 60);
            first(2) > first(0)
        };
        assert!(points_right(at[0]), "improving");
        assert!(points_right(at[1]), "improving");
        assert!(!points_right(at[3]), "declining");
        // Steady shows nothing at all.
        let (left, top, width) = at[2];
        for dy in 0..METER_H {
            for dx in 0..SLOT_W {
                assert_eq!(
                    grey(left + width + SLOT_GAP + dx, top + dy),
                    background,
                    "steady slot"
                );
            }
        }
    }

    #[test]
    fn trends_never_move_labels_or_meters() {
        let steady = card(&[bar(40, 0), bar(80, 0)], false);
        let rising = card(&[bar(40, 3), bar(80, 3)], false);
        let falling = card(&[bar(40, -3), bar(80, -1)], false);
        assert_eq!(steady.size, rising.size);
        assert_eq!(steady.size, falling.size);
        let at = meters(&steady);
        assert_eq!(meters(&rising), at);
        assert_eq!(meters(&falling), at);
        // Everything outside the chevron slots is pixel for pixel the same.
        let in_slot = |x: u32, y: u32| {
            at.iter().any(|&(left, top, width)| {
                let x0 = left + width + SLOT_GAP;
                x >= x0 && x < x0 + SLOT_W && y >= top && y < top + METER_H
            })
        };
        for other in [&rising, &falling] {
            for y in 0..steady.size.1 {
                for x in 0..steady.size.0 {
                    if !in_slot(x, y) {
                        assert_eq!(rgb(&steady, x, y), rgb(other, x, y), "({x}, {y})");
                    }
                }
            }
        }
        assert_ne!(steady.pixels, rising.pixels);
    }

    #[test]
    fn labels_are_centred_over_equal_width_meters_in_either_layout() {
        let bars = [bar(30, 0), bar(80, 1), bar(55, 0)];
        let bars: Vec<_> = bars
            .iter()
            .zip(["Hunger", "Socialisation", "Hunger"])
            .map(|(b, label)| NeedBar { label, ..*b })
            .collect();
        for narrow in [false, true] {
            let img = card(&bars, narrow);
            let at = meters(&img);
            assert_eq!(at.len(), 3);
            assert!(at.iter().all(|m| m.2 == at[0].2), "equal widths");
            for (&(left, top, width), bar) in at.iter().zip(&bars) {
                // The label's ink spans the rows just above the meter.
                let ink: Vec<u32> = (left..left + width)
                    .filter(|&x| (top - font::LINE..top).any(|y| rgb(&img, x, y) == LABEL))
                    .collect();
                let (first, last) = (*ink.first().unwrap(), *ink.last().unwrap());
                assert_eq!(last - first + 1, font::width(bar.label), "{}", bar.label);
                let (a, b) = (first - left, left + width - 1 - last);
                assert!(a.abs_diff(b) <= 1, "{} off centre: {a} {b}", bar.label);
            }
        }
        // Two columns: two to a row, the second beside the first with the
        // card's text edge on the left; one column: one to a row, centred.
        let two = meters(&card(&bars, false));
        assert_eq!(two[0].1, two[1].1);
        assert_eq!(two[0].0, PAD);
        assert_eq!(two[1].0, PAD + Columns::new(false).width() + COLUMN_GAP);
        assert!(two[2].1 > two[0].1 && two[2].0 == two[0].0);
        let one = meters(&card(&bars, true));
        assert!(one[0].1 < one[1].1 && one[1].1 < one[2].1);
        assert!(one.iter().all(|m| m.0 == one[0].0));
        assert!(one[0].2 > two[0].2, "a narrow card's meters are wider");
        let width = Page::inner();
        let used = Columns::new(true).width();
        assert_eq!(one[0].0, PAD + (width - used) / 2);
    }

    /// Writes the example cards of the issue as PNGs to
    /// `target/shots/inspect-need-bars/` when `INSPECT_SHOTS` is set.
    #[test]
    fn writes_need_bar_shots_on_request() {
        if std::env::var_os("INSPECT_SHOTS").is_none() {
            return;
        }
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/shots/inspect-need-bars");
        std::fs::create_dir_all(&dir).unwrap();
        let named = |a: NeedBar, b: NeedBar| {
            [
                NeedBar {
                    label: "Hunger",
                    ..a
                },
                NeedBar {
                    label: "Socialisation",
                    ..b
                },
            ]
        };
        // Four cards, side by side: every colour, short to full fills, and
        // improving, falling and steady trends.
        let cards = [
            named(bar(25, 2), bar(100, 0)),
            named(bar(40, 1), bar(50, -3)),
            named(bar(60, 0), bar(10, 3)),
            named(bar(90, -2), bar(0, -1)),
        ];
        let images: Vec<_> = cards.iter().map(|bars| card_with_heading(bars)).collect();
        let (gap, scale) = (6, 4);
        let total_w: u32 = images.iter().map(|i| i.size.0).sum::<u32>() + gap * 3;
        let height = images.iter().map(|i| i.size.1).max().unwrap();
        let mut sheet = vec![0_u8; (total_w * height * 8 * 4) as usize];
        // Row one in colour, row two the same in greyscale.
        let row_h = height;
        for pass in 0..2_u32 {
            let mut x0 = 0;
            for image in &images {
                for y in 0..image.size.1 {
                    for x in 0..image.size.0 {
                        let i = ((y * image.size.0 + x) * 4) as usize;
                        let mut p = [image.pixels[i], image.pixels[i + 1], image.pixels[i + 2]];
                        if pass == 1 {
                            let g = u8::try_from(luminance(p)).unwrap_or(255);
                            p = [g, g, g];
                        }
                        let o = ((((pass * (row_h + gap) + y) * total_w) + x0 + x) * 4) as usize;
                        sheet[o..o + 3].copy_from_slice(&p);
                        sheet[o + 3] = 255;
                    }
                }
                x0 += image.size.0 + gap;
            }
        }
        let sheet_h = row_h * 2 + gap;
        sheet.truncate((total_w * sheet_h * 4) as usize);
        std::fs::write(
            dir.join("four-levels-and-trends.png"),
            crate::testing::png(
                &enlarge(&sheet, (total_w, sheet_h), scale),
                (total_w * scale, sheet_h * scale),
            ),
        )
        .unwrap();
    }

    /// A card with a heading and `bars`, as a creature's live page has.
    fn card_with_heading(bars: &[NeedBar]) -> crate::card::CardImage {
        let mut page = Page::new(Some("1/2"));
        page.heading("Neon tetra #7", "");
        page.field("Health:", "100%");
        draw(&mut page, bars, false);
        page.finish()
    }

    /// `rgba` enlarged `scale` times, nearest neighbour.
    pub(crate) fn enlarge(rgba: &[u8], (w, h): (u32, u32), scale: u32) -> Vec<u8> {
        let mut big = Vec::with_capacity((w * h * scale * scale * 4) as usize);
        for y in 0..h * scale {
            for x in 0..w * scale {
                let i = (((y / scale) * w + x / scale) * 4) as usize;
                big.extend_from_slice(&rgba[i..i + 4]);
            }
        }
        big
    }
}
