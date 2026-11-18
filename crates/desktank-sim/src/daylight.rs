//! Day and night (SPEC §2.1, §2.6, T6): the tank's lighting from the local
//! time of day, which the host injects (the simulation never reads the
//! wall clock).
//!
//! Lighting happens only at render time: the colour multiplier here tints
//! the drawn tank, and the simulation's light field doesn't change with the
//! hour, so day and night never wake a chunk. The schedule is a tank light:
//! full from 07:30 to 19:30, a warm ramp at dawn (05:30–07:30) and dusk
//! (19:30–21:30), and dim blue moonlight overnight.

/// Minutes in a day.
pub const MINUTES_PER_DAY: u16 = 24 * 60;
/// Midday, in minutes after midnight: the default time, drawn untinted.
pub const NOON: u16 = 12 * 60;

const NIGHT: [f32; 3] = [0.30, 0.36, 0.58];
const DAWN: [f32; 3] = [0.98, 0.78, 0.62];
const DAY: [f32; 3] = [1.0, 1.0, 1.0];
const DUSK: [f32; 3] = [0.96, 0.70, 0.58];

/// Key times (minutes after midnight) and the tint there; linear between.
const KEYS: [(u16, [f32; 3]); 8] = [
    (0, NIGHT),
    (330, NIGHT),
    (390, DAWN),
    (450, DAY),
    (1170, DAY),
    (1230, DUSK),
    (1290, NIGHT),
    (MINUTES_PER_DAY, NIGHT),
];

/// The colour multiplier (per sRGB-encoded channel, 0–1) for `minute`
/// minutes after midnight (taken modulo a day). Exactly 1 by day.
#[must_use]
pub fn tint(minute: u16) -> [f32; 3] {
    let now = minute % MINUTES_PER_DAY;
    let key = KEYS.windows(2).position(|w| now < w[1].0).unwrap_or(0);
    let ((start, from), (end, to)) = (KEYS[key], KEYS[key + 1]);
    let along = f32::from(now - start) / f32::from(end - start);
    // Where `from` and `to` agree this is exactly `from` (day stays 1).
    [0, 1, 2].map(|c| from[c] + (to[c] - from[c]) * along)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn day_is_untinted_and_night_is_dim_and_blue() {
        assert_eq!(tint(NOON), [1.0; 3]);
        assert_eq!(tint(450), [1.0; 3]);
        let night = tint(0);
        assert_eq!(night, NIGHT);
        assert!(night[2] > night[0] && night.iter().all(|&c| c < 0.6));
        assert_eq!(tint(MINUTES_PER_DAY + 1), tint(1));
        // Dawn and dusk are warm and between night and day.
        for m in [390, 1230] {
            let [r, g, b] = tint(m);
            assert!(r > g && g > b && r < 1.0, "{m}: {r} {g} {b}");
        }
        // Continuous: no jumps between neighbouring minutes.
        for m in 0..MINUTES_PER_DAY {
            let (a, b) = (tint(m), tint(m + 1));
            for i in 0..3 {
                assert!((a[i] - b[i]).abs() < 0.02, "jump at {m}");
            }
        }
    }
}
