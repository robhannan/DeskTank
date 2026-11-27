//! The loop-agnostic tank: a world, its pacing and its renderer (SPEC §4.2).

use core::time::Duration;

use desktank_sim::{DEFAULT_TIME_RATIO, TICKS_PER_SECOND, World, daylight};
use thiserror::Error;

use crate::camera::ViewRect;
use crate::card::{self, Card, CardAnchor, CardImage, CardLayout};
use crate::grid::DirtyRects;
use crate::hud::{self, BrushPreview, Magnifier, Palette, PaletteLayout};
use crate::inspect::{InspectCard, InspectPage, Inspected, ScreenRect};
use crate::layout::Layout;
use crate::pacing::{FrameRequest, Pacer};
use crate::renderer::{Overlay, Renderer};
use crate::timer::GpuTimer;
use crate::tour::{Stage, Tour, TourCard};
use crate::{Gpu, TankSurface};

/// Frame-rate cap for optional plant sway (SPEC §2.7, T6d).
pub const SWAY_MAX_FPS: u32 = 10;

/// Targets narrower than this many pixels (three card widths: a phone held
/// upright, a small window) lay the inspect card's need meters out in one
/// column.
pub const NARROW_TARGET_W: u32 = 3 * crate::card::CARD_W;

/// Minimum interval between sway-only redraws (100 ms = 10 fps).
const SWAY_INTERVAL: Duration = Duration::from_millis(100);

const SWAY_ENABLED: u8 = 1 << 0;
const SWAY_BATTERY: u8 = 1 << 1;
const SWAY_SAVER: u8 = 1 << 2;
const SWAY_HIDDEN: u8 = 1 << 3;
const SWAY_ALLOW_BATTERY: u8 = 1 << 4;

/// Optional plant-sway setting and host-environment gates (SPEC §2.7, T6d).
#[derive(Clone, Copy, Debug, Default)]
struct Sway {
    flags: u8,
    last_frame: Option<Duration>,
}

impl Sway {
    fn enabled(self) -> bool {
        self.flags & SWAY_ENABLED != 0
    }

    fn hidden(self) -> bool {
        self.flags & SWAY_HIDDEN != 0
    }

    fn active(self) -> bool {
        self.enabled()
            && self.flags & (SWAY_SAVER | SWAY_HIDDEN) == 0
            && (self.flags & SWAY_BATTERY == 0 || self.allows_battery())
    }

    fn allows_battery(self) -> bool {
        self.flags & SWAY_ALLOW_BATTERY != 0
    }

    fn set_flag(&mut self, bit: u8, on: bool) {
        if on {
            self.flags |= bit;
        } else {
            self.flags &= !bit;
        }
    }

    fn frame_due(self, now: Duration) -> bool {
        self.last_frame.is_none_or(|t| now >= t + SWAY_INTERVAL)
    }
}

/// Tank settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TankConfig {
    /// Frame-rate cap (SPEC §6.2: 30 on AC power).
    pub max_fps: u32,
}

impl Default for TankConfig {
    fn default() -> Self {
        Self { max_fps: 30 }
    }
}

/// The screensaver camera tour (D29; see [`Tank::set_tour`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TourSettings {
    /// Seeds the tour's choices of creature, timing and glide: the same
    /// seed over the same tank and steps gives the same tour.
    pub seed: u64,
    /// Shows the inspect card of the creature in focus, read-only (the
    /// tour card, off by default in every host).
    pub card: bool,
}

/// A tour in progress.
#[derive(Debug)]
struct TourRun {
    settings: TourSettings,
    director: Tour,
    /// Whether the card now pinned is the tour's (and so the tour's to
    /// take away).
    card_pinned: bool,
}

/// World time a simulation step advances the tour by.
#[allow(clippy::cast_precision_loss)] // 20 ticks a second
const TOUR_STEP: f32 = 1.0 / TICKS_PER_SECOND as f32;

/// Error drawing a frame.
#[derive(Debug, Error)]
pub enum DrawError {
    /// The surface rejected the frame as invalid; a bug, not a transient.
    #[error("surface texture failed validation")]
    Validation,
}

/// The day/night clock that runs at the world's time ratio while the host's
/// wall-clock injection is off.
#[derive(Clone, Copy, Debug)]
struct DayClock {
    clock: daylight::DaylightClock,
    /// The clock has been seeded from a host-injected minute.
    seeded: bool,
    /// Day and night follow the injected wall clock even at a scaled ratio.
    location_sync: bool,
}

impl Default for DayClock {
    fn default() -> Self {
        Self {
            clock: daylight::DaylightClock::new(daylight::NOON),
            seeded: false,
            location_sync: false,
        }
    }
}

/// How an opaque tank's background looks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Background {
    /// Water-coloured water, sky-coloured air and a dark blue letterbox.
    Water,
    /// OLED mode: exactly black (see [`Tank::set_oled`]).
    Oled,
}

/// A simulated world plus everything needed to show it. The host calls
/// [`Tank::tick`] on its clock and [`Tank::draw`] (or [`Tank::render_to`])
/// when the returned [`FrameRequest`] asks for a frame.
#[allow(clippy::struct_excessive_bools)] // independent render toggles, not a state machine
pub struct Tank {
    world: World,
    pacer: Pacer,
    dirty: DirtyRects,
    /// A frame is wanted: the world changed or the target was invalidated.
    needs_frame: bool,
    renderer: Option<Renderer>,
    timer: Option<GpuTimer>,
    gpu_time: Option<Duration>,
    /// The interact palette, if shown.
    palette: Option<Palette>,
    /// The palette image needs uploading.
    palette_dirty: bool,
    /// The species card, if shown.
    card: Option<Card>,
    /// The card's image, built when it was set (`None` while no card is
    /// shown or its species is unknown).
    card_image: Option<CardImage>,
    /// The card image needs uploading.
    card_dirty: bool,
    /// The edge of the canvas a palette outside it is on (the card sits
    /// nearest it when there is no palette in the canvas).
    card_anchor: CardAnchor,
    /// The host's minimum card scale (see [`Tank::set_card_min_scale`]).
    card_min_scale: u32,
    /// The in-tank inspect card.
    inspect: InspectCard,
    /// The brush outline, if shown.
    cursor: Option<BrushPreview>,
    /// The device placement ghost and whether it fits (D27).
    ghost: Option<(desktank_sim::Device, bool)>,
    /// The magnified lens around the pointer, if shown.
    magnifier: Option<Magnifier>,
    /// The part of the grid the target shows, if not the whole tank at its
    /// integer layout (see [`Tank::set_camera`]).
    camera: Option<ViewRect>,
    /// The screensaver camera tour, if on (see [`Tank::set_tour`]).
    tour: Option<TourRun>,
    /// The size in pixels of the target the tank is shown in (zero until
    /// the host says, see [`Tank::set_target_size`]).
    target_size: (u32, u32),
    /// The field tinted over the tank (debug overlay), if any.
    field_overlay: Option<crate::FieldOverlay>,
    /// The centimetre ruler is shown (see [`Tank::set_ruler`]).
    ruler: bool,
    /// Field version last drawn with the overlay.
    fields_drawn: u64,
    overlay_water_alpha: Option<f32>,
    /// What an opaque tank's water, air and letterbox are drawn as.
    background: Background,
    /// Current-rebuild counter last drawn (for surface-current ripples).
    flow_drawn: u64,
    /// Local time of day, minutes after midnight (day and night).
    time_of_day: u16,
    /// The day's lighting and rest times (the fixed schedule unless the host
    /// follows the sun, see [`Tank::set_daylight_schedule`]).
    daylight_schedule: daylight::DaylightSchedule,
    /// The scaled day/night clock (see [`Tank::update_local_time`]).
    day_clock: DayClock,
    /// Creatures version last drawn (`None` before the first frame).
    creatures_drawn: Option<u64>,
    /// Plants version last drawn (`None` before the first frame).
    plants_drawn: Option<u64>,
    /// Algae version last drawn (`None` before the first frame).
    algae_drawn: Option<u64>,
    /// Optional render-only plant sway state (SPEC §2.7, T6d); off by default.
    sway: Sway,
}

impl Tank {
    /// A tank showing `world`.
    #[must_use]
    pub fn new(world: World, config: TankConfig) -> Self {
        let dirty = DirtyRects::new(world.width(), world.height());
        Self {
            world,
            pacer: Pacer::new(TICKS_PER_SECOND, config.max_fps),
            dirty,
            needs_frame: true,
            renderer: None,
            timer: None,
            gpu_time: None,
            palette: None,
            palette_dirty: false,
            card: None,
            card_image: None,
            card_dirty: false,
            card_anchor: CardAnchor::default(),
            card_min_scale: 0,
            ruler: false,
            inspect: InspectCard::default(),
            cursor: None,
            ghost: None,
            magnifier: None,
            camera: None,
            tour: None,
            target_size: (0, 0),
            field_overlay: None,
            fields_drawn: 0,
            overlay_water_alpha: None,
            background: Background::Water,
            flow_drawn: 0,
            time_of_day: daylight::NOON,
            daylight_schedule: daylight::DaylightSchedule::FIXED,
            day_clock: DayClock::default(),
            creatures_drawn: None,
            plants_drawn: None,
            algae_drawn: None,
            sway: Sway::default(),
        }
    }

    /// Turns optional render-only plant sway on or off (SPEC §2.7, T6d).
    /// Off by default. While active, sway redraws a settled planted tank at
    /// most [`SWAY_MAX_FPS`] (10 fps). Battery power requires a separate
    /// opt-in. Screensaver mode and hidden surfaces always disable sway.
    pub fn set_sway(&mut self, enabled: bool) {
        self.set_sway_flag(SWAY_ENABLED, enabled);
    }

    /// Whether the user has enabled plant sway.
    #[must_use]
    pub fn sway(&self) -> bool {
        self.sway.enabled()
    }

    /// Allows enabled sway to run on battery power. Disallowed by default.
    pub fn set_allow_sway_on_battery(&mut self, allow: bool) {
        self.set_sway_flag(SWAY_ALLOW_BATTERY, allow);
    }

    /// Whether the user allows sway on battery power.
    #[must_use]
    pub fn allows_sway_on_battery(&self) -> bool {
        self.sway.allows_battery()
    }

    /// Tells the tank whether the host is on battery power.
    pub fn set_on_battery(&mut self, on_battery: bool) {
        self.set_sway_flag(SWAY_BATTERY, on_battery);
    }

    /// Tells the tank whether the host is a screensaver (disables sway).
    pub fn set_saver_mode(&mut self, saver_mode: bool) {
        self.set_sway_flag(SWAY_SAVER, saver_mode);
    }

    /// Tells the tank whether the host surface is hidden (disables sway
    /// without requesting a frame).
    pub fn set_hidden(&mut self, hidden: bool) {
        let was = self.sway.hidden();
        self.sway.set_flag(SWAY_HIDDEN, hidden);
        if hidden {
            self.sway.last_frame = None;
        } else if was && (self.sway.enabled() || self.tour.is_some()) {
            // The last frame may be swayed and sway may have been turned
            // off (battery, saver) while hidden, and the tour stood still:
            // redraw once on return.
            self.needs_frame = true;
        }
    }

    fn set_sway_flag(&mut self, bit: u8, on: bool) {
        let was = self.sway_active();
        self.sway.set_flag(bit, on);
        if self.sway_active() != was && !self.sway.hidden() {
            self.needs_frame = true;
            if !self.sway_active() {
                self.sway.last_frame = None;
            }
        }
    }

    /// Whether plant sway is enabled and allowed by power, visibility and
    /// screensaver settings, and the tank has at least one plant.
    #[must_use]
    pub fn sway_active(&self) -> bool {
        self.sway.active() && self.world.plant_count() > 0
    }

    /// Current sway phase in seconds of world time (deterministic for
    /// capture), wrapped at 20π s, the common period of both waves in
    /// `sway_dx` (`tank.wgsl`), so f32 stays precise and the wrap is seamless.
    fn sway_phase(&self) -> f32 {
        #[allow(clippy::cast_precision_loss)] // exact below 2^53 ticks
        let seconds = self.world.tick_count() as f64 / f64::from(TICKS_PER_SECOND);
        #[allow(clippy::cast_possible_truncation)] // < 63, fits f32
        let phase = seconds.rem_euclid(20.0 * std::f64::consts::PI) as f32;
        phase
    }

    /// Pins the time of day (minutes after midnight) the tank is lit for
    /// (`--time`, `capture`): the scaled clock restarts from `minute` and is
    /// no longer seeded by [`Tank::update_local_time`] until the next call.
    /// Asks for a frame only if the tint changes. Default: noon. The world
    /// gets it too: creatures rest at night ([`World::set_time_of_day`]).
    pub fn set_time_of_day(&mut self, minute: u16) {
        self.day_clock.clock.set_minute(minute);
        self.day_clock.seeded = false;
        self.apply_minute(minute);
    }

    /// Injects the host's local wall-clock minute (SPEC §2.6: day and night
    /// follow local time). At the default time ratio, or with location sync
    /// on, the minute is applied directly. Otherwise the first call seeds the
    /// scaled clock and later calls leave it running, so a faster or slower
    /// day isn't snapped back to wall-clock time. Asks for a frame only if
    /// the tint changes, so a host can call this every minute.
    pub fn update_local_time(&mut self, minute: u16) {
        let minute = minute % daylight::MINUTES_PER_DAY;
        if self.day_clock.location_sync || !self.clock_scaled() {
            self.day_clock.clock.set_minute(minute);
            self.day_clock.seeded = false;
            self.apply_minute(minute);
        } else if !self.day_clock.seeded {
            self.day_clock.clock.set_minute(minute);
            self.day_clock.seeded = true;
            self.apply_minute(minute);
        }
    }

    /// Makes day and night follow the injected wall clock even at a scaled
    /// time ratio. Enabling it makes the next [`Tank::update_local_time`]
    /// snap to wall-clock time.
    pub fn set_location_sync(&mut self, enabled: bool) {
        if enabled && !self.day_clock.location_sync {
            self.day_clock.seeded = false;
        }
        self.day_clock.location_sync = enabled;
    }

    /// Sets the day's lighting and rest times, such as the sun's sunrise and
    /// sunset at the user's location
    /// ([`daylight::DaylightSchedule::solar_schedule`]). The world gets it
    /// too, so creatures wake and settle with the sun. Asks for a frame only
    /// if the tint at the current time of day changes, so a host can call
    /// this every minute. Default: [`daylight::DaylightSchedule::FIXED`].
    pub fn set_daylight_schedule(&mut self, schedule: daylight::DaylightSchedule) {
        let bits = |s| daylight::tint_with_schedule(self.time_of_day, s).map(f32::to_bits);
        if bits(schedule) != bits(self.daylight_schedule) {
            self.needs_frame = true;
        }
        self.world.set_daylight_schedule(schedule);
        self.daylight_schedule = schedule;
    }

    /// The day's lighting and rest times.
    #[must_use]
    pub fn daylight_schedule(&self) -> daylight::DaylightSchedule {
        self.daylight_schedule
    }

    /// Whether day and night follow the injected wall clock.
    #[must_use]
    pub fn location_sync(&self) -> bool {
        self.day_clock.location_sync
    }

    /// Whether the scaled clock drives the day: seeded, not location-synced
    /// and a time ratio other than the default.
    fn clock_running(&self) -> bool {
        self.day_clock.seeded && !self.day_clock.location_sync && self.clock_scaled()
    }

    fn clock_scaled(&self) -> bool {
        (self.world.time_ratio() - DEFAULT_TIME_RATIO).abs() >= f32::EPSILON
    }

    /// Applies `minute` to the world and the tint, asking for a frame only
    /// if the tint changed.
    fn apply_minute(&mut self, minute: u16) {
        let minute = minute % daylight::MINUTES_PER_DAY;
        self.world.set_time_of_day(minute);
        let schedule = self.daylight_schedule;
        let bits = |m: u16| daylight::tint_with_schedule(m, schedule).map(f32::to_bits);
        if bits(minute) != bits(self.time_of_day) {
            self.needs_frame = true;
        }
        self.time_of_day = minute;
    }

    /// Catches the world up by `seconds` ([`World::catch_up`]) and, when the
    /// scaled clock is running, the day with it.
    pub fn catch_up(&mut self, seconds: u32) -> desktank_sim::CatchUp {
        let report = self.world.catch_up(seconds);
        if self.clock_running() {
            let minute = self
                .day_clock
                .clock
                .advance_seconds(report.seconds, self.world.time_ratio());
            self.apply_minute(minute);
        }
        report
    }

    /// The local time of day the tank is lit for, minutes after midnight.
    #[must_use]
    pub fn time_of_day(&self) -> u16 {
        self.time_of_day
    }

    /// Tints the tank by a field's value (a debug overlay), or with `None`
    /// turns the tint off. While on, a field change asks for a frame.
    pub fn set_field_overlay(&mut self, field: Option<crate::FieldOverlay>) {
        if self.field_overlay != field {
            self.field_overlay = field;
            self.needs_frame = true;
        }
    }

    /// The field overlay shown, if any.
    #[must_use]
    pub fn field_overlay(&self) -> Option<crate::FieldOverlay> {
        self.field_overlay
    }

    /// Shows or hides the centimetre ruler (off by default): pixel-art ticks
    /// and labels along the tank's bottom and left edges, 0 cm at the
    /// bottom-left corner, at the scale creatures are drawn at (see
    /// `desktank_sim::species::CELLS_PER_CM`). Labels grow with
    /// [`Tank::set_card_min_scale`] on high-density displays. Off, it costs
    /// no draw and no memory; on, it adds one blended pass over the two edge
    /// strips and never asks for a frame by itself. A change asks for a frame;
    /// setting the same value again costs nothing.
    pub fn set_ruler(&mut self, enabled: bool) {
        if self.ruler != enabled {
            self.ruler = enabled;
            self.needs_frame = true;
        }
    }

    /// Whether the ruler is shown (see [`Tank::set_ruler`]).
    #[must_use]
    pub fn ruler(&self) -> bool {
        self.ruler
    }

    /// Shows (or with `None`, hides) the interact palette. A change asks
    /// for a frame; setting the same palette again costs nothing.
    pub fn set_palette(&mut self, palette: Option<Palette>) {
        if self.palette != palette {
            self.palette = palette;
            self.palette_dirty = true;
            self.needs_frame = true;
        }
    }

    /// Shows (or with `None`, hides) the species card: a creature's or a
    /// plant's data, drawn below the palette (or top-left without one) on
    /// top of the tank. A change builds the card image once and asks for a
    /// frame; setting the same card again costs nothing, and nothing is
    /// built or drawn while it is `None`.
    pub fn set_card(&mut self, card: Option<Card>) {
        if self.card != card {
            self.card = card;
            self.card_image = card.and_then(|c| {
                card::build(
                    c,
                    self.world.materials(),
                    self.world.species(),
                    self.world.plant_species(),
                )
            });
            self.card_dirty = true;
            self.needs_frame = true;
        }
    }

    /// Tells the tank which edge of the canvas the host's palette is on when
    /// it is not drawn in the canvas (the web's DOM palette is below it):
    /// the species card then sits on that side, bottom-left for
    /// [`CardAnchor::Bottom`], rather than top-left (the default). With the
    /// palette in the canvas the card stays directly below it. A change
    /// asks for a frame; setting the same anchor again costs nothing.
    pub fn set_card_anchor(&mut self, anchor: CardAnchor) {
        if self.card_anchor != anchor {
            self.card_anchor = anchor;
            self.needs_frame = true;
        }
    }

    /// The card anchor set.
    #[must_use]
    pub fn card_anchor(&self) -> CardAnchor {
        self.card_anchor
    }

    /// Sets the smallest scale the species and inspect cards are drawn at,
    /// where they fit (see [`crate::card_scale`]). Card scales count target
    /// pixels, so on a high-density display (a phone at 3 device pixels per
    /// CSS pixel) the default 2 is too small to read: the web host passes
    /// its device-pixel ratio, rounded up. 0 (the default) changes nothing.
    /// A change asks for a frame; setting the same value again costs
    /// nothing.
    pub fn set_card_min_scale(&mut self, scale: u32) {
        if self.card_min_scale != scale {
            self.card_min_scale = scale;
            self.needs_frame = true;
        }
    }

    /// The minimum card scale set.
    #[must_use]
    pub fn card_min_scale(&self) -> u32 {
        self.card_min_scale
    }

    /// The species card set, if any.
    #[must_use]
    pub fn card(&self) -> Option<Card> {
        self.card
    }

    /// Where the card is drawn in a `target`-pixel surface, for hit tests
    /// (hosts swallow clicks on it); `None` if no card is shown, its species
    /// is unknown or it doesn't fit the target even at scale 1.
    #[must_use]
    pub fn card_layout(&self, target: (u32, u32)) -> Option<CardLayout> {
        let image = self.card_image.as_ref()?;
        CardLayout::fit(
            target,
            &self.layout(target),
            self.palette_layout(target).as_ref(),
            self.card_anchor,
            self.card_min_scale,
            image.size,
        )
    }

    /// Pins (or with `None`, removes) the inspect card to a creature, plant,
    /// device or cell (SPEC §2.5): drawn beside it, following it, on its
    /// live page. It needs no input to show, so any host can draw it. The
    /// card goes by itself when its creature, plant or device does. A change builds the image
    /// once and asks for a frame; setting the same thing again costs
    /// nothing (and keeps the page), and nothing is built or drawn while it
    /// is `None`.
    pub fn set_inspected(&mut self, inspected: Option<Inspected>) {
        self.needs_frame |= self.inspect.set(&self.world, inspected);
    }

    /// What the inspect card is pinned to, if anything (`None` once it has
    /// gone, even before the next tick or frame drops the card).
    #[must_use]
    pub fn inspected(&self) -> Option<Inspected> {
        self.inspect
            .what
            .filter(|&what| crate::inspect::anchor(&self.world, what).is_some())
    }

    /// Shows `page` of the inspect card (if one is shown). A change
    /// rebuilds the image once and asks for a frame.
    pub fn set_inspect_page(&mut self, page: InspectPage) {
        self.needs_frame |= self.inspect.set_page(&self.world, page);
    }

    /// The inspect card's page.
    #[must_use]
    pub fn inspect_page(&self) -> InspectPage {
        self.inspect.page
    }

    /// Where the inspect card is drawn in a `target`-pixel surface, for hit
    /// tests (a click on it turns the page); `None` if none is shown or it
    /// doesn't fit the target even at scale 1.
    #[must_use]
    pub fn inspect_layout(&self, target: (u32, u32)) -> Option<CardLayout> {
        self.inspect.layout(
            &self.world,
            target,
            &self.layout(target),
            self.camera.as_ref(),
            self.palette_layout(target).as_ref(),
            self.card_min_scale,
        )
    }

    /// Lets the viewer name creatures (the web's owner; every desktop
    /// window): an unnamed creature's inspect card then offers the prompt
    /// and its Name row can be clicked. Off by default, as a viewer's card
    /// only shows a name that is there. A change rebuilds the image once.
    pub fn set_inspect_naming(&mut self, allowed: bool) {
        self.needs_frame |= self.inspect.set_can_name(&self.world, allowed);
    }

    /// Edits the inspected creature's name in the card: the Name row shows
    /// `edit` (cut to [`desktank_sim::MAX_NAME_CHARS`] characters, control
    /// characters dropped) with a caret at its end and a hint, on the live
    /// page; `None` goes back to the name. The host owns the text and keys
    /// (and naming the creature, [`desktank_sim::World::set_creature_name`]);
    /// this only draws it. Ignored unless a creature is inspected and
    /// [`Tank::set_inspect_naming`] allows it. A change rebuilds the image
    /// once and asks for a frame, so call it when the text changes, not per
    /// frame; the caret is steady, so a blink never asks for frames.
    pub fn set_inspect_name_edit(&mut self, edit: Option<&str>) {
        self.needs_frame |= self.inspect.set_name_edit(&self.world, edit);
    }

    /// The name being edited in the inspect card (as drawn), if any.
    #[must_use]
    pub fn inspect_name_edit(&self) -> Option<&str> {
        self.inspect.name_edit()
    }

    /// Where the Name row of the inspect card is drawn in a `target`-pixel
    /// surface, for hit tests (a click on it starts editing the name, not
    /// turning the page); `None` unless naming is allowed and a creature's
    /// live page is shown.
    #[must_use]
    pub fn inspect_name_rect(&self, target: (u32, u32)) -> Option<ScreenRect> {
        self.inspect.name_rect(&self.inspect_layout(target)?)
    }

    /// Brings the inspect card up to date with the world (see
    /// [`InspectCard::refresh`]).
    fn refresh_inspect(&mut self) {
        if self.inspect.what.is_some() {
            self.needs_frame |= self.inspect.refresh(&self.world, false);
        }
    }

    /// Lays the inspect card out for a `size`-pixel target: a narrow target
    /// (see [`NARROW_TARGET_W`]) gets one column of need meters, the
    /// rest two. A change rebuilds the card for the frame being drawn.
    fn fit_inspect(&mut self, size: (u32, u32)) {
        self.needs_frame |= self
            .inspect
            .set_narrow(&self.world, size.0 < NARROW_TARGET_W);
    }

    /// Shows (or hides) the brush outline. A change asks for a frame.
    pub fn set_cursor(&mut self, cursor: Option<BrushPreview>) {
        if self.cursor != cursor {
            self.cursor = cursor;
            self.needs_frame = true;
        }
    }

    /// Shows (or hides) the ghost of the device the place tool would put
    /// down, and whether it fits there (D27; from
    /// [`desktank_sim::tools::Tools::ghost`]). Setting the same ghost again
    /// asks for no extra frame.
    pub fn set_device_ghost(&mut self, ghost: Option<(desktank_sim::Device, bool)>) {
        if self.ghost != ghost {
            self.ghost = ghost;
            self.needs_frame = true;
        }
    }

    /// Shows (or hides) the magnified lens around the pointer. Setting the
    /// same magnifier again costs nothing and asks for no extra frame.
    pub fn set_magnifier(&mut self, magnifier: Option<Magnifier>) {
        if self.magnifier != magnifier {
            self.magnifier = magnifier;
            self.needs_frame = true;
        }
    }

    /// The magnified lens currently shown, if any.
    #[must_use]
    pub fn magnifier(&self) -> Option<Magnifier> {
        self.magnifier
    }

    /// Where the palette is drawn in a `target`-pixel surface, for hit
    /// tests; `None` if no palette is shown.
    #[must_use]
    pub fn palette_layout(&self, target: (u32, u32)) -> Option<PaletteLayout> {
        let p = self.palette.as_ref()?;
        Some(PaletteLayout::fit(
            target,
            &self.layout(target),
            p.tabs.len(),
            p.subs.len(),
            p.items.len(),
            p.morphs.map_or(0, |m| m.count + usize::from(m.cycle)),
        ))
    }

    /// Sets overlay water opacity and bottom-aligns the grid when given a value.
    /// The opacity is clamped to 0–1, with NaN treated as zero. A value of
    /// `None` restores the opaque, centred tank.
    pub fn set_overlay_water_alpha(&mut self, alpha: Option<f32>) {
        let alpha = alpha.map(|a| if a.is_nan() { 0.0 } else { a.clamp(0.0, 1.0) });
        if self.overlay_water_alpha != alpha {
            self.overlay_water_alpha = alpha;
            self.invalidate();
        }
    }

    /// Turns OLED mode on or off (off by default). On, every background pixel
    /// of an opaque tank (water, air, the letterbox) is exactly black and the
    /// depth haze fades to black, so only creatures, plants, ground, devices,
    /// bubbles and ripples light pixels. It has no effect on the translucent
    /// click-through overlay. Only the next frame changes: no pipeline is
    /// rebuilt.
    pub fn set_oled(&mut self, enabled: bool) {
        let background = if enabled {
            Background::Oled
        } else {
            Background::Water
        };
        if self.background != background {
            self.background = background;
            self.invalidate();
        }
    }

    /// Whether OLED mode is on (see [`Tank::set_oled`]).
    #[must_use]
    pub fn oled(&self) -> bool {
        self.background == Background::Oled
    }

    /// The current overlay water opacity, if overlay rendering is enabled.
    #[must_use]
    pub fn overlay_water_alpha(&self) -> Option<f32> {
        self.overlay_water_alpha
    }

    /// Where the grid is drawn in a `target`-pixel surface.
    #[must_use]
    pub fn layout(&self, target: (u32, u32)) -> Layout {
        let side = |n: usize| u32::try_from(n).unwrap_or(u32::MAX);
        let grid = (side(self.world.width()), side(self.world.height()));
        if self.overlay_water_alpha.is_some() {
            Layout::fit_bottom(grid, target)
        } else {
            Layout::fit(grid, target)
        }
    }

    /// Points the camera at `view` (cells; see [`ViewRect`]), or with
    /// `None` takes it away. With a camera the tank draws offscreen at the
    /// integer [`Tank::layout`] it would have without one and a post pass
    /// resamples `view` of it over the whole target with a sharp-bilinear
    /// filter (square texels, no shimmer at a fractional zoom). Cards, the
    /// palette and the brush outline draw over that in screen space at
    /// their usual scale, and the inspect card stays pinned beside its
    /// creature through the camera. With `None` nothing changes from the
    /// plain tank: no offscreen pass, the same pixels.
    ///
    /// The view's aspect should be the target's (see
    /// [`Tank::camera_fit`] and [`Tank::camera_cover`]). A view that isn't
    /// finite with a positive size counts as `None`. A change asks for a
    /// frame; setting the same view again costs nothing. The offscreen image
    /// is made at the first frame with a camera, remade on resize and freed
    /// when the camera goes. While a camera is set, every frame redraws the
    /// whole target.
    pub fn set_camera(&mut self, view: Option<ViewRect>) {
        let view = view.filter(|v| v.is_valid());
        if self.camera != view {
            self.camera = view;
            if view.is_none()
                && let Some(r) = self.renderer.as_mut()
            {
                r.release_camera();
            }
            self.needs_frame = true;
        }
    }

    /// The camera's view, if one is set.
    #[must_use]
    pub fn camera(&self) -> Option<ViewRect> {
        self.camera
    }

    /// The view of the whole tank in a `target`-pixel surface, letterboxed
    /// as [`Tank::layout`] places it: a camera set to it draws what no
    /// camera does.
    #[must_use]
    pub fn camera_fit(&self, target: (u32, u32)) -> ViewRect {
        ViewRect::fit(&self.layout(target), target)
    }

    /// The view of `centre` (grid cells) at `zoom` times
    /// [`Tank::camera_fit`], clamped inside the tank: zoom 1 is the fit
    /// view and a larger zoom covers the target, so no letterbox shows (see
    /// [`ViewRect::cover`]).
    #[must_use]
    pub fn camera_cover(&self, target: (u32, u32), centre: (f32, f32), zoom: f32) -> ViewRect {
        ViewRect::cover(&self.layout(target), target, centre, zoom)
    }

    /// The smallest zoom at which [`Tank::camera_cover`] shows no letterbox
    /// in `target` (1 where the tank fills it).
    #[must_use]
    pub fn camera_min_cover_zoom(&self, target: (u32, u32)) -> f32 {
        ViewRect::min_cover_zoom(&self.layout(target), target)
    }

    /// The view `target` shows: the camera's, or the whole tank at its
    /// layout.
    #[must_use]
    pub fn view(&self, target: (u32, u32)) -> ViewRect {
        self.camera.unwrap_or_else(|| self.camera_fit(target))
    }

    /// Where grid point `cell` (cells, fractional) is in a `target`-pixel
    /// surface, under the camera if one is set.
    #[must_use]
    pub fn grid_to_screen(&self, target: (u32, u32), cell: (f64, f64)) -> (f64, f64) {
        self.view(target).grid_to_screen(target, cell)
    }

    /// The grid point under `target` pixel `px`, under the camera if one is
    /// set (floor it for the cell, as [`Layout::cell_at`] does).
    #[must_use]
    pub fn screen_to_grid(&self, target: (u32, u32), px: (f64, f64)) -> (f64, f64) {
        self.view(target).screen_to_grid(target, px)
    }

    /// Turns the screensaver camera tour on, with `settings`, or with `None`
    /// off (D29). The tour drives the camera (see [`Tank::set_camera`], which
    /// a host leaves alone meanwhile) over world time: it focuses on a live
    /// creature at 2–3× for 20–40 s, following it, glides to another over
    /// 6–10 s, and every 3–5 focuses pulls back to the whole tank for about
    /// 20 s. With no creatures the view drifts slowly round the tank. The
    /// director is deterministic: `settings.seed` and the world fix every
    /// view, and it reads no clock.
    ///
    /// It runs on [`Tank::tick`] (or [`Tank::step`]) and needs the target's
    /// size: call [`Tank::set_target_size`] first (a draw tells it too).
    /// While the tour moves the view the tank asks for frames, at the frame
    /// cap and no faster; while hidden ([`Tank::set_hidden`]) it asks for
    /// none and the tour doesn't advance. Changing only `card` keeps the
    /// tour going; a new seed starts it again.
    ///
    /// With `settings.card`, once a glide has settled the inspect card is
    /// pinned to the creature in focus, turning from its live page to its
    /// species page and back every 12 s, and removed when the next glide or
    /// pull-back starts. It is read-only: it never shows a name prompt or
    /// takes a name edit ([`Tank::set_inspect_naming`] and
    /// [`Tank::set_inspect_name_edit`] change nothing), and shows a name
    /// only a creature already has.
    ///
    /// `None` takes the camera away, and the tour's card with it (a card
    /// pinned some other way stays).
    pub fn set_tour(&mut self, settings: Option<TourSettings>) {
        let Some(settings) = settings else {
            if let Some(run) = self.tour.take() {
                if run.card_pinned {
                    self.set_inspected(None);
                }
                self.needs_frame |= self.inspect.set_read_only(&self.world, false);
                self.set_camera(None);
            }
            return;
        };
        match &mut self.tour {
            Some(run) if run.settings.seed == settings.seed => run.settings = settings,
            _ => {
                let card_pinned = self.tour.as_ref().is_some_and(|run| run.card_pinned);
                self.tour = Some(TourRun {
                    settings,
                    director: Tour::new(settings.seed),
                    card_pinned,
                });
            }
        }
        self.needs_frame |= self.inspect.set_read_only(&self.world, settings.card);
        self.apply_tour();
    }

    /// The tour's settings, if it is on.
    #[must_use]
    pub fn tour(&self) -> Option<TourSettings> {
        self.tour.as_ref().map(|run| run.settings)
    }

    /// The tour card the tour wants now: the creature in focus and its page,
    /// once a glide has settled and the card setting is on; `None` during
    /// glides, pull-backs and drifts. (The tank pins and turns the inspect
    /// card to it by itself.)
    #[must_use]
    pub fn tour_card(&self) -> Option<TourCard> {
        let run = self.tour.as_ref().filter(|run| run.settings.card)?;
        run.director.card()
    }

    /// Tells the tank the size in pixels of the target it is shown in, for
    /// the tour to size its views (see [`Tank::set_tour`]). Call it at
    /// start-up and on every resize; a draw sets it too. A zero size counts
    /// as unknown: the tour keeps its last view until a size is known.
    pub fn set_target_size(&mut self, size: (u32, u32)) {
        if self.target_size == size {
            return;
        }
        self.target_size = size;
        if self.tour.is_some() {
            self.apply_tour();
            self.needs_frame = true;
        }
    }

    /// Where the tour is shown, once the target's size is known.
    fn tour_stage(&self) -> Option<Stage> {
        let target = self.target_size;
        (target.0 > 0 && target.1 > 0).then(|| Stage {
            layout: self.layout(target),
            target,
        })
    }

    /// Moves the tour on by one simulation step. Nothing moves while hidden.
    fn advance_tour(&mut self) {
        if self.sway.hidden() {
            return;
        }
        let Some(stage) = self.tour_stage() else {
            return;
        };
        if let Some(run) = self.tour.as_mut() {
            run.director.advance(TOUR_STEP, &self.world, &stage);
            self.apply_tour();
        }
    }

    /// Points the camera at the tour's view and pins, turns or removes the
    /// tour card to match. A view that hasn't moved asks for no frame.
    fn apply_tour(&mut self) {
        let Some(stage) = self.tour_stage() else {
            return;
        };
        let Some(run) = self.tour.as_ref() else {
            return;
        };
        let view = run.director.view(&stage);
        let card = run.settings.card.then(|| run.director.card()).flatten();
        let pinned = run.card_pinned;
        self.set_camera(Some(view));
        if let Some(card) = card {
            self.set_inspected(Some(Inspected::Creature(card.creature)));
            self.set_inspect_page(card.page);
        } else if pinned {
            self.set_inspected(None);
        }
        if let Some(run) = self.tour.as_mut() {
            run.card_pinned = card.is_some();
        }
    }

    /// The world.
    #[must_use]
    pub fn world(&self) -> &World {
        &self.world
    }

    /// The world, for edits. Edits show after the next simulation tick.
    pub fn world_mut(&mut self) -> &mut World {
        &mut self.world
    }

    /// Advances to `now` on the host's monotonic clock (any origin): runs
    /// the simulation ticks due (fixed 20 Hz) and says whether to draw and
    /// when to call again.
    pub fn tick(&mut self, now: Duration) -> FrameRequest {
        let sim_ticks = self.pacer.due_ticks(now);
        for _ in 0..sim_ticks {
            self.step();
        }
        let world_wanted = self.frame_wanted();
        let sway_on = self.sway_active();
        let sway_wanted = sway_on && self.sway.frame_due(now);
        // Hiding gates sway (via `sway_active`) and a touring tank's frames
        // (the tour's view is only worth drawing to be seen, and it keeps
        // the frame waiting for the return); otherwise the host pauses the
        // tank itself when hidden, and a world change still needs its frame.
        let hidden_tour = self.tour.is_some() && self.sway.hidden();
        let wanted = (world_wanted || sway_wanted) && !hidden_tour;
        let redraw = wanted && self.pacer.frame_allowed(now);
        if redraw {
            self.pacer.frame_drawn(now);
            if sway_on {
                self.sway.last_frame = Some(now);
            }
        }
        let mut wake_at = self.pacer.wake_at(now, wanted && !redraw);
        // Wake for the next sway frame if it isn't due yet; a due frame held
        // back by the frame cap is already in the pacer's wake time (and its
        // sway deadline may be in the past, which would spin the host).
        if sway_on
            && !redraw
            && !sway_wanted
            && let Some(last) = self.sway.last_frame
        {
            wake_at = wake_at.min(last + SWAY_INTERVAL);
        }
        FrameRequest {
            redraw,
            wake_at,
            sim_ticks,
        }
    }

    /// Runs one simulation tick now, outside the real-time pacing, and
    /// records what changed for the next frame. For headless hosts such as
    /// `desktank capture`, which step world time themselves.
    pub fn step(&mut self) {
        self.world.tick();
        if self.clock_running() {
            let minute = self
                .day_clock
                .clock
                .advance_life_ticks(self.world.last_life_ticks());
            if minute != self.time_of_day {
                self.apply_minute(minute);
            }
        }
        self.refresh_inspect();
        self.advance_tour();
        for r in self.world.dirty_rects() {
            self.dirty.add(r);
        }
        if self.field_overlay.is_some() && self.world.fields_version() != self.fields_drawn {
            self.needs_frame = true;
        }
        if self.world.device_rebuilds() != self.flow_drawn {
            self.needs_frame = true;
        }
    }

    /// Stops world time, e.g. while the surface is hidden. The next
    /// [`Tank::tick`] restarts the clock without catching up.
    pub fn pause(&mut self) {
        self.pacer.pause();
        self.sway.last_frame = None;
    }

    /// Asks for a frame even if the world hasn't changed (resize, expose).
    pub fn invalidate(&mut self) {
        self.needs_frame = true;
    }

    /// Re-uploads the whole grid with the next frame, so edits made through
    /// [`Tank::world_mut`] show without a simulation tick (while paused).
    pub fn cells_edited(&mut self) {
        self.dirty.mark_all();
    }

    /// Whether a frame is waiting: the world changed since the last frame
    /// (cells, creatures, plant tints or algae changed), or the target
    /// was invalidated. A tank with swimming creatures wants a frame every
    /// tick (20 a second, under the frame cap) even when every chunk
    /// sleeps; a plant's tint changes at most once per plant update.
    #[must_use]
    pub fn frame_wanted(&self) -> bool {
        self.needs_frame
            || !self.dirty.is_empty()
            || self.creatures_drawn != Some(self.world.creatures_version())
            || self.plants_drawn != Some(self.world.plants_version())
            || self.algae_drawn != Some(self.world.algae_version())
    }

    /// Changes the frame-rate cap (SPEC §6.2: 15 fps on battery). The
    /// simulation tick is unaffected.
    pub fn set_max_fps(&mut self, max_fps: u32) {
        self.pacer.set_max_fps(max_fps);
    }

    /// Drops every GPU resource (after device or context loss). The world is
    /// untouched; the next frame, drawn with a new [`Gpu`], rebuilds the
    /// renderer and uploads the whole grid.
    pub fn release_gpu(&mut self) {
        self.renderer = None;
        self.timer = None;
        self.gpu_time = None;
        self.dirty.mark_all();
        self.needs_frame = true;
    }

    /// Moves `old`'s GPU renderer and timer into this tank, so a replacement
    /// tank reuses the compiled pipelines and static textures. The renderer
    /// is reset for this tank's world (resized at the next frame if the
    /// grid differs), and everything is uploaded again.
    pub fn take_gpu(&mut self, old: &mut Self) {
        self.renderer = old.renderer.take();
        self.timer = old.timer.take();
        self.gpu_time = None;
        if let Some(r) = self.renderer.as_mut() {
            r.reset_world();
        }
        self.dirty.mark_all();
        self.palette_dirty = true;
        self.card_dirty = true;
        self.inspect.dirty = true;
        self.needs_frame = true;
        self.creatures_drawn = None;
        self.plants_drawn = None;
        self.algae_drawn = None;
        self.fields_drawn = 0;
        self.flow_drawn = 0;
    }

    /// Measures GPU time per frame with timestamp queries. Returns whether
    /// the device supports them. Reading the time blocks, so only
    /// benchmarks should turn this on.
    pub fn enable_gpu_timing(&mut self, gpu: &Gpu) -> bool {
        self.timer = GpuTimer::new(gpu);
        self.timer.is_some()
    }

    /// GPU time of the last frame, if timing is enabled and it was read.
    #[must_use]
    pub fn last_gpu_time(&self) -> Option<Duration> {
        self.gpu_time
    }

    /// Draws a frame to `surface` and presents it. Returns `Ok(false)` if
    /// the surface had no frame to give (occluded, timed out, or outdated
    /// and now reconfigured); the frame stays wanted, so the next tick
    /// asks again.
    ///
    /// # Errors
    ///
    /// Returns an error if the surface texture fails validation.
    pub fn draw(&mut self, gpu: &Gpu, surface: &TankSurface) -> Result<bool, DrawError> {
        let Some((frame, suboptimal)) = surface.acquire(gpu)? else {
            return Ok(false);
        };
        let drawn = self.present(gpu, surface, frame);
        if suboptimal {
            // Drawn; reconfigure for the next.
            surface.reconfigure(gpu);
        }
        Ok(drawn)
    }

    fn present(&mut self, gpu: &Gpu, surface: &TankSurface, frame: wgpu::SurfaceTexture) -> bool {
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        self.render_to(gpu, &view, surface.format(), surface.size());
        gpu.queue.present(frame);
        true
    }

    /// Draws a frame into `target` (`size` pixels of `format`, with
    /// `RENDER_ATTACHMENT` usage), e.g. an offscreen texture. Uploads the
    /// dirty rectangles first.
    pub fn render_to(
        &mut self,
        gpu: &Gpu,
        target: &wgpu::TextureView,
        format: wgpu::TextureFormat,
        size: (u32, u32),
    ) {
        if let Some(r) = self.renderer.as_mut().filter(|r| r.format() == format) {
            if !r.matches(&self.world, format) {
                r.resize_grid(gpu, &self.world);
                self.dirty.mark_all();
            }
        } else {
            self.renderer = Some(Renderer::new(gpu, &self.world, format));
            self.dirty.mark_all();
            self.palette_dirty = true;
            self.card_dirty = true;
            self.inspect.dirty = true;
        }
        // Fit the narrow/wide breakpoint before refreshing so a resize never
        // rebuilds the inspect card twice in one frame.
        self.fit_inspect(size);
        // The tour's views are of this size from here on.
        self.set_target_size(size);
        // Edits made through `world_mut` while paused may have removed it.
        self.refresh_inspect();
        let layout = self.layout(size);
        let palette = self.palette_layout(size);
        let card = self.card_layout(size);
        let inspect = self.inspect_layout(size);
        let sway_on = self.sway_active();
        let oled = self.oled();
        let ripples = self.world.surface_ripples();
        let has_current_ripples = self.world.has_surface_current_ripples();
        let surface_ripple_threshold = if has_current_ripples {
            let cur = self.world.ecology().current;
            (cur.surface_ripple_speed / cur.speed).clamp(f32::EPSILON, 1.0)
        } else {
            0.0
        };
        let phase = if sway_on || !ripples.is_empty() || has_current_ripples {
            self.sway_phase()
        } else {
            0.0
        };
        let Some(renderer) = self.renderer.as_mut() else {
            return;
        };
        renderer.upload(gpu, &self.world, &mut self.dirty);
        renderer.upload_sprites(gpu, &self.world, self.ghost);
        self.creatures_drawn = Some(self.world.creatures_version());
        renderer.upload_plants(gpu, &self.world);
        self.plants_drawn = Some(self.world.plants_version());
        renderer.upload_algae(gpu, &self.world);
        self.algae_drawn = Some(self.world.algae_version());
        if self.field_overlay.is_some() {
            renderer.upload_fields(gpu, &self.world);
            self.fields_drawn = self.world.fields_version();
        } else if has_current_ripples || sway_on {
            // Current-driven ripples and sway both read the 2D flow texture.
            renderer.upload_flow(gpu, &self.world);
        }
        self.flow_drawn = self.world.device_rebuilds();
        self.upload_hud(gpu);
        let Some(renderer) = self.renderer.as_mut() else {
            return;
        };

        let mut encoder = gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("frame"),
            });
        renderer.encode(
            gpu,
            &mut encoder,
            target,
            size,
            layout,
            self.camera,
            self.overlay_water_alpha,
            self.world.surface_row(),
            daylight::tint_with_schedule(self.time_of_day, self.daylight_schedule),
            phase,
            sway_on,
            &ripples,
            surface_ripple_threshold,
            Overlay {
                palette,
                card,
                inspect,
                cursor: self.cursor,
                magnifier: self.magnifier,
                field: self.field_overlay,
                ruler: self.ruler.then_some(self.card_min_scale),
            },
            oled,
            self.timer.as_ref(),
        );
        if let Some(t) = &self.timer {
            t.resolve(&mut encoder);
        }
        let submission = gpu.queue.submit([encoder.finish()]);
        self.needs_frame = false;
        if let Some(t) = &self.timer {
            self.gpu_time = t.read(gpu, submission);
        }
    }

    /// Uploads the palette, species card and inspect card images that
    /// changed to the HUD texture.
    fn upload_hud(&mut self, gpu: &Gpu) {
        let Some(renderer) = &self.renderer else {
            return;
        };
        if self.palette_dirty {
            if let Some(p) = &self.palette {
                let (image, image_size) = hud::build(
                    p,
                    self.world.materials(),
                    self.world.species(),
                    self.world.plant_species(),
                    self.world.device_kinds(),
                );
                renderer.upload_hud(gpu, &image, image_size);
            }
            self.palette_dirty = false;
        }
        if self.card_dirty {
            if let Some(image) = &self.card_image {
                renderer.upload_card(gpu, &image.pixels, image.size);
            }
            self.card_dirty = false;
        }
        if std::mem::take(&mut self.inspect.dirty)
            && let Some(image) = &self.inspect.image
        {
            renderer.upload_inspect(gpu, &image.pixels, image.size);
        }
    }

    /// Simulates the state-clearing side of `render_to` for GPU-free pacing
    /// and frame-count tests.
    #[cfg(test)]
    fn mark_drawn(&mut self) {
        self.dirty.take(|_| {});
        self.creatures_drawn = Some(self.world.creatures_version());
        self.plants_drawn = Some(self.world.plants_version());
        self.algae_drawn = Some(self.world.algae_version());
        if self.field_overlay.is_some() {
            self.fields_drawn = self.world.fields_version();
        }
        self.flow_drawn = self.world.device_rebuilds();
        self.needs_frame = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use desktank_sim::{Brush, MaterialTable, Propagule, Rect, WorldConfig};

    /// Builds a settled planted tank (a mature plant on sand/rock with 0
    /// awake chunks and no excess nutrients to spawn algae).
    fn settled_planted_tank() -> Tank {
        let mut world = World::new(
            WorldConfig {
                width: 64,
                height: 48,
                seed: 7,
                water_level: 0.9,
            },
            MaterialTable::builtin().unwrap(),
        )
        .unwrap();
        let sand = world.materials().id("sand").unwrap();
        let rock = world.materials().id("rock").unwrap();
        world
            .fill_rect(
                Rect {
                    x0: 0,
                    y0: 42,
                    x1: 64,
                    y1: 48,
                },
                sand,
                Brush::Replace,
            )
            .unwrap();
        world
            .fill_rect(
                Rect {
                    x0: 28,
                    y0: 38,
                    x1: 36,
                    y1: 42,
                },
                rock,
                Brush::Replace,
            )
            .unwrap();
        for b in world.bodies().map(|b| b.id).collect::<Vec<_>>() {
            world.set_anchored(b, true);
        }
        let moss = world.plant_species().id("java_moss").unwrap();
        world
            .paint_plant(32, 37, 0, moss, Propagule::Fragments)
            .unwrap();
        // Grow around the small rock until every site is covered and all
        // chunks sleep.
        for _ in 0..20 * 600 {
            world.tick();
        }
        assert!(world.plant_count() > 0, "expected a rooted plant");
        assert_eq!(world.awake_chunks(), 0, "expected a settled tank");
        let mut tank = Tank::new(world, TankConfig::default());
        // Anchor clock and draw the initial frame.
        let req = tank.tick(Duration::ZERO);
        assert!(req.redraw);
        tank.mark_drawn();
        tank
    }

    /// Steps `tank` at 50 ms intervals for `seconds` starting from `start`,
    /// returning how many frames `tick` asked to draw.
    fn count_frames(tank: &mut Tank, start: Duration, seconds: u32) -> u32 {
        count_frames_every(tank, start, seconds, 50)
    }

    /// Steps `tank` every `step_ms` for `seconds` starting from `start`,
    /// returning how many frames `tick` asked to draw.
    fn count_frames_every(tank: &mut Tank, start: Duration, seconds: u32, step_ms: u64) -> u32 {
        let mut frames = 0;
        let steps = u64::from(seconds) * 1000 / step_ms;
        for step in 1..=steps {
            let now = start + Duration::from_millis(step * step_ms);
            let req = tank.tick(now);
            assert!(
                req.wake_at > now,
                "wake_at {:?} not after {now:?}",
                req.wake_at
            );
            if req.redraw {
                tank.mark_drawn();
                frames += 1;
            }
        }
        frames
    }

    #[test]
    fn settled_planted_tank_draws_no_extra_frames_with_sway_off() {
        let mut tank = settled_planted_tank();
        assert!(!tank.sway(), "sway must be off by default");
        assert!(!tank.sway_active());
        let frames = count_frames(&mut tank, Duration::ZERO, 10);
        assert_eq!(frames, 0, "settled tank with sway off drew {frames} frames");
    }

    #[test]
    fn the_ruler_is_off_by_default_and_asks_for_one_frame_per_change() {
        let mut tank = settled_planted_tank();
        assert!(!tank.ruler(), "the ruler must be off by default");
        tank.set_ruler(false);
        assert!(!tank.needs_frame, "off to off changes nothing");
        tank.set_ruler(true);
        assert!(tank.ruler() && tank.needs_frame);
        tank.mark_drawn();
        tank.set_ruler(true);
        assert!(!tank.needs_frame, "on to on changes nothing");
        // A settled tank with the ruler on draws no extra frames.
        assert_eq!(count_frames(&mut tank, Duration::ZERO, 10), 0);
        tank.set_ruler(false);
        assert!(!tank.ruler() && tank.needs_frame);
    }

    #[test]
    fn a_coat_change_requests_one_frame_with_settled_cells() {
        let mut tank = settled_planted_tank();
        assert!(!tank.frame_wanted());
        assert!(tank.world_mut().set_algae_coat(0, 42, 15));
        assert!(tank.dirty.is_empty());
        assert!(tank.tick(Duration::from_millis(50)).redraw);
        tank.mark_drawn();
        assert!(!tank.frame_wanted());
    }

    #[test]
    fn sway_caps_redraws_at_10_fps_and_respects_power_and_visibility() {
        let mut tank = settled_planted_tank();
        tank.set_sway(true);
        assert!(tank.sway());
        assert!(tank.sway_active());
        // Draw the state-change frame at t = 1 s, then measure 10 s.
        let mut now = Duration::from_secs(1);
        let req = tank.tick(now);
        assert!(req.redraw);
        tank.mark_drawn();
        // Polled every 5 ms (as a host woken early might), sway draws at
        // its cap and no faster.
        let frames = count_frames_every(&mut tank, now, 10, 5);
        assert!(
            (10 * SWAY_MAX_FPS - 2..=10 * SWAY_MAX_FPS).contains(&frames),
            "sway drew {frames} frames in 10 s (cap {})",
            10 * SWAY_MAX_FPS
        );
        now += Duration::from_secs(10);

        // On battery: one frame to snap back to rest, then 0 sway frames.
        tank.set_on_battery(true);
        assert!(!tank.allows_sway_on_battery());
        assert!(!tank.sway_active());
        let req = tank.tick(now + Duration::from_millis(50));
        if req.redraw {
            tank.mark_drawn();
        }
        now += Duration::from_millis(50);
        assert_eq!(
            count_frames(&mut tank, now, 5),
            0,
            "sway must draw nothing on battery"
        );
        now += Duration::from_secs(5);
        tank.set_allow_sway_on_battery(true);
        assert!(tank.sway_active());
        let frames = count_frames_every(&mut tank, now, 5, 5);
        assert!(
            (5 * SWAY_MAX_FPS - 1..=5 * SWAY_MAX_FPS).contains(&frames),
            "sway opted in on battery drew {frames} frames in 5 s"
        );
        now += Duration::from_secs(5);

        // In screensaver mode: 0 sway frames.
        tank.set_saver_mode(true);
        assert!(!tank.sway_active());
        let req = tank.tick(now + Duration::from_millis(50));
        if req.redraw {
            tank.mark_drawn();
        }
        now += Duration::from_millis(50);
        assert_eq!(
            count_frames(&mut tank, now, 5),
            0,
            "sway must draw nothing in saver mode"
        );
        now += Duration::from_secs(5);
        tank.set_saver_mode(false);
        assert!(tank.sway_active());
        assert!(tank.tick(now).redraw, "sway resumes out of saver mode");
        tank.mark_drawn();

        // While hidden: 0 sway frames immediately (no rest-snap frame either).
        tank.set_hidden(true);
        assert!(!tank.sway_active());
        assert_eq!(
            count_frames(&mut tank, now, 5),
            0,
            "sway must draw nothing while hidden"
        );
        now += Duration::from_secs(5);

        // Shown again: sway resumes at once.
        tank.set_hidden(false);
        assert!(tank.tick(now + Duration::from_millis(50)).redraw);

        tank.set_sway(false);
        assert!(!tank.sway_active(), "the battery opt-in requires sway");
        tank.set_sway(true);
        assert!(tank.sway_active());
        tank.set_allow_sway_on_battery(false);
        assert!(!tank.sway_active());
        tank.set_on_battery(false);
        assert!(tank.sway_active());
    }

    #[test]
    fn sway_under_a_lower_frame_cap_never_wakes_in_the_past() {
        let mut tank = settled_planted_tank();
        tank.set_max_fps(5);
        tank.set_sway(true);
        // `count_frames_every` asserts every wake time is in the future.
        let frames = count_frames_every(&mut tank, Duration::from_secs(1), 10, 5);
        assert!(
            (48..=51).contains(&frames),
            "{frames} frames in 10 s at a 5 fps cap"
        );
    }

    #[test]
    fn sway_phase_wraps_seamlessly() {
        let period = 20.0 * std::f32::consts::PI;
        for phase in [0.0_f32, 0.3, 1.7] {
            let wave = |t: f32| (t * 1.6).sin() * 0.65 + (t * 0.9).sin() * 0.35;
            assert!((wave(phase) - wave(phase + period)).abs() < 1e-3);
        }
    }

    #[test]
    fn sway_draws_nothing_without_plants() {
        let world = World::new(
            WorldConfig {
                width: 64,
                height: 48,
                seed: 7,
                water_level: 0.9,
            },
            MaterialTable::builtin().unwrap(),
        )
        .unwrap();
        let mut tank = Tank::new(world, TankConfig::default());
        let req = tank.tick(Duration::ZERO);
        assert!(req.redraw);
        tank.mark_drawn();
        tank.set_sway(true);
        assert!(!tank.sway_active());
        assert_eq!(count_frames(&mut tank, Duration::ZERO, 5), 0);
    }

    #[test]
    fn overlay_alpha_is_sanitized_and_changes_request_a_frame() {
        let mut tank = settled_planted_tank();
        for (input, expected) in [
            (Some(f32::NAN), Some(0.0)),
            (Some(f32::INFINITY), Some(1.0)),
            (Some(-0.25), Some(0.0)),
            (None, None),
        ] {
            tank.mark_drawn();
            tank.set_overlay_water_alpha(input);
            assert_eq!(tank.overlay_water_alpha(), expected);
            assert!(tank.frame_wanted());
            tank.mark_drawn();
            tank.set_overlay_water_alpha(input);
            assert!(!tank.frame_wanted());
        }
    }

    #[test]
    fn settled_tank_draws_only_when_the_card_changes() {
        let mut tank = settled_planted_tank();
        let guppy = Card::Creature(tank.world().species().id("guppy").unwrap(), None);
        let moss = Card::Plant(tank.world().plant_species().id("java_moss").unwrap());
        // No card: nothing built, nothing to draw, no frame.
        assert_eq!(tank.card(), None);
        tank.set_card(None);
        assert!(tank.card_image.is_none() && !tank.card_dirty);
        assert_eq!(tank.card_layout((1280, 720)), None);
        assert_eq!(count_frames(&mut tank, Duration::ZERO, 2), 0);

        // A card builds its image once and asks for one frame.
        tank.set_card(Some(guppy));
        assert_eq!(tank.card(), Some(guppy));
        assert!(tank.card_image.is_some() && tank.card_dirty);
        assert!(tank.card_layout((1280, 720)).is_some());
        assert_eq!(count_frames(&mut tank, Duration::from_secs(2), 2), 1);

        // The same card again rebuilds nothing, uploads nothing, asks for
        // no frame.
        tank.card_dirty = false;
        let image = tank.card_image.clone();
        tank.set_card(Some(guppy));
        assert!(!tank.card_dirty);
        assert_eq!(tank.card_image, image);
        assert_eq!(count_frames(&mut tank, Duration::from_secs(4), 2), 0);

        // Another card is a change; hiding it is another.
        tank.set_card(Some(moss));
        assert!(tank.card_dirty);
        assert_ne!(tank.card_image, image);
        assert_eq!(count_frames(&mut tank, Duration::from_secs(6), 1), 1);
        tank.set_card(None);
        assert!(tank.card_image.is_none());
        assert_eq!(count_frames(&mut tank, Duration::from_secs(7), 1), 1);
        assert_eq!(count_frames(&mut tank, Duration::from_secs(9), 2), 0);

        // An unknown species shows nothing but is still a change.
        tank.set_card(Some(Card::Creature(desktank_sim::SpeciesId(250), None)));
        assert!(tank.card_image.is_none());
        assert_eq!(tank.card_layout((1280, 720)), None);
    }

    #[test]
    fn settled_tank_draws_the_inspect_card_only_when_it_changes() {
        let mut tank = settled_planted_tank();
        let plant = Inspected::Plant(tank.world().plants().next().unwrap().id);
        // Nothing inspected: nothing built, no frame.
        tank.set_inspected(None);
        assert!(tank.inspect.image.is_none() && !tank.inspect.dirty);
        assert_eq!(tank.inspect_layout((1280, 720)), None);
        assert_eq!(count_frames(&mut tank, Duration::ZERO, 2), 0);

        // Inspecting builds the live page once and asks for one frame. The
        // live page changes at most once a life day (ten minutes) for a
        // plant.
        tank.set_inspected(Some(plant));
        assert_eq!(tank.inspected(), Some(plant));
        assert_eq!(tank.inspect_page(), InspectPage::Live);
        assert!(tank.inspect.image.is_some() && tank.inspect.dirty);
        let layout = tank.inspect_layout((1280, 720)).unwrap();
        assert!(layout.scale >= 1);
        assert!(count_frames(&mut tank, Duration::from_secs(2), 3) <= 2);
        tank.inspect.dirty = false;
        let live = tank.inspect.image.clone();
        tank.set_inspected(Some(plant));
        assert!(!tank.inspect.dirty);

        // Turning the page rebuilds once; setting it again is free.
        tank.set_inspect_page(InspectPage::Species);
        assert!(tank.inspect.dirty);
        assert_ne!(tank.inspect.image, live);
        assert_eq!(count_frames(&mut tank, Duration::from_secs(5), 1), 1);
        tank.inspect.dirty = false;
        tank.set_inspect_page(InspectPage::Species);
        assert!(!tank.inspect.dirty);
        assert_eq!(count_frames(&mut tank, Duration::from_secs(6), 2), 0);

        // Removing it is a change; then nothing.
        tank.set_inspected(None);
        assert!(tank.inspect.image.is_none());
        assert_eq!(count_frames(&mut tank, Duration::from_secs(8), 1), 1);
        assert_eq!(count_frames(&mut tank, Duration::from_secs(9), 2), 0);
    }

    #[test]
    fn the_inspect_card_follows_its_creature_and_goes_with_it() {
        let mut tank = settled_planted_tank();
        let guppy = tank.world().species().id("guppy").unwrap();
        let id = tank.world_mut().add_creature(guppy, 16, 20).unwrap();
        tank.set_inspected(Some(Inspected::Creature(id)));
        tank.set_inspect_page(InspectPage::Species);
        let target = (1280, 720);
        let before = tank.inspect_layout(target).unwrap();
        let centre = tank.world().creature(id).unwrap().centre();
        // It follows as the creature swims.
        let mut moved = false;
        for _ in 0..200 {
            tank.step();
            let now = tank.world().creature(id).unwrap().centre();
            if (now.0 - centre.0).abs() > 2.0 || (now.1 - centre.1).abs() > 2.0 {
                moved = tank.inspect_layout(target).unwrap() != before;
                break;
            }
        }
        assert!(moved, "the card followed");
        assert_eq!(tank.inspect_page(), InspectPage::Species);
        // Netted out: the card goes with it, at the next tick or frame.
        assert!(tank.world_mut().remove_creature(id));
        tank.step();
        assert_eq!(tank.inspected(), None);
        assert!(tank.inspect.image.is_none());
        assert_eq!(tank.inspect_layout(target), None);
        assert_eq!(tank.inspect_page(), InspectPage::Live);
        assert!(tank.frame_wanted());
    }

    #[test]
    fn setting_a_camera_asks_for_a_frame_and_a_bad_one_counts_as_none() {
        let mut tank = settled_planted_tank();
        let target = (1280, 720);
        assert_eq!(count_frames(&mut tank, Duration::ZERO, 2), 0);
        assert_eq!(tank.camera(), None);

        let view = tank.camera_cover(target, (32.0, 24.0), 2.5);
        tank.set_camera(Some(view));
        assert_eq!(tank.camera(), Some(view));
        assert!(tank.frame_wanted());
        tank.mark_drawn();
        // The same view again costs nothing; a new one asks again.
        tank.set_camera(Some(view));
        assert!(!tank.frame_wanted());
        tank.set_camera(Some(ViewRect {
            x: view.x + 0.25,
            ..view
        }));
        assert!(tank.frame_wanted());
        tank.mark_drawn();
        // A view that can't be drawn is no camera, and going back is a change.
        tank.set_camera(Some(ViewRect {
            w: f32::NAN,
            ..view
        }));
        assert_eq!(tank.camera(), None);
        assert!(tank.frame_wanted());
        tank.mark_drawn();
        tank.set_camera(None);
        assert!(!tank.frame_wanted());
    }

    #[test]
    fn grid_and_screen_map_through_the_camera_or_the_plain_layout() {
        let mut tank = settled_planted_tank();
        let target = (1280, 720);
        let layout = tank.layout(target);
        // Without a camera it is the integer layout's mapping.
        let (px, py) = tank.grid_to_screen(target, (10.5, 4.25));
        let s = f64::from(layout.scale);
        let at = |v: i64| f64::from(i32::try_from(v).unwrap());
        let offset = (at(layout.offset.0), at(layout.offset.1));
        assert!((px - (offset.0 + 10.5 * s)).abs() < 1e-3);
        assert!((py - (offset.1 + 4.25 * s)).abs() < 1e-3);
        let (gx, gy) = tank.screen_to_grid(target, (px, py));
        assert!((gx - 10.5).abs() < 1e-3 && (gy - 4.25).abs() < 1e-3);
        let cell = layout.cell_at(px, py);
        assert_eq!(
            (gx.floor(), gy.floor()),
            (at(cell.0), at(cell.1)),
            "screen_to_grid agrees with Layout::cell_at"
        );

        // Under a camera it follows the view.
        let view = tank.camera_cover(target, (32.0, 24.0), 2.0);
        tank.set_camera(Some(view));
        let (px, py) = tank.grid_to_screen(target, (32.0, 24.0));
        assert!((px - 640.0).abs() < 1e-3 && (py - 360.0).abs() < 1e-3);
        let (gx, gy) = tank.screen_to_grid(target, (640.0, 360.0));
        assert!((gx - 32.0).abs() < 1e-3 && (gy - 24.0).abs() < 1e-3);
        assert_eq!(tank.view(target), view);
        tank.set_camera(None);
        assert_eq!(tank.view(target), tank.camera_fit(target));
    }

    #[test]
    fn the_inspect_card_follows_its_creature_through_the_camera_at_its_usual_scale() {
        let mut tank = settled_planted_tank();
        let guppy = tank.world().species().id("guppy").unwrap();
        let id = tank.world_mut().add_creature(guppy, 32, 24).unwrap();
        tank.set_inspect_naming(true);
        tank.set_inspected(Some(Inspected::Creature(id)));
        let target = (1280, 720);
        let plain = tank.inspect_layout(target).unwrap();

        let centre = tank.world().creature(id).unwrap().centre();
        tank.set_camera(Some(tank.camera_cover(target, centre, 2.5)));
        let zoomed = tank.inspect_layout(target).unwrap();
        // Same size and scale; beside the creature, which is mid-screen.
        assert_eq!((zoomed.scale, zoomed.image), (plain.scale, plain.image));
        let (cx, cy) = tank.grid_to_screen(target, (f64::from(centre.0), f64::from(centre.1)));
        assert!((cx - 640.0).abs() < 1.0 && (cy - 360.0).abs() < 1.0);
        assert!(f64::from(zoomed.origin.0) > cx);
        let mid = f64::from(zoomed.origin.1) + f64::from(zoomed.size().1) / 2.0;
        assert!((mid - cy).abs() < 2.0, "{mid} vs {cy}");
        // The name row's hit rectangle moves with it.
        let row = tank.inspect_name_rect(target).unwrap();
        assert!(row.contains(
            f64::from(zoomed.origin.0) + f64::from(row.origin.0 - zoomed.origin.0),
            f64::from(row.origin.1)
        ));
        // Dropping the camera puts the card back.
        tank.set_camera(None);
        assert_eq!(tank.inspect_layout(target), Some(plain));
    }

    #[test]
    fn the_inspect_card_and_the_species_card_share_one_scale() {
        let world = World::new(
            WorldConfig {
                width: 320,
                height: 180,
                seed: 7,
                water_level: 0.9,
            },
            MaterialTable::builtin().unwrap(),
        )
        .unwrap();
        let mut tank = Tank::new(world, TankConfig::default());
        let guppy = tank.world().species().id("guppy").unwrap();
        let id = tank.world_mut().add_creature(guppy, 160, 90).unwrap();
        tank.set_card(Some(Card::Creature(guppy, None)));
        tank.set_inspected(Some(Inspected::Creature(id)));
        // A desktop canvas (cell scale 4), a DPR-3 phone's with the host's
        // minimum, and a coarse small one.
        for (target, min, scale) in [((1280, 720), 0, 4), ((1092, 614), 3, 3), ((512, 288), 0, 1)] {
            tank.set_card_min_scale(min);
            let species = tank.card_layout(target).unwrap().scale;
            assert_eq!(species, scale, "{target:?}");
            for page in [InspectPage::Live, InspectPage::Species] {
                tank.set_inspect_page(page);
                assert_eq!(
                    tank.inspect_layout(target).unwrap().scale,
                    species,
                    "{target:?} {page:?}"
                );
            }
        }
        // With a palette in the canvas both are capped by its scale and still
        // match.
        tank.set_card_min_scale(0);
        tank.set_palette(Some(Palette {
            tabs: Vec::new(),
            open: 0,
            subs: Vec::new(),
            open_sub: 0,
            items: Vec::new(),
            selected: None,
            morphs: None,
        }));
        let target = (1280, 720);
        let palette = tank.palette_layout(target).unwrap();
        let species = tank.card_layout(target).unwrap().scale;
        assert!(species <= palette.scale);
        assert_eq!(tank.inspect_layout(target).unwrap().scale, species);
    }

    #[test]
    fn the_host_minimum_card_scale_enlarges_the_card_and_asks_for_one_frame() {
        let world = World::new(
            WorldConfig {
                width: 320,
                height: 180,
                seed: 7,
                water_level: 0.9,
            },
            MaterialTable::builtin().unwrap(),
        )
        .unwrap();
        let mut tank = Tank::new(world, TankConfig::default());
        let guppy = Card::Creature(tank.world().species().id("guppy").unwrap(), None);
        tank.set_card(Some(guppy));
        let req = tank.tick(Duration::ZERO);
        assert!(req.redraw);
        tank.mark_drawn();
        // A DPR-3 phone's canvas: the grid is at scale 2 and so is the card.
        let target = (1092, 614);
        assert_eq!(tank.layout(target).scale, 2);
        assert_eq!(tank.card_min_scale(), 0);
        assert_eq!(tank.card_layout(target).unwrap().scale, 2);
        assert_eq!(count_frames(&mut tank, Duration::from_secs(2), 2), 0);

        tank.set_card_min_scale(3);
        assert_eq!(tank.card_min_scale(), 3);
        assert_eq!(tank.card_layout(target).unwrap().scale, 3);
        assert_eq!(count_frames(&mut tank, Duration::from_secs(4), 2), 1);

        // The same value again is free.
        tank.set_card_min_scale(3);
        assert_eq!(count_frames(&mut tank, Duration::from_secs(6), 2), 0);
    }

    #[test]
    fn naming_edits_the_name_row_and_rebuilds_the_card_only_when_the_text_changes() {
        let mut tank = settled_planted_tank();
        let guppy = tank.world().species().id("guppy").unwrap();
        let id = tank.world_mut().add_creature(guppy, 16, 20).unwrap();
        let target = (1280, 720);
        tank.set_inspected(Some(Inspected::Creature(id)));
        // Naming is off until the host allows it: no prompt, nothing to
        // click, and an edit is ignored.
        assert_eq!(tank.inspect_name_rect(target), None);
        let plain = tank.inspect.image.clone();
        tank.set_inspect_name_edit(Some("Bub"));
        assert_eq!(tank.inspect_name_edit(), None);
        assert_eq!(tank.inspect.image, plain);

        // Allowing it adds the prompt row, once, and the row's rect: on the
        // card, under its heading, inside its width.
        tank.inspect.dirty = false;
        tank.set_inspect_naming(true);
        assert!(tank.inspect.dirty);
        assert_ne!(tank.inspect.image, plain);
        let card = tank.inspect_layout(target).unwrap();
        let row = tank.inspect_name_rect(target).unwrap();
        assert!(row.origin.0 >= card.origin.0 && row.origin.1 > card.origin.1);
        assert!(row.origin.0 + row.size.0 <= card.origin.0 + card.size().0);
        assert!(row.origin.1 + row.size.1 < card.origin.1 + card.size().1);
        assert!(row.contains(f64::from(row.origin.0) + 1.0, f64::from(row.origin.1) + 1.0));
        assert!(!row.contains(f64::from(card.origin.0), f64::from(card.origin.1)));
        tank.inspect.dirty = false;
        tank.set_inspect_naming(true);
        assert!(!tank.inspect.dirty, "the same setting rebuilds nothing");

        // Editing rebuilds for each change of the text and for nothing else.
        let prompt = tank.inspect.image.clone();
        tank.set_inspect_name_edit(Some(""));
        assert!(tank.inspect.dirty);
        assert_eq!(tank.inspect_name_edit(), Some(""));
        assert_ne!(tank.inspect.image, prompt);
        tank.inspect.dirty = false;
        tank.set_inspect_name_edit(Some(""));
        assert!(!tank.inspect.dirty, "the same text rebuilds nothing");
        tank.set_inspect_name_edit(Some("Bub"));
        assert!(tank.inspect.dirty);
        let typed = tank.inspect.image.clone();
        tank.inspect.dirty = false;
        // Ticks with the text unchanged rebuild nothing either.
        for _ in 0..5 {
            tank.step();
        }
        tank.set_inspect_name_edit(Some("Bub"));
        assert!(!tank.inspect.dirty);
        assert_eq!(tank.inspect.image, typed);
        // Long text is cut to the cap and control characters are dropped.
        let long = format!("{}\n", "x".repeat(desktank_sim::MAX_NAME_CHARS * 2));
        tank.set_inspect_name_edit(Some(&long));
        assert_eq!(
            tank.inspect_name_edit().map(|t| t.chars().count()),
            Some(desktank_sim::MAX_NAME_CHARS)
        );
        assert!(!tank.inspect.image.as_ref().unwrap().clipped);

        // Editing shows the live page; the species page has no name row.
        tank.set_inspect_name_edit(None);
        tank.set_inspect_page(InspectPage::Species);
        assert_eq!(tank.inspect_name_rect(target), None);
        tank.set_inspect_name_edit(Some("Bub"));
        assert_eq!(tank.inspect_page(), InspectPage::Live);
        assert!(tank.inspect_name_rect(target).is_some());

        // A name the world has is what the row shows once editing stops; a
        // viewer, who may not name, sees it too, with no row to click.
        tank.world_mut().set_creature_name(id, "Bubbles");
        tank.set_inspect_name_edit(None);
        assert_eq!(tank.inspect_name_edit(), None);
        assert!(tank.inspect_name_rect(target).is_some());
        tank.set_inspect_naming(false);
        assert_eq!(tank.inspect_name_rect(target), None);
        assert!(tank.inspect.image.is_some());

        // Inspecting something else, or a plant, ends an edit.
        tank.set_inspect_naming(true);
        tank.set_inspect_name_edit(Some("x"));
        tank.set_inspected(Some(Inspected::Cell { x: 5, y: 60 }));
        assert_eq!(tank.inspect_name_edit(), None);
        assert_eq!(tank.inspect_name_rect(target), None);
        tank.set_inspect_name_edit(Some("x"));
        assert_eq!(tank.inspect_name_edit(), None, "only creatures are named");

        // The permission outlives the creature.
        tank.set_inspected(Some(Inspected::Creature(id)));
        assert!(tank.world_mut().remove_creature(id));
        tank.step();
        assert_eq!(tank.inspected(), None);
        let other = tank.world_mut().add_creature(guppy, 40, 20).unwrap();
        tank.set_inspected(Some(Inspected::Creature(other)));
        assert!(tank.inspect_name_rect(target).is_some());
    }

    const TOUR_TARGET: (u32, u32) = (1280, 720);

    /// A settled planted tank with `fish` guppies, touring in a 1280×720
    /// target.
    fn touring_tank(fish: i64, settings: TourSettings) -> Tank {
        let mut tank = settled_planted_tank();
        let guppy = tank.world().species().id("guppy").unwrap();
        for i in 0..fish {
            tank.world_mut()
                .add_creature(guppy, 10 + 9 * (i % 5), 12 + 7 * (i % 3))
                .unwrap();
        }
        tank.set_target_size(TOUR_TARGET);
        tank.set_tour(Some(settings));
        tank
    }

    /// Steps `tank` until the tour card shows, at most `seconds` of world time.
    fn step_to_a_tour_card(tank: &mut Tank, seconds: u32) -> TourCard {
        for _ in 0..seconds * 20 {
            tank.step();
            if let Some(card) = tank.tour_card() {
                return card;
            }
        }
        panic!("no tour card within {seconds} s");
    }

    #[test]
    fn a_tour_moves_the_camera_and_asks_for_frames_at_the_cap_only() {
        for fish in [0, 3] {
            let mut tank = touring_tank(fish, TourSettings::default());
            assert!(tank.camera().is_some(), "the camera is set at once");
            assert_eq!(tank.tour(), Some(TourSettings::default()));
            let before = tank.camera();
            // 100 Hz host ticks: the view moves every 20 Hz sim step, and
            // frames never come faster than the 30 fps cap allows.
            let frames = count_frames_every(&mut tank, Duration::ZERO, 20, 10);
            assert!(
                (380..=601).contains(&frames),
                "{fish} fish: {frames} frames in 20 s"
            );
            assert_ne!(tank.camera(), before, "the view moved");
            // Without a tour, the same settled tank asks for none.
            let mut still = settled_planted_tank();
            assert_eq!(count_frames_every(&mut still, Duration::ZERO, 20, 10), 0);
        }
    }

    #[test]
    fn a_hidden_tank_neither_asks_for_frames_nor_advances_the_tour() {
        // With nothing swimming, and with fish whose moves would want frames.
        for fish in [0, 3] {
            let mut tank = touring_tank(fish, TourSettings::default());
            count_frames_every(&mut tank, Duration::ZERO, 5, 10);
            tank.set_hidden(true);
            let view = tank.camera();
            let start = Duration::from_secs(5);
            let frames = count_frames_every(&mut tank, start, 30, 10);
            assert_eq!(frames, 0, "{fish} fish: no frames while hidden");
            assert_eq!(tank.camera(), view, "the tour stood still");
            // Back in view, it redraws and goes on.
            tank.set_hidden(false);
            assert!(tank.frame_wanted());
            let frames = count_frames_every(&mut tank, start + Duration::from_secs(30), 5, 10);
            assert!(frames > 50, "{fish} fish: {frames}");
            assert_ne!(tank.camera(), view);
        }
    }

    #[test]
    fn a_paused_touring_tank_goes_idle_like_a_plain_paused_one() {
        for fish in [0, 3] {
            let mut tank = touring_tank(fish, TourSettings::default());
            count_frames_every(&mut tank, Duration::ZERO, 5, 10);
            // A host holding the clock (a paused owner, a viewer) pauses the
            // tank before every tick: no step runs, so the view is still.
            let view = tank.camera();
            let mut frames = 0;
            for step in 1..=300u64 {
                let now = Duration::from_secs(5) + Duration::from_millis(step * 10);
                tank.pause();
                let req = tank.tick(now);
                assert_eq!(req.sim_ticks, 0);
                if req.redraw {
                    tank.mark_drawn();
                    frames += 1;
                }
            }
            // The one draw owed for the change in flight is fine; a stream
            // of identical frames is not.
            assert!(frames <= 1, "{fish} fish: {frames} frames in 3 s paused");
            assert!(!tank.frame_wanted(), "{fish} fish: still wants a frame");
            assert_eq!(tank.camera(), view);
        }
    }

    #[test]
    fn the_tour_waits_for_a_target_size_and_follows_a_resize() {
        let mut tank = settled_planted_tank();
        let guppy = tank.world().species().id("guppy").unwrap();
        tank.world_mut().add_creature(guppy, 16, 20).unwrap();
        tank.set_tour(Some(TourSettings::default()));
        for _ in 0..40 {
            tank.step();
        }
        assert_eq!(tank.camera(), None, "no size, no view");
        tank.set_target_size(TOUR_TARGET);
        let view = tank.camera().unwrap();
        assert!(view.is_valid());
        assert!(tank.frame_wanted());
        tank.mark_drawn();
        // A resize turns the same tour into a view of the new target.
        for _ in 0..400 {
            tank.step();
        }
        tank.set_target_size((1680, 720));
        let wide = tank.camera().unwrap();
        assert!(wide.is_valid());
        assert!((wide.w / wide.h - 1680.0 / 720.0).abs() < 0.01, "{wide:?}");
        assert!(tank.frame_wanted());
        // The same size again changes nothing.
        tank.mark_drawn();
        tank.set_target_size((1680, 720));
        assert!(!tank.frame_wanted());
    }

    #[test]
    fn turning_the_tour_off_clears_the_camera_and_the_tour_card_only() {
        let mut tank = touring_tank(
            4,
            TourSettings {
                seed: 1,
                card: true,
            },
        );
        let card = step_to_a_tour_card(&mut tank, 60);
        assert_eq!(tank.inspected(), Some(Inspected::Creature(card.creature)));
        assert!(tank.camera().is_some());
        tank.mark_drawn();
        tank.set_tour(None);
        assert_eq!(tank.tour(), None);
        assert_eq!(tank.camera(), None);
        assert_eq!(tank.inspected(), None);
        assert_eq!(tank.tour_card(), None);
        assert!(tank.inspect.image.is_none());
        assert!(tank.frame_wanted(), "the plain tank needs its frame");
        // The tank stands still again.
        tank.mark_drawn();
        for _ in 0..200 {
            tank.step();
        }
        assert_eq!(tank.camera(), None);
        tank.set_tour(None);

        // A card pinned by hand stays when a tour without a card goes.
        let mut tank = touring_tank(2, TourSettings::default());
        let id = tank.world().creatures()[0].id();
        tank.set_inspected(Some(Inspected::Creature(id)));
        for _ in 0..600 {
            tank.step();
        }
        assert_eq!(tank.inspected(), Some(Inspected::Creature(id)));
        tank.set_tour(None);
        assert_eq!(tank.inspected(), Some(Inspected::Creature(id)));
        assert_eq!(tank.camera(), None);
    }

    #[test]
    fn changing_only_the_card_setting_keeps_the_tour_going_and_a_new_seed_restarts_it() {
        let mut tank = touring_tank(4, TourSettings::default());
        for _ in 0..300 {
            tank.step();
        }
        let view = tank.camera();
        let same = TourSettings::default();
        tank.set_tour(Some(same));
        assert_eq!(tank.camera(), view);
        tank.set_tour(Some(TourSettings { card: true, ..same }));
        assert_eq!(tank.camera(), view, "the tour went on");
        assert!(tank.tour().unwrap().card);
        tank.set_tour(Some(TourSettings {
            seed: 9,
            card: true,
        }));
        assert_ne!(tank.camera(), view, "a new seed starts over");
    }

    #[test]
    fn the_tour_card_pins_to_the_creature_in_focus_turns_pages_and_goes_with_the_glide() {
        let mut tank = touring_tank(
            4,
            TourSettings {
                seed: 3,
                card: true,
            },
        );
        // Not while gliding in; then once it has settled.
        assert_eq!(tank.inspected(), None);
        let card = step_to_a_tour_card(&mut tank, 60);
        let id = card.creature;
        assert_eq!(card.page, InspectPage::Live);
        assert_eq!(tank.inspected(), Some(Inspected::Creature(id)));
        assert_eq!(tank.inspect_page(), InspectPage::Live);
        assert!(tank.inspect_layout(TOUR_TARGET).is_some(), "it is drawn");
        // Live for 12 s, then species, then live (checked away from the
        // turns at 12 s and 24 s).
        for (turn, (seconds, page)) in [
            (11, InspectPage::Live),
            (2, InspectPage::Species),
            (9, InspectPage::Species),
            (4, InspectPage::Live),
        ]
        .into_iter()
        .enumerate()
        {
            for _ in 0..seconds * 20 {
                tank.step();
            }
            if tank.tour_card().is_none() {
                // A focus lasts 20 s at least: the 24 s mark may be past it.
                assert!(turn >= 2, "the focus ended after {turn} turns");
                break;
            }
            assert_eq!(tank.inspect_page(), page);
            assert_eq!(tank.inspected(), Some(Inspected::Creature(id)));
        }
        // The card goes when the next glide starts, and stays gone.
        let mut gone = false;
        for _ in 0..60 * 20 {
            tank.step();
            if tank.tour_card().is_none() {
                assert_eq!(tank.inspected(), None);
                assert!(tank.inspect.image.is_none());
                gone = true;
                break;
            }
        }
        assert!(gone, "the focus ended");
        for _ in 0..4 * 20 {
            tank.step();
            assert_eq!(tank.inspected(), None, "no card while gliding");
        }
        // It comes back on whoever is next.
        let next = step_to_a_tour_card(&mut tank, 60);
        assert_eq!(tank.inspected(), Some(Inspected::Creature(next.creature)));
        assert_eq!(tank.inspect_page(), InspectPage::Live);
    }

    #[test]
    fn a_tour_without_the_card_setting_pins_no_card() {
        let mut tank = touring_tank(4, TourSettings::default());
        for _ in 0..90 * 20 {
            tank.step();
            assert_eq!(tank.inspected(), None);
            assert_eq!(tank.tour_card(), None);
        }
    }

    #[test]
    fn the_tour_card_never_shows_the_name_prompt_even_with_naming_allowed() {
        let settings = TourSettings {
            seed: 5,
            card: true,
        };
        let mut tank = touring_tank(4, settings);
        tank.set_inspect_naming(true);
        let card = step_to_a_tour_card(&mut tank, 60);
        let id = card.creature;
        // Whatever the host allows: no row to click, no edit taken.
        assert_eq!(tank.inspect_name_rect(TOUR_TARGET), None);
        tank.set_inspect_name_edit(Some("Bub"));
        assert_eq!(tank.inspect_name_edit(), None);
        // The card is the one a viewer sees, with no prompt on an unnamed
        // creature (and the owner's would have one).
        let mut viewer = InspectCard::default();
        viewer.set(tank.world(), Some(Inspected::Creature(id)));
        assert_eq!(tank.inspect.image, viewer.image);
        viewer.set_can_name(tank.world(), true);
        assert_ne!(
            tank.inspect.image, viewer.image,
            "the owner's card has a prompt"
        );

        // A creature that already has a name shows it, still read-only.
        let mut named = touring_tank(1, settings);
        let only = named.world().creatures()[0].id();
        named.world_mut().set_creature_name(only, "Bubbles");
        named.set_inspect_naming(true);
        step_to_a_tour_card(&mut named, 60);
        let mut viewer = InspectCard::default();
        viewer.set(named.world(), Some(Inspected::Creature(only)));
        assert_eq!(
            named.inspect.image, viewer.image,
            "the viewer's card, with its name row"
        );
        assert_eq!(named.inspect_name_rect(TOUR_TARGET), None);
        named.set_inspect_name_edit(Some("Bub"));
        assert_eq!(named.inspect_name_edit(), None);
        assert_eq!(
            named.world().creature(only).and_then(|c| c.name()),
            Some("Bubbles"),
            "and nothing renames it"
        );
    }

    #[test]
    fn the_card_anchor_moves_the_card_and_asks_for_one_frame() {
        let mut tank = settled_planted_tank();
        let guppy = Card::Creature(tank.world().species().id("guppy").unwrap(), None);
        tank.set_card(Some(guppy));
        let target = (1280, 720);
        // Top-left by default.
        assert_eq!(tank.card_anchor(), CardAnchor::Top);
        let top = tank.card_layout(target).unwrap();
        assert_eq!(top.origin, (2 * top.scale, 2 * top.scale));
        assert_eq!(count_frames(&mut tank, Duration::from_secs(2), 2), 1);

        // Another anchor is one change: one frame, and the layout (which
        // hit tests and drawing share) is now bottom-left.
        tank.set_card_anchor(CardAnchor::Bottom);
        assert_eq!(tank.card_anchor(), CardAnchor::Bottom);
        let bottom = tank.card_layout(target).unwrap();
        assert_eq!(bottom.scale, top.scale);
        assert_eq!(bottom.origin.0, 2 * bottom.scale);
        assert_eq!(
            bottom.origin.1 + bottom.size().1 + 2 * bottom.scale,
            target.1
        );
        assert_eq!(count_frames(&mut tank, Duration::from_secs(4), 2), 1);

        // The same anchor again is free.
        tank.set_card_anchor(CardAnchor::Bottom);
        assert_eq!(count_frames(&mut tank, Duration::from_secs(6), 2), 0);

        // A palette in the canvas still puts the card below it.
        tank.set_palette(Some(Palette {
            tabs: Vec::new(),
            open: 0,
            subs: Vec::new(),
            open_sub: 0,
            items: Vec::new(),
            selected: None,
            morphs: None,
        }));
        let palette = tank.palette_layout(target).unwrap();
        let below = tank.card_layout(target).unwrap();
        assert!(below.origin.1 >= palette.origin.1 + palette.size().1);
    }

    #[test]
    fn the_card_sits_below_the_palette_when_there_is_one() {
        let mut tank = settled_planted_tank();
        let guppy = Card::Creature(tank.world().species().id("guppy").unwrap(), None);
        tank.set_card(Some(guppy));
        let alone = tank.card_layout((1280, 720)).unwrap();
        tank.set_palette(Some(Palette {
            tabs: Vec::new(),
            open: 0,
            subs: Vec::new(),
            open_sub: 0,
            items: Vec::new(),
            selected: None,
            morphs: None,
        }));
        let below = tank.card_layout((1280, 720)).unwrap();
        let palette = tank.palette_layout((1280, 720)).unwrap();
        assert!(below.origin.1 >= palette.origin.1 + palette.size().1);
        assert!(below.origin.1 > alone.origin.1);
    }

    #[test]
    fn settled_tank_draws_only_when_magnifier_changes() {
        let mut tank = settled_planted_tank();
        assert_eq!(tank.magnifier(), None);
        // Setting None again asks for no frame.
        tank.set_magnifier(None);
        assert_eq!(count_frames(&mut tank, Duration::ZERO, 2), 0);

        // Turning the magnifier on asks for one frame, then 0 while still.
        let m = Magnifier {
            cell: (32, 30),
            zoom: 4,
        };
        tank.set_magnifier(Some(m));
        assert_eq!(tank.magnifier(), Some(m));
        assert_eq!(count_frames(&mut tank, Duration::from_secs(2), 2), 1);
        tank.set_magnifier(Some(m));
        assert_eq!(count_frames(&mut tank, Duration::from_secs(4), 2), 0);

        // Moving to a new cell or changing zoom asks for one frame each.
        tank.set_magnifier(Some(Magnifier {
            cell: (33, 30),
            zoom: 4,
        }));
        assert_eq!(count_frames(&mut tank, Duration::from_secs(6), 1), 1);
        tank.set_magnifier(Some(Magnifier {
            cell: (33, 30),
            zoom: 8,
        }));
        assert_eq!(count_frames(&mut tank, Duration::from_secs(7), 1), 1);

        // Turning it off asks for one frame, then 0.
        tank.set_magnifier(None);
        assert_eq!(count_frames(&mut tank, Duration::from_secs(8), 2), 1);
    }

    #[test]
    fn settled_hob_tank_draws_no_extra_frames_when_idle() {
        let mut world = World::new(
            WorldConfig {
                width: 96,
                height: 64,
                seed: 7,
                water_level: 0.86,
            },
            MaterialTable::builtin().unwrap(),
        )
        .unwrap();
        world
            .add_device(desktank_sim::DeviceKind::HobFilter, 48, 0, false)
            .unwrap()
            .expect("HOB fits");
        assert!(world.has_surface_ripples());
        for _ in 0..120 {
            world.tick();
        }
        assert_eq!(world.awake_chunks(), 0, "HOB tank settled");
        let mut tank = Tank::new(world, TankConfig::default());
        let req = tank.tick(Duration::ZERO);
        assert!(req.redraw);
        tank.mark_drawn();
        let frames = count_frames(&mut tank, Duration::ZERO, 10);
        assert_eq!(
            frames, 0,
            "idle tank with HOB filter must draw 0 extra frames, drew {frames}"
        );
    }

    #[test]
    fn settled_surface_current_tank_draws_no_extra_frames_when_idle() {
        let mut world = World::new(
            WorldConfig {
                width: 128,
                height: 64,
                seed: 13,
                water_level: 0.85,
            },
            MaterialTable::builtin().unwrap(),
        )
        .unwrap();
        let surface = i64::try_from(world.surface_row()).unwrap();
        world
            .add_device(desktank_sim::DeviceKind::InternalFilter, 0, surface, false)
            .unwrap()
            .expect("high internal filter fits");
        for _ in 0..120 {
            world.tick();
        }
        assert!(world.has_surface_current_ripples());
        assert_eq!(world.awake_chunks(), 0, "high filter tank settled");
        let mut tank = Tank::new(world, TankConfig::default());
        let req = tank.tick(Duration::ZERO);
        assert!(req.redraw);
        tank.mark_drawn();
        let frames = count_frames(&mut tank, Duration::ZERO, 10);
        assert_eq!(
            frames, 0,
            "idle tank with strong surface current must draw 0 extra frames, drew {frames}"
        );
    }

    /// An empty tank at `ratio`, its first frame drawn.
    fn clock_tank(ratio: f32) -> Tank {
        let mut world = World::new(
            WorldConfig {
                width: 32,
                height: 24,
                seed: 5,
                water_level: 0.8,
            },
            MaterialTable::builtin().unwrap(),
        )
        .unwrap();
        world.set_time_ratio(ratio);
        let mut tank = Tank::new(world, TankConfig::default());
        tank.mark_drawn();
        tank
    }

    /// Steps `tank` through `minutes` of life time at a ratio of 60 (1200
    /// life ticks per minute, 60 a step).
    fn step_minutes_at_60(tank: &mut Tank, minutes: u32) {
        for _ in 0..minutes * 20 {
            tank.step();
        }
    }

    #[test]
    fn update_local_time_applies_directly_at_the_default_ratio() {
        let mut tank = clock_tank(1.0);
        tank.update_local_time(600);
        assert_eq!(tank.time_of_day(), 600);
        for _ in 0..100 {
            tank.step();
        }
        assert_eq!(tank.time_of_day(), 600, "ratio 1 leaves the clock alone");
        tank.update_local_time(601);
        assert_eq!(tank.time_of_day(), 601, "the host's minute always wins");
        tank.update_local_time(601 + daylight::MINUTES_PER_DAY);
        assert_eq!(tank.time_of_day(), 601, "wrapped to a day");
    }

    #[test]
    fn a_scaled_clock_is_seeded_once_then_runs_on_its_own() {
        let mut tank = clock_tank(60.0);
        tank.update_local_time(600);
        assert_eq!(tank.time_of_day(), 600);
        step_minutes_at_60(&mut tank, 5);
        assert_eq!(tank.time_of_day(), 605);
        assert_eq!(tank.world().time_of_day(), 605, "the world follows");
        // The host's wall clock no longer snaps the scaled day back.
        tank.update_local_time(900);
        assert_eq!(tank.time_of_day(), 605);
        step_minutes_at_60(&mut tank, 1);
        assert_eq!(tank.time_of_day(), 606);
    }

    #[test]
    fn the_scaled_clock_wraps_at_midnight() {
        let mut tank = clock_tank(60.0);
        tank.update_local_time(daylight::MINUTES_PER_DAY - 2);
        step_minutes_at_60(&mut tank, 3);
        assert_eq!(tank.time_of_day(), 1);
    }

    #[test]
    fn the_scaled_clock_does_not_run_until_seeded() {
        let mut tank = clock_tank(60.0);
        step_minutes_at_60(&mut tank, 5);
        assert_eq!(tank.time_of_day(), daylight::NOON);
    }

    #[test]
    fn a_scaled_clock_requests_no_frames_on_a_daylight_plateau() {
        let mut tank = clock_tank(60.0);
        tank.update_local_time(600);
        tank.mark_drawn();
        step_minutes_at_60(&mut tank, 30);
        assert_eq!(tank.time_of_day(), 630);
        assert!(!tank.needs_frame, "same tint, no frame wanted");
    }

    #[test]
    fn a_scaled_clock_requests_a_frame_when_the_tint_changes() {
        let mut tank = clock_tank(60.0);
        tank.update_local_time(1169);
        tank.mark_drawn();
        step_minutes_at_60(&mut tank, 3);
        assert!(tank.needs_frame, "dusk has begun");
    }

    #[test]
    fn a_scaled_clock_stays_put_when_not_stepped() {
        // Hidden or paused hosts don't call `step`, so the day stands still.
        let mut tank = clock_tank(60.0);
        tank.update_local_time(600);
        tank.pause();
        tank.update_local_time(700);
        assert_eq!(tank.time_of_day(), 600);
    }

    #[test]
    fn catch_up_advances_a_running_scaled_clock() {
        let mut tank = clock_tank(60.0);
        tank.update_local_time(600);
        let report = tank.catch_up(120);
        assert_eq!(report.seconds, 120);
        // 120 s at ratio 60 is 2 hours.
        assert_eq!(tank.time_of_day(), 720);
        assert_eq!(tank.world().time_of_day(), 720);
    }

    #[test]
    fn catch_up_leaves_the_clock_alone_at_the_default_ratio_or_unseeded() {
        let mut tank = clock_tank(1.0);
        tank.update_local_time(600);
        tank.catch_up(3600);
        assert_eq!(tank.time_of_day(), 600);

        let mut tank = clock_tank(60.0);
        tank.catch_up(120);
        assert_eq!(tank.time_of_day(), daylight::NOON, "not seeded yet");
    }

    #[test]
    fn location_sync_keeps_day_and_night_on_the_wall_clock() {
        let mut tank = clock_tank(60.0);
        assert!(!tank.location_sync());
        tank.set_location_sync(true);
        assert!(tank.location_sync());
        tank.update_local_time(600);
        step_minutes_at_60(&mut tank, 5);
        assert_eq!(tank.time_of_day(), 600, "the scaled clock is off");
        tank.catch_up(120);
        assert_eq!(tank.time_of_day(), 600);
        tank.update_local_time(660);
        assert_eq!(tank.time_of_day(), 660, "the wall clock applies directly");
    }

    #[test]
    fn enabling_location_sync_stops_a_running_clock_and_snaps_to_the_wall_clock() {
        let mut tank = clock_tank(60.0);
        tank.update_local_time(600);
        step_minutes_at_60(&mut tank, 5);
        assert_eq!(tank.time_of_day(), 605);
        tank.set_location_sync(true);
        step_minutes_at_60(&mut tank, 5);
        assert_eq!(tank.time_of_day(), 605);
        tank.update_local_time(800);
        assert_eq!(tank.time_of_day(), 800);
        // Turning it off reseeds from the next injected minute.
        tank.set_location_sync(false);
        tank.update_local_time(820);
        assert_eq!(tank.time_of_day(), 820);
        step_minutes_at_60(&mut tank, 2);
        assert_eq!(tank.time_of_day(), 822);
    }

    #[test]
    fn set_daylight_schedule_updates_the_world_and_asks_for_a_frame_on_a_tint_change() {
        let mut tank = clock_tank(1.0);
        assert_eq!(tank.daylight_schedule(), daylight::DaylightSchedule::FIXED);
        // 10:00 is full day under the fixed schedule.
        tank.update_local_time(600);
        tank.mark_drawn();
        let same_day = daylight::DaylightSchedule::from_sun_times(5 * 60, 22 * 60);
        tank.set_daylight_schedule(same_day);
        assert_eq!(tank.daylight_schedule(), same_day);
        assert_eq!(tank.world().daylight_schedule(), same_day);
        assert!(!tank.needs_frame, "10:00 is day under both schedules");
        // Sunrise at 10:00 puts the tank at dawn.
        let late = daylight::DaylightSchedule::from_sun_times(600, 22 * 60);
        tank.set_daylight_schedule(late);
        assert!(tank.needs_frame, "the tint changed at 10:00");
        assert_eq!(tank.world().daylight_schedule(), late);
        tank.mark_drawn();
        tank.set_daylight_schedule(late);
        assert!(!tank.needs_frame, "same schedule, no frame");
    }

    #[test]
    fn apply_minute_follows_the_active_schedule() {
        let mut tank = clock_tank(1.0);
        tank.update_local_time(7 * 60);
        tank.mark_drawn();
        tank.set_daylight_schedule(daylight::DaylightSchedule::from_sun_times(4 * 60, 22 * 60));
        tank.mark_drawn();
        // With a 04:00 sunrise 07:00 and 07:30 are both full day, while the
        // fixed schedule is still at dawn at 06:50.
        tank.update_local_time(7 * 60 + 30);
        assert!(!tank.needs_frame, "day to day under this schedule");
        tank.set_daylight_schedule(daylight::DaylightSchedule::FIXED);
        tank.mark_drawn();
        tank.update_local_time(6 * 60 + 50);
        assert!(tank.needs_frame, "the fixed schedule is at dawn at 06:50");
    }

    #[test]
    fn set_time_of_day_pins_the_time_and_restarts_seeding() {
        let mut tank = clock_tank(60.0);
        tank.update_local_time(600);
        step_minutes_at_60(&mut tank, 5);
        tank.set_time_of_day(300);
        assert_eq!(tank.time_of_day(), 300);
        step_minutes_at_60(&mut tank, 5);
        assert_eq!(tank.time_of_day(), 300, "pinned: no scaled advance");
        tank.catch_up(120);
        assert_eq!(tank.time_of_day(), 300);
        // The next injected minute seeds a fresh scaled day.
        tank.update_local_time(1000);
        assert_eq!(tank.time_of_day(), 1000);
    }

    #[test]
    fn set_time_of_day_requests_a_frame_only_when_the_tint_changes() {
        let mut tank = clock_tank(1.0);
        tank.set_time_of_day(600);
        tank.mark_drawn();
        tank.set_time_of_day(700);
        assert!(!tank.needs_frame);
        tank.set_time_of_day(0);
        assert!(tank.needs_frame);
    }

    #[test]
    fn a_narrow_target_lays_the_need_meters_in_one_column_and_a_change_is_one_rebuild() {
        let mut tank = settled_planted_tank();
        let tetra = tank.world().species().id("neon_tetra").unwrap();
        let id = tank.world_mut().add_creature(tetra, 30, 20).unwrap();
        tank.set_inspected(Some(Inspected::Creature(id)));
        let height = |tank: &Tank| tank.inspect.image.as_ref().unwrap().size.1;
        tank.inspect.dirty = false;
        tank.fit_inspect((1280, 720));
        assert!(!tank.inspect.dirty, "a wide target changes nothing");
        let wide = height(&tank);
        tank.fit_inspect((NARROW_TARGET_W - 1, 640));
        assert!(tank.inspect.dirty && height(&tank) > wide);
        tank.inspect.dirty = false;
        tank.fit_inspect((NARROW_TARGET_W - 1, 480));
        assert!(!tank.inspect.dirty, "the same layout is free");
        tank.fit_inspect((NARROW_TARGET_W, 480));
        assert!(tank.inspect.dirty && height(&tank) == wide);
        // Nothing inspected: the layout is only remembered, nothing built.
        tank.set_inspected(None);
        tank.inspect.dirty = false;
        tank.fit_inspect((200, 200));
        assert!(tank.inspect.image.is_none() && !tank.inspect.dirty);
    }
}
