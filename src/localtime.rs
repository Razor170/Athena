//! Turning Unix seconds into calendar dates and the user's local time.
//!
//! EVE and ESI work in UTC. The Finance tab shows times in the PC's own
//! time zone, as Windows sets it (including summer time). On other systems
//! (the Linux test builds) local time is UTC.

/// Calendar date (year, month 1-12, day 1-31) for days since 1970-01-01.
/// Howard Hinnant's well-known algorithm, the reverse of
/// `days_from_civil` in `modules/mining/gamelog.rs`.
pub fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    let year = year_of_era + era * 400 + if month <= 2 { 1 } else { 0 };
    (year, month, day)
}

/// Unix seconds moved into local time: the same number of seconds since
/// 1970, but counted in the local time zone. Use the result only for
/// showing dates and times, and for finding local day starts.
pub fn to_local(unix: i64) -> i64 {
    unix + utc_offset_secs(unix)
}

/// The start of the local day that `unix` falls in, as real Unix seconds.
pub fn local_day_start(unix: i64) -> i64 {
    let local = to_local(unix);
    let local_midnight = local.div_euclid(86_400) * 86_400;
    from_local(local_midnight, unix)
}

/// A local time (as from [`to_local`]) back to real Unix seconds, using
/// the offset at that moment, not at `near`: midnight on 1 October is in
/// summer time even when asked in November. `near` gives the first guess.
fn from_local(local: i64, near: i64) -> i64 {
    let guess = local - utc_offset_secs(near);
    local - utc_offset_secs(guess)
}

/// The start of the local month that `unix` falls in, `months_back`
/// months earlier (0: this month), as real Unix seconds.
pub fn local_month_start(unix: i64, months_back: i64) -> i64 {
    let local = to_local(unix);
    let (year, month, _) = civil_from_days(local.div_euclid(86_400));
    let index = year * 12 + (month - 1) - months_back;
    let (year, month) = (index.div_euclid(12), index.rem_euclid(12) + 1);
    let days = crate::modules::mining::gamelog::days_from_civil(year, month, 1);
    from_local(days * 86_400, unix)
}

/// "14:03:58" in local time.
pub fn clock_text(unix: i64) -> String {
    let seconds = to_local(unix).rem_euclid(86_400);
    format!(
        "{:02}:{:02}:{:02}",
        seconds / 3600,
        seconds % 3600 / 60,
        seconds % 60
    )
}

/// "Thu 12 Nov 2026" in local time.
pub fn date_text(unix: i64) -> String {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    const WEEKDAYS: [&str; 7] = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"];
    let days = to_local(unix).div_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    format!(
        "{} {day} {} {year}",
        WEEKDAYS[days.rem_euclid(7) as usize],
        MONTHS[(month - 1) as usize]
    )
}

/// How far local time is ahead of UTC at `unix`, in seconds (3600 for
/// Berlin in winter, 7200 in summer).
#[cfg(windows)]
pub fn utc_offset_secs(unix: i64) -> i64 {
    use windows_sys::Win32::Foundation::SYSTEMTIME;
    use windows_sys::Win32::System::Time::SystemTimeToTzSpecificLocalTime;

    let to_system_time = |unix: i64| {
        let (year, month, day) = civil_from_days(unix.div_euclid(86_400));
        let seconds = unix.rem_euclid(86_400);
        SYSTEMTIME {
            wYear: year as u16,
            wMonth: month as u16,
            wDayOfWeek: 0,
            wDay: day as u16,
            wHour: (seconds / 3600) as u16,
            wMinute: (seconds % 3600 / 60) as u16,
            wSecond: (seconds % 60) as u16,
            wMilliseconds: 0,
        }
    };
    let utc = to_system_time(unix);
    let mut local = to_system_time(unix);
    // SAFETY: both pointers point to valid SYSTEMTIME values that live
    // through the call; a null time zone means "the PC's current zone".
    let ok = unsafe { SystemTimeToTzSpecificLocalTime(std::ptr::null(), &utc, &mut local) };
    if ok == 0 {
        return 0;
    }
    let local_unix = crate::modules::mining::gamelog::days_from_civil(
        local.wYear as i64,
        local.wMonth as i64,
        local.wDay as i64,
    ) * 86_400
        + local.wHour as i64 * 3600
        + local.wMinute as i64 * 60
        + local.wSecond as i64;
    local_unix - unix
}

#[cfg(not(windows))]
pub fn utc_offset_secs(_unix: i64) -> i64 {
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_and_clock() {
        // Tests run on Linux, where local time is UTC.
        // `date -u -d "2026-11-12 14:03:58" +%s`
        let t = 1_794_492_238;
        assert_eq!(clock_text(t), "14:03:58");
        assert_eq!(date_text(t), "Thu 12 Nov 2026");
        assert_eq!(local_day_start(t), 1_794_441_600);
        assert_eq!(date_text(local_month_start(t, 0)), "Sun 1 Nov 2026");
        assert_eq!(date_text(local_month_start(t, 1)), "Thu 1 Oct 2026");
        assert_eq!(date_text(local_month_start(t, 11)), "Mon 1 Dec 2025");
        assert_eq!(civil_from_days(0), (1970, 1, 1));
    }
}
