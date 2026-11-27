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
mod tests {
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
            ("Social contact", NeedLevel::Comfortable)
        );
        assert_eq!(NeedBar::of(reading(Need::Hunger, 5)).label, "Fed");
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
}
