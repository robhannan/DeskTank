# Desktop window configuration

`desktank window` reads `config.toml` from the directory for your OS.
Set `DESKTANK_CONFIG_DIR` to use another directory.

| OS | File |
|---|---|
| macOS | `~/Library/Application Support/DeskTank/config.toml` |
| Windows | `%APPDATA%\DeskTank\config.toml` |
| Linux | `$XDG_CONFIG_HOME/desktank/config.toml`, or `~/.config/desktank/config.toml` when `XDG_CONFIG_HOME` is unset |

A missing file or omitted key uses its default. Saves apply within a second
while the window is running. An invalid file logs an error to stderr and
keeps the last good settings, or uses defaults at startup. Explicit
`--sway`, `--sway-on-battery`, `--easy-mode` and `--creature-cap` flags
take precedence.

```toml
version = 1

[power]
# Both caps are 1–120 fps. The battery cap must be at or below the AC cap.
ac_fps = 30
battery_fps = 15
# 0–100%. Below this level, the tank pauses on battery power.
min_battery_percent = 20
pause_in_low_power_mode = true
allow_over_lock = false

[plants]
sway = false
sway_on_battery = false

[life]
# Omit either key to keep the tank's setting. A new tank starts with
# easy mode off and a cap of 60. The cap accepts 1–128 and cannot be
# lowered below the tank's current population.
# easy_mode = false
# population_cap = 60
```
