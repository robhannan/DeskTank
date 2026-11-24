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
`--sway`, `--sway-on-battery`, `--easy-mode`, `--creature-cap` and
`--catch-up` flags take precedence. Use `--catch-up=false` to turn it off
when the file enables it.

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
# Advance the tank when it reopens or resumes after a power or visibility pause.
catch_up = false
```

Catch-up applies elapsed wall-clock time in one summarised step. Reopening
uses the loaded save's last modification time. A resume counts from when
the window paused: when it was hidden, occluded or minimised, and on macOS
also when the display slept, the session locked or power ran low. Until the
Windows and Linux power monitors land (O1), a system sleep with the window
in view doesn't pause it there, so that time isn't caught up. Gaps shorter
than one minute are ignored and gaps over 24 hours are capped at 24 hours.
Only the process that owns the saved tank catches up: read-only viewers and
`--no-save` tanks don't. Changes to `catch_up` take effect on the next
resume.
