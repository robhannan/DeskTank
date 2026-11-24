//! Versioned settings for desktop display modes.

use std::fs;
use std::io;
use std::path::Path;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::Duration;

use desktank_platform::PowerPolicy;
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
}

impl Default for Config {
    fn default() -> Self {
        Self {
            version: VERSION,
            power: PowerPolicy::default(),
            plants: Plants::default(),
            life: Life::default(),
            overlay: Overlay::default(),
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

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct Life {
    pub easy_mode: Option<bool>,
    pub population_cap: Option<u16>,
    pub catch_up: bool,
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

#[derive(Clone, Copy, Debug, Default)]
pub struct CliOverrides {
    pub plants: Plants,
    pub easy_mode: Option<bool>,
    pub population_cap: Option<u16>,
    pub catch_up: Option<bool>,
}

pub fn merge(mut config: Config, cli: CliOverrides) -> Config {
    config.plants.sway |= cli.plants.sway;
    config.plants.sway_on_battery |= cli.plants.sway_on_battery;
    config.life.easy_mode = cli.easy_mode.or(config.life.easy_mode);
    config.life.population_cap = cli.population_cap.or(config.life.population_cap);
    config.life.catch_up = cli.catch_up.unwrap_or(config.life.catch_up);
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
    if !(config.overlay.tint.is_finite() && (0.0..=1.0).contains(&config.overlay.tint)) {
        return Err(ConfigError::Value("overlay.tint must be in 0.0..=1.0"));
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
        let overlay = Overlay {
            tint: 0.0,
            exclude_from_capture: true,
        };
        assert_eq!(defaults.overlay, overlay);
        assert_eq!(parse("[overlay]").unwrap().overlay, overlay);
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
            [overlay]
            tint = 0.25
            exclude_from_capture = false
        ",
        )
        .unwrap();
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
}
