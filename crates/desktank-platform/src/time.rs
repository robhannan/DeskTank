//! The OS's local date and time, for the day and night clock.

/// The local wall clock: the time of day, the day of the year and the local
/// offset from UTC, as the sunrise and sunset maths needs them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LocalDateTime {
    /// Minutes after local midnight (`0..1440`).
    pub minute: u16,
    /// Day of the year, 1 on 1 January (`1..=366`).
    pub day_of_year: u16,
    /// Local time minus UTC, in minutes, with daylight saving applied.
    pub utc_offset_minutes: i16,
}

/// The local date and time, if the OS reports them.
#[must_use]
pub fn local_date_time() -> Option<LocalDateTime> {
    #[cfg(unix)]
    {
        unix_local_date_time()
    }
    #[cfg(target_os = "windows")]
    {
        windows_local_date_time()
    }
    #[cfg(not(any(unix, target_os = "windows")))]
    {
        None
    }
}

/// Local time of day as minutes after midnight (`0..1440`), if the OS reports it.
#[must_use]
pub fn local_minute() -> Option<u16> {
    local_date_time().map(|date_time| date_time.minute)
}

#[cfg(unix)]
fn unix_local_date_time() -> Option<LocalDateTime> {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs();
    let time = libc::time_t::try_from(secs).ok()?;
    // SAFETY: `libc::tm` is plain C data, so all-zero is a valid initial
    // value, and `localtime_r` writes into `tm` without using static state.
    let mut tm: libc::tm = unsafe { core::mem::zeroed() };
    // SAFETY: `time` and `tm` are valid pointers for `localtime_r`.
    let ptr = unsafe { libc::localtime_r(&raw const time, &raw mut tm) };
    if ptr.is_null() {
        return None;
    }
    let hour = u16::try_from(tm.tm_hour).ok()?;
    let min = u16::try_from(tm.tm_min).ok()?;
    // `tm_yday` counts from 0; a leap year's last day is 365.
    let day_of_year = u16::try_from(tm.tm_yday).ok()?.checked_add(1)?;
    let utc_offset_minutes = i16::try_from(tm.tm_gmtoff / 60).ok()?;
    (hour < 24 && min < 60 && (1..=366).contains(&day_of_year)).then_some(LocalDateTime {
        minute: hour * 60 + min,
        day_of_year,
        utc_offset_minutes,
    })
}

#[cfg(target_os = "windows")]
fn windows_local_date_time() -> Option<LocalDateTime> {
    use windows_sys::Win32::Foundation::SYSTEMTIME;
    use windows_sys::Win32::System::SystemInformation::GetLocalTime;
    use windows_sys::Win32::System::Time::{GetTimeZoneInformation, TIME_ZONE_INFORMATION};

    let mut now = SYSTEMTIME::default();
    // SAFETY: `now` is a valid out pointer for `GetLocalTime`.
    unsafe { GetLocalTime(&raw mut now) };
    let mut zone = TIME_ZONE_INFORMATION::default();
    // SAFETY: `zone` is a valid out pointer for `GetTimeZoneInformation`.
    let id = unsafe { GetTimeZoneInformation(&raw mut zone) };
    if now.wHour >= 24 || now.wMinute >= 60 {
        return None;
    }
    Some(LocalDateTime {
        minute: now.wHour * 60 + now.wMinute,
        day_of_year: day_of_year(now.wYear, now.wMonth, now.wDay)?,
        utc_offset_minutes: windows_utc_offset(
            zone.Bias,
            zone.StandardBias,
            zone.DaylightBias,
            id,
        )?,
    })
}

/// `TIME_ZONE_ID_STANDARD` and `TIME_ZONE_ID_DAYLIGHT`, the results of
/// `GetTimeZoneInformation` that name the bias in force.
#[cfg(any(target_os = "windows", test))]
const TIME_ZONE_ID_STANDARD: u32 = 1;
#[cfg(any(target_os = "windows", test))]
const TIME_ZONE_ID_DAYLIGHT: u32 = 2;
/// `TIME_ZONE_ID_INVALID`: the call failed.
#[cfg(any(target_os = "windows", test))]
const TIME_ZONE_ID_INVALID: u32 = u32::MAX;

/// Local time minus UTC in minutes from a Windows `TIME_ZONE_INFORMATION`.
/// Windows reports the bias the other way round (UTC minus local), and the
/// result of `GetTimeZoneInformation` says whether the standard or the
/// daylight bias is in force (neither when the zone has no daylight saving).
#[cfg(any(target_os = "windows", test))]
fn windows_utc_offset(bias: i32, standard_bias: i32, daylight_bias: i32, id: u32) -> Option<i16> {
    let active = match id {
        TIME_ZONE_ID_INVALID => return None,
        TIME_ZONE_ID_STANDARD => standard_bias,
        TIME_ZONE_ID_DAYLIGHT => daylight_bias,
        _ => 0,
    };
    i16::try_from(-(bias.checked_add(active)?)).ok()
}

/// Day of the year (1 on 1 January) for a Gregorian date, or `None` if the
/// month or day is out of range.
#[cfg(any(target_os = "windows", test))]
fn day_of_year(year: u16, month: u16, day: u16) -> Option<u16> {
    const DAYS: [u16; 12] = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let leap = (year.is_multiple_of(4) && !year.is_multiple_of(100)) || year.is_multiple_of(400);
    let index = usize::from(month.checked_sub(1)?);
    let length = *DAYS.get(index)? + u16::from(leap && month == 2);
    if !(1..=length).contains(&day) {
        return None;
    }
    let before: u16 = DAYS[..index].iter().sum::<u16>() + u16::from(leap && month > 2);
    Some(before + day)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn day_of_year_counts_from_one_and_knows_leap_years() {
        assert_eq!(day_of_year(2026, 1, 1), Some(1));
        assert_eq!(day_of_year(2026, 3, 1), Some(60));
        assert_eq!(day_of_year(2026, 12, 31), Some(365));
        assert_eq!(day_of_year(2024, 2, 29), Some(60));
        assert_eq!(day_of_year(2024, 3, 1), Some(61));
        assert_eq!(day_of_year(2024, 12, 31), Some(366));
        assert_eq!(
            day_of_year(1900, 3, 1),
            Some(60),
            "1900 was not a leap year"
        );
        assert_eq!(day_of_year(2000, 3, 1), Some(61), "2000 was");
    }

    #[test]
    fn day_of_year_rejects_impossible_dates() {
        assert_eq!(day_of_year(2026, 0, 1), None);
        assert_eq!(day_of_year(2026, 13, 1), None);
        assert_eq!(day_of_year(2026, 1, 0), None);
        assert_eq!(day_of_year(2026, 4, 31), None);
        assert_eq!(day_of_year(2026, 2, 29), None);
        assert_eq!(day_of_year(2024, 2, 30), None);
    }

    #[test]
    fn windows_offset_flips_the_bias_and_adds_the_active_one() {
        // US Pacific: bias 480, daylight bias -60.
        assert_eq!(
            windows_utc_offset(480, 0, -60, TIME_ZONE_ID_STANDARD),
            Some(-480)
        );
        assert_eq!(
            windows_utc_offset(480, 0, -60, TIME_ZONE_ID_DAYLIGHT),
            Some(-420)
        );
        // India: bias -330, no daylight saving (id 0, unknown).
        assert_eq!(windows_utc_offset(-330, 0, 0, 0), Some(330));
        // Central Europe: bias -60, daylight bias -60.
        assert_eq!(
            windows_utc_offset(-60, 0, -60, TIME_ZONE_ID_DAYLIGHT),
            Some(120)
        );
        assert_eq!(windows_utc_offset(-60, 0, -60, TIME_ZONE_ID_INVALID), None);
    }

    #[test]
    fn local_date_time_is_in_range_and_matches_local_minute() {
        // Skip on targets with no local-time source.
        let Some(now) = local_date_time() else {
            assert_eq!(local_minute(), None);
            return;
        };
        assert!(now.minute < 1440, "{now:?}");
        assert!((1..=366).contains(&now.day_of_year), "{now:?}");
        assert!(
            (-14 * 60..=14 * 60).contains(&now.utc_offset_minutes),
            "{now:?}"
        );
        // The minute can roll over between the two calls.
        let minute = local_minute().unwrap();
        assert!(minute == now.minute || minute == (now.minute + 1) % 1440);
    }
}
