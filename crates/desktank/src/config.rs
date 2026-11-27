//! Versioned settings for desktop display modes.

use std::fs;
use std::io;
use std::path::Path;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::Duration;

use desktank_platform::{Coordinates, LocalDateTime, PowerPolicy};
use desktank_sim::daylight::DaylightSchedule;
use desktank_sim::{MAX_TIME_RATIO, MIN_TIME_RATIO};
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use serde::Deserialize;

pub const FILE_NAME: &str = "config.toml";
const VERSION: u32 = 1;
const DEBOUNCE: Duration = Duration::from_millis(200);

#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    version: u32,
    #[serde(with = "PowerFields")]
    pub power: PowerPolicy,
    pub plants: Plants,
    pub life: Life,
    pub overlay: Overlay,
    pub daylight: Daylight,
    pub display: Display,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            version: VERSION,
            power: PowerPolicy::default(),
            plants: Plants::default(),
            life: Life::default(),
            overlay: Overlay::default(),
            daylight: Daylight::default(),
            display: Display::default(),
        }
    }
}

// Keep the TOML schema in the binary and use the platform policy's defaults.
#[derive(Deserialize)]
#[serde(
    remote = "PowerPolicy",
    default = "PowerPolicy::default",
    deny_unknown_fields
)]
struct PowerFields {
    ac_fps: u32,
    battery_fps: u32,
    min_battery_percent: u8,
    pause_in_low_power_mode: bool,
    allow_over_lock: bool,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct Plants {
    pub sway: bool,
    pub sway_on_battery: bool,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Life {
    pub easy_mode: Option<bool>,
    pub population_cap: Option<u16>,
    pub catch_up: bool,
    /// Life and day/night clock multiplier (1 is real time).
    pub time_ratio: Option<f32>,
}

/// The `[display]` table.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct Display {
    /// OLED mode: the opaque tank's water, air and letterbox are true black
    /// (sRGB 0,0,0). The click-through overlay ignores it.
    pub oled: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Overlay {
    pub tint: f32,
    pub exclude_from_capture: bool,
}

impl Default for Overlay {
    fn default() -> Self {
        Self {
            tint: 0.0,
            exclude_from_capture: true,
        }
    }
}

/// Where the day's sunrise and sunset come from.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum DaylightMode {
    /// The fixed schedule: sunrise 06:30, sunset 20:30.
    #[default]
    Fixed,
    /// Sunrise and sunset where the device is: from the OS's location, or
    /// `latitude` and `longitude` when it has none.
    Location,
}

/// The `[daylight]` table (SPEC §2.6).
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Daylight {
    pub schedule: DaylightMode,
    /// Degrees north, for when the OS can't say where the device is.
    pub latitude: Option<f64>,
    /// Degrees east, for when the OS can't say where the device is.
    pub longitude: Option<f64>,
}

impl Daylight {
    /// Whether day and night follow the sun at the device's location.
    pub fn location_sync(&self) -> bool {
        self.schedule == DaylightMode::Location
    }

    /// The coordinates from `latitude` and `longitude`, if both are set.
    pub fn manual_coordinates(&self) -> Option<Coordinates> {
        Some(Coordinates {
            latitude: self.latitude?,
            longitude: self.longitude?,
        })
    }

    /// The day's schedule at `now` for `coordinates`: the sun's, when synced
    /// to the location and both are known, otherwise the fixed schedule.
    pub fn schedule_at(
        &self,
        now: Option<LocalDateTime>,
        coordinates: Option<Coordinates>,
    ) -> DaylightSchedule {
        match (self.location_sync(), now, coordinates) {
            (true, Some(now), Some(place)) => DaylightSchedule::solar_schedule(
                now.day_of_year,
                place.latitude,
                place.longitude,
                now.utc_offset_minutes,
            ),
            _ => DaylightSchedule::FIXED,
        }
    }
}

/// Whether `degrees` is a latitude: finite, in `-90.0..=90.0`.
pub fn is_latitude(degrees: f64) -> bool {
    degrees.is_finite() && (-90.0..=90.0).contains(&degrees)
}

/// Whether `degrees` is a longitude: finite, in `-180.0..=180.0`.
pub fn is_longitude(degrees: f64) -> bool {
    degrees.is_finite() && (-180.0..=180.0).contains(&degrees)
}

#[derive(Clone, Copy, Debug, Default)]
pub struct CliOverrides {
    pub plants: Plants,
    pub easy_mode: Option<bool>,
    pub population_cap: Option<u16>,
    pub catch_up: Option<bool>,
    pub time_ratio: Option<f32>,
    pub daylight_schedule: Option<DaylightMode>,
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
    pub oled: Option<bool>,
}

pub fn merge(mut config: Config, cli: CliOverrides) -> Config {
    config.plants.sway |= cli.plants.sway;
    config.plants.sway_on_battery |= cli.plants.sway_on_battery;
    config.life.easy_mode = cli.easy_mode.or(config.life.easy_mode);
    config.life.population_cap = cli.population_cap.or(config.life.population_cap);
    config.life.catch_up = cli.catch_up.unwrap_or(config.life.catch_up);
    config.life.time_ratio = cli.time_ratio.or(config.life.time_ratio);
    config.daylight.schedule = cli.daylight_schedule.unwrap_or(config.daylight.schedule);
    config.daylight.latitude = cli.latitude.or(config.daylight.latitude);
    config.daylight.longitude = cli.longitude.or(config.daylight.longitude);
    config.display.oled = cli.oled.unwrap_or(config.display.oled);
    config
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("reading file: {0}")]
    Read(#[from] io::Error),
    #[error("invalid TOML: {0}")]
    Toml(#[from] toml::de::Error),
    #[error("unsupported config version {0}; expected {VERSION}")]
    Version(u32),
    #[error("{0}")]
    Value(&'static str),
}

pub fn parse(text: &str) -> Result<Config, ConfigError> {
    let config: Config = toml::from_str(text)?;
    if config.version != VERSION {
        return Err(ConfigError::Version(config.version));
    }
    let power = config.power;
    if !(1..=120).contains(&power.ac_fps) {
        return Err(ConfigError::Value("power.ac_fps must be in 1..=120"));
    }
    if !(1..=power.ac_fps).contains(&power.battery_fps) {
        return Err(ConfigError::Value(
            "power.battery_fps must be in 1..=120 and no greater than power.ac_fps",
        ));
    }
    if power.min_battery_percent > 100 {
        return Err(ConfigError::Value(
            "power.min_battery_percent must be in 0..=100",
        ));
    }
    if config
        .life
        .population_cap
        .is_some_and(|cap| !(1..=128).contains(&cap))
    {
        return Err(ConfigError::Value("life.population_cap must be in 1..=128"));
    }
    if config
        .life
        .time_ratio
        .is_some_and(|r| !(r.is_finite() && (MIN_TIME_RATIO..=MAX_TIME_RATIO).contains(&r)))
    {
        return Err(ConfigError::Value("life.time_ratio must be in 0.1..=100.0"));
    }
    if !(config.overlay.tint.is_finite() && (0.0..=1.0).contains(&config.overlay.tint)) {
        return Err(ConfigError::Value("overlay.tint must be in 0.0..=1.0"));
    }
    let daylight = config.daylight;
    if daylight.latitude.is_some() != daylight.longitude.is_some() {
        return Err(ConfigError::Value(
            "daylight.latitude and daylight.longitude must be set together",
        ));
    }
    if daylight
        .latitude
        .is_some_and(|degrees| !is_latitude(degrees))
    {
        return Err(ConfigError::Value(
            "daylight.latitude must be in -90.0..=90.0",
        ));
    }
    if daylight
        .longitude
        .is_some_and(|degrees| !is_longitude(degrees))
    {
        return Err(ConfigError::Value(
            "daylight.longitude must be in -180.0..=180.0",
        ));
    }
    Ok(config)
}

pub fn load(path: &Path) -> Result<Option<Config>, ConfigError> {
    match fs::read_to_string(path) {
        Ok(text) => parse(&text).map(Some),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

/// Watch the directory so replacing the file with a rename is detected too.
/// Dropping the watcher closes the channel and stops the debounce thread.
pub fn watch(
    dir: &Path,
    wake: impl Fn() -> bool + Send + 'static,
) -> anyhow::Result<RecommendedWatcher> {
    fs::create_dir_all(dir)?;
    let (tx, rx) = mpsc::channel();
    let mut watcher = RecommendedWatcher::new(
        move |result: notify::Result<notify::Event>| match result {
            Ok(event)
                if event.need_rescan()
                    || (!matches!(event.kind, EventKind::Access(_))
                        && event.paths.iter().any(|path| {
                            path.file_name().is_some_and(|name| name == FILE_NAME)
                        })) =>
            {
                let _ = tx.send(());
            }
            Ok(_) => {}
            Err(error) => eprintln!("desktank: config watcher error: {error}"),
        },
        notify::Config::default(),
    )?;
    watcher.watch(dir, RecursiveMode::NonRecursive)?;
    thread::Builder::new()
        .name("desktank-config".into())
        .spawn(move || {
            while rx.recv().is_ok() {
                loop {
                    match rx.recv_timeout(DEBOUNCE) {
                        Ok(()) => {}
                        Err(RecvTimeoutError::Timeout) if wake() => break,
                        Err(_) => return,
                    }
                }
            }
        })?;
    Ok(watcher)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_uses_defaults() {
        let defaults = parse("").unwrap();
        assert_eq!(defaults, Config::default());
        assert_eq!(defaults.version, 1);
        assert_eq!(defaults.power, PowerPolicy::default());
        assert_eq!(defaults.plants, Plants::default());
        assert_eq!(defaults.life, Life::default());
        assert!(!defaults.life.catch_up);
        assert_eq!(defaults.life.time_ratio, None);
        let overlay = Overlay {
            tint: 0.0,
            exclude_from_capture: true,
        };
        assert_eq!(defaults.overlay, overlay);
        assert_eq!(defaults.daylight, Daylight::default());
        assert_eq!(defaults.daylight.schedule, DaylightMode::Fixed);
        assert_eq!(defaults.daylight.manual_coordinates(), None);
        assert!(!defaults.daylight.location_sync());
        assert_eq!(parse("[daylight]").unwrap().daylight, Daylight::default());
        assert_eq!(parse("[overlay]").unwrap().overlay, overlay);
        assert_eq!(defaults.display, Display { oled: false });
        assert_eq!(parse("[display]").unwrap().display, Display::default());
        assert_eq!(
            parse("[power]\nac_fps = 60").unwrap().power.battery_fps,
            defaults.power.battery_fps
        );
    }

    #[test]
    fn parses_full_config() {
        let config = parse(
            r"
            version = 1
            [power]
            ac_fps = 60
            battery_fps = 24
            min_battery_percent = 10
            pause_in_low_power_mode = false
            allow_over_lock = true
            [plants]
            sway = true
            sway_on_battery = true
            [life]
            easy_mode = true
            population_cap = 42
            catch_up = true
            time_ratio = 12.5
            [overlay]
            tint = 0.25
            exclude_from_capture = false
            [daylight]
            schedule = 'location'
            latitude = 51.5
            longitude = -0.12
            [display]
            oled = true
        ",
        )
        .unwrap();
        assert_eq!(
            config.daylight,
            Daylight {
                schedule: DaylightMode::Location,
                latitude: Some(51.5),
                longitude: Some(-0.12),
            }
        );
        assert_eq!(config.display, Display { oled: true });
        assert_eq!(
            config.power,
            PowerPolicy {
                ac_fps: 60,
                battery_fps: 24,
                min_battery_percent: 10,
                pause_in_low_power_mode: false,
                allow_over_lock: true,
            }
        );
        assert_eq!(
            config.plants,
            Plants {
                sway: true,
                sway_on_battery: true
            }
        );
        assert_eq!(
            config.life,
            Life {
                easy_mode: Some(true),
                population_cap: Some(42),
                catch_up: true,
                time_ratio: Some(12.5),
            }
        );
        assert_eq!(
            config.overlay,
            Overlay {
                tint: 0.25,
                exclude_from_capture: false,
            }
        );
    }

    #[test]
    fn rejects_bad_toml_and_unknown_keys() {
        for text in [
            "[power",
            "extra = 1",
            "[power]\nextra = 1",
            "[plants]\nextra = 1",
            "[life]\nextra = 1",
            "[overlay]\nextra = 1",
            "[daylight]\nextra = 1",
            "[display]\nextra = 1",
            "[display]\noled = \"yes\"",
            "[daylight]\nschedule = \"sunny\"",
            "[daylight]\nschedule = \"Location\"",
        ] {
            assert!(matches!(parse(text), Err(ConfigError::Toml(_))), "{text}");
        }
    }

    #[test]
    fn rejects_unsupported_version() {
        assert!(matches!(parse("version = 2"), Err(ConfigError::Version(2))));
    }

    #[test]
    fn rejects_out_of_range_values() {
        for text in [
            "[power]\nac_fps = 0",
            "[power]\nac_fps = 121",
            "[power]\nbattery_fps = 0",
            "[power]\nbattery_fps = 31",
            "[power]\nmin_battery_percent = 101",
            "[life]\npopulation_cap = 0",
            "[life]\npopulation_cap = 129",
        ] {
            assert!(matches!(parse(text), Err(ConfigError::Value(_))), "{text}");
        }
    }

    #[test]
    fn time_ratio_accepts_the_supported_range() {
        for (text, expected) in [("0.1", 0.1), ("1.0", 1.0), ("60", 60.0), ("100.0", 100.0)] {
            let config = parse(&format!("[life]\ntime_ratio = {text}")).unwrap();
            assert_eq!(config.life.time_ratio, Some(expected), "{text}");
        }
        assert_eq!(
            parse("[life]\ncatch_up = true").unwrap().life.time_ratio,
            None
        );
    }

    #[test]
    fn rejects_invalid_time_ratio() {
        for ratio in ["0.0", "0.09", "-1.0", "100.1", "nan", "inf", "-inf"] {
            assert!(
                matches!(
                    parse(&format!("[life]\ntime_ratio = {ratio}")),
                    Err(ConfigError::Value("life.time_ratio must be in 0.1..=100.0"))
                ),
                "{ratio}"
            );
        }
    }

    #[test]
    fn cli_time_ratio_overrides_the_file() {
        let config = parse("[life]\ntime_ratio = 5.0").unwrap();
        assert_eq!(
            merge(config, CliOverrides::default()).life.time_ratio,
            Some(5.0)
        );
        let effective = merge(
            config,
            CliOverrides {
                time_ratio: Some(60.0),
                ..CliOverrides::default()
            },
        );
        assert_eq!(effective.life.time_ratio, Some(60.0));
        let from_cli = merge(
            Config::default(),
            CliOverrides {
                time_ratio: Some(2.0),
                ..CliOverrides::default()
            },
        );
        assert_eq!(from_cli.life.time_ratio, Some(2.0));
    }

    #[test]
    fn rejects_invalid_overlay_tint() {
        for tint in ["-0.1", "1.1", "nan", "inf", "-inf"] {
            assert!(
                matches!(
                    parse(&format!("[overlay]\ntint = {tint}")),
                    Err(ConfigError::Value("overlay.tint must be in 0.0..=1.0"))
                ),
                "{tint}"
            );
        }
        assert!(parse("[overlay]\ntint = 1.0").is_ok());
    }

    #[test]
    fn cli_overrides_only_explicit_values() {
        let config = parse(
            "[plants]\nsway = true\n[life]\neasy_mode = true\npopulation_cap = 42\ncatch_up = false",
        )
        .unwrap();
        assert_eq!(merge(config, CliOverrides::default()), config);
        let effective = merge(
            config,
            CliOverrides {
                plants: Plants {
                    sway: false,
                    sway_on_battery: true,
                },
                easy_mode: Some(false),
                population_cap: Some(30),
                catch_up: Some(true),
                time_ratio: None,
                ..CliOverrides::default()
            },
        );
        assert_eq!(
            effective.plants,
            Plants {
                sway: true,
                sway_on_battery: true
            }
        );
        assert_eq!(
            effective.life,
            Life {
                easy_mode: Some(false),
                population_cap: Some(30),
                catch_up: true,
                time_ratio: None,
            }
        );
        assert!(
            !merge(
                effective,
                CliOverrides {
                    catch_up: Some(false),
                    ..CliOverrides::default()
                }
            )
            .life
            .catch_up
        );
        assert_eq!(effective.power, config.power);
    }

    #[test]
    fn daylight_schedule_parses_both_modes_and_manual_coordinates() {
        let daylight = |text: &str| parse(text).unwrap().daylight;
        assert_eq!(
            daylight("[daylight]\nschedule = \"fixed\"").schedule,
            DaylightMode::Fixed
        );
        let location = daylight("[daylight]\nschedule = \"location\"");
        assert_eq!(location.schedule, DaylightMode::Location);
        assert!(location.location_sync());
        assert_eq!(location.manual_coordinates(), None);
        // Whole degrees are fine for a TOML float.
        let manual = daylight("[daylight]\nlatitude = 51\nlongitude = -90.5");
        assert_eq!(
            manual.manual_coordinates(),
            Some(Coordinates {
                latitude: 51.0,
                longitude: -90.5
            })
        );
        assert_eq!(manual.schedule, DaylightMode::Fixed);
        for (lat, lon) in [("90", "180"), ("-90", "-180"), ("0", "0")] {
            let text = format!("[daylight]\nlatitude = {lat}\nlongitude = {lon}");
            assert!(parse(&text).is_ok(), "{text}");
        }
    }

    #[test]
    fn daylight_coordinates_must_be_set_together() {
        for text in [
            "[daylight]\nlatitude = 51.5",
            "[daylight]\nlongitude = 0.0",
            "[daylight]\nschedule = \"location\"\nlatitude = 51.5",
        ] {
            assert!(
                matches!(
                    parse(text),
                    Err(ConfigError::Value(
                        "daylight.latitude and daylight.longitude must be set together"
                    ))
                ),
                "{text}"
            );
        }
    }

    #[test]
    fn daylight_coordinates_must_be_real_places() {
        for lat in ["-90.1", "90.1", "nan", "inf", "-inf"] {
            assert!(
                matches!(
                    parse(&format!("[daylight]\nlatitude = {lat}\nlongitude = 0.0")),
                    Err(ConfigError::Value(
                        "daylight.latitude must be in -90.0..=90.0"
                    ))
                ),
                "{lat}"
            );
        }
        for lon in ["-180.1", "180.1", "nan", "inf", "-inf"] {
            assert!(
                matches!(
                    parse(&format!("[daylight]\nlatitude = 0.0\nlongitude = {lon}")),
                    Err(ConfigError::Value(
                        "daylight.longitude must be in -180.0..=180.0"
                    ))
                ),
                "{lon}"
            );
        }
    }

    #[test]
    fn cli_oled_overrides_the_file() {
        let config = parse("[display]\noled = true").unwrap();
        assert!(config.display.oled);
        assert!(merge(config, CliOverrides::default()).display.oled);
        assert!(
            !merge(
                config,
                CliOverrides {
                    oled: Some(false),
                    ..CliOverrides::default()
                }
            )
            .display
            .oled
        );
        assert!(
            merge(
                Config::default(),
                CliOverrides {
                    oled: Some(true),
                    ..CliOverrides::default()
                }
            )
            .display
            .oled
        );
    }

    #[test]
    fn cli_daylight_overrides_the_file() {
        let config =
            parse("[daylight]\nschedule = \"location\"\nlatitude = 51.5\nlongitude = -0.12")
                .unwrap();
        assert_eq!(merge(config, CliOverrides::default()), config);
        let effective = merge(
            config,
            CliOverrides {
                daylight_schedule: Some(DaylightMode::Fixed),
                latitude: Some(-33.9),
                longitude: Some(151.2),
                ..CliOverrides::default()
            },
        );
        assert_eq!(
            effective.daylight,
            Daylight {
                schedule: DaylightMode::Fixed,
                latitude: Some(-33.9),
                longitude: Some(151.2),
            }
        );
        let from_cli = merge(
            Config::default(),
            CliOverrides {
                daylight_schedule: Some(DaylightMode::Location),
                latitude: Some(1.0),
                longitude: Some(2.0),
                ..CliOverrides::default()
            },
        );
        assert!(from_cli.daylight.location_sync());
        assert_eq!(
            from_cli.daylight.manual_coordinates(),
            Some(Coordinates {
                latitude: 1.0,
                longitude: 2.0
            })
        );
        // Only the schedule given on the command line changes.
        assert_eq!(effective.life, config.life);
        assert_eq!(effective.overlay, config.overlay);
    }

    #[test]
    fn schedule_at_follows_the_sun_only_when_synced_and_located() {
        let london = Coordinates {
            latitude: 51.5,
            longitude: -0.12,
        };
        let summer = LocalDateTime {
            minute: 720,
            day_of_year: 172,
            utc_offset_minutes: 60,
        };
        let winter = LocalDateTime {
            day_of_year: 355,
            utc_offset_minutes: 0,
            ..summer
        };
        let synced = Daylight {
            schedule: DaylightMode::Location,
            ..Daylight::default()
        };
        let fixed = Daylight::default();
        assert_eq!(
            fixed.schedule_at(Some(summer), Some(london)),
            DaylightSchedule::FIXED,
            "fixed ignores the sun"
        );
        assert_eq!(
            synced.schedule_at(Some(summer), None),
            DaylightSchedule::FIXED,
            "no location"
        );
        assert_eq!(
            synced.schedule_at(None, Some(london)),
            DaylightSchedule::FIXED,
            "no date"
        );
        let june = synced.schedule_at(Some(summer), Some(london));
        let december = synced.schedule_at(Some(winter), Some(london));
        assert_ne!(june, DaylightSchedule::FIXED);
        assert!(
            june.dawn_peak < december.dawn_peak && june.dusk_peak > december.dusk_peak,
            "London's June day is longer: {june:?} {december:?}"
        );
    }

    #[test]
    fn latitude_and_longitude_checks_reject_non_finite_values() {
        assert!(is_latitude(-90.0) && is_latitude(90.0) && !is_latitude(90.5));
        assert!(is_longitude(-180.0) && is_longitude(180.0) && !is_longitude(-180.5));
        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert!(!is_latitude(bad) && !is_longitude(bad));
        }
    }
}
