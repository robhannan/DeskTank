//! Process resource use for `desktank bench`: CPU time and resident memory.

use core::time::Duration;

/// A snapshot of this process's resource use.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ProcessStats {
    /// User plus system CPU time since the process started.
    pub cpu_time: Duration,
    /// Current resident set size in bytes, if the OS reports it.
    pub rss_bytes: Option<u64>,
    /// Peak resident set size in bytes, if the OS reports it.
    pub peak_rss_bytes: Option<u64>,
}

/// This process's resource use now.
#[must_use]
pub fn process_stats() -> ProcessStats {
    imp::process_stats()
}

#[cfg(unix)]
mod imp {
    use super::{Duration, ProcessStats};

    pub fn process_stats() -> ProcessStats {
        // SAFETY: `rusage` is plain data, so all-zero is a valid value, and
        // `getrusage` only writes into the struct we pass.
        let mut ru: libc::rusage = unsafe { core::mem::zeroed() };
        // SAFETY: `ru` is a valid, writable `rusage`.
        let ok = unsafe { libc::getrusage(libc::RUSAGE_SELF, &raw mut ru) } == 0;
        if !ok {
            return ProcessStats::default();
        }
        let tv = |t: libc::timeval| {
            Duration::from_secs(u64::try_from(t.tv_sec).unwrap_or(0))
                + Duration::from_micros(u64::try_from(t.tv_usec).unwrap_or(0))
        };
        let maxrss = u64::try_from(ru.ru_maxrss).unwrap_or(0);
        // macOS reports ru_maxrss in bytes, Linux and the BSDs in KiB.
        let peak = if cfg!(target_vendor = "apple") {
            maxrss
        } else {
            maxrss * 1024
        };
        ProcessStats {
            cpu_time: tv(ru.ru_utime) + tv(ru.ru_stime),
            rss_bytes: rss(),
            peak_rss_bytes: Some(peak),
        }
    }

    #[cfg(target_vendor = "apple")]
    fn rss() -> Option<u64> {
        // SAFETY: `proc_taskinfo` is plain data, so all-zero is valid.
        let mut info: libc::proc_taskinfo = unsafe { core::mem::zeroed() };
        let size = i32::try_from(size_of::<libc::proc_taskinfo>()).ok()?;
        // SAFETY: `info` is writable for `size` bytes, the size the
        // PROC_PIDTASKINFO flavour fills; the pid is our own.
        let n = unsafe {
            libc::proc_pidinfo(
                libc::getpid(),
                libc::PROC_PIDTASKINFO,
                0,
                (&raw mut info).cast(),
                size,
            )
        };
        (n == size).then_some(info.pti_resident_size)
    }

    #[cfg(not(target_vendor = "apple"))]
    fn rss() -> Option<u64> {
        // Linux: the second field of /proc/self/statm is resident pages.
        let statm = std::fs::read_to_string("/proc/self/statm").ok()?;
        let pages: u64 = statm.split_whitespace().nth(1)?.parse().ok()?;
        // SAFETY: sysconf has no preconditions.
        let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
        Some(pages * u64::try_from(page).ok()?)
    }
}

#[cfg(windows)]
mod imp {
    use super::{Duration, ProcessStats};
    use windows_sys::Win32::Foundation::FILETIME;
    use windows_sys::Win32::System::ProcessStatus::{
        K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};

    pub fn process_stats() -> ProcessStats {
        let zero = FILETIME {
            dwLowDateTime: 0,
            dwHighDateTime: 0,
        };
        let (mut created, mut exited, mut kernel, mut user) = (zero, zero, zero, zero);
        // SAFETY: the pseudo-handle from GetCurrentProcess is always valid,
        // and the four FILETIMEs are writable.
        let ok = unsafe {
            GetProcessTimes(
                GetCurrentProcess(),
                &raw mut created,
                &raw mut exited,
                &raw mut kernel,
                &raw mut user,
            )
        } != 0;
        // FILETIME counts 100 ns intervals.
        let ft = |t: FILETIME| {
            Duration::from_nanos(
                ((u64::from(t.dwHighDateTime) << 32) | u64::from(t.dwLowDateTime)) * 100,
            )
        };
        let cpu_time = if ok {
            ft(kernel) + ft(user)
        } else {
            Duration::ZERO
        };
        // SAFETY: plain data, all-zero is valid.
        let mut mem: PROCESS_MEMORY_COUNTERS = unsafe { core::mem::zeroed() };
        let cb = u32::try_from(size_of::<PROCESS_MEMORY_COUNTERS>()).unwrap_or(0);
        mem.cb = cb;
        // SAFETY: `mem` is writable for `cb` bytes; the handle is our own.
        let ok = unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), &raw mut mem, cb) } != 0;
        let bytes = |n: usize| ok.then(|| u64::try_from(n).unwrap_or(u64::MAX));
        ProcessStats {
            cpu_time,
            rss_bytes: bytes(mem.WorkingSetSize),
            peak_rss_bytes: bytes(mem.PeakWorkingSetSize),
        }
    }
}

#[cfg(not(any(unix, windows)))]
mod imp {
    use super::ProcessStats;

    pub fn process_stats() -> ProcessStats {
        ProcessStats::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_cpu_time_and_memory() {
        let before = process_stats();
        // Burn a little CPU so the counter moves.
        let mut x = 0u64;
        for i in 0..20_000_000u64 {
            x = core::hint::black_box(x.wrapping_add(i));
        }
        let after = process_stats();
        assert!(after.cpu_time > before.cpu_time);
        if cfg!(any(target_os = "macos", target_os = "linux", windows)) {
            let rss = after.rss_bytes.unwrap();
            assert!(rss > 1 << 20, "rss {rss}");
            assert!(after.peak_rss_bytes.unwrap() >= rss / 2);
        }
    }
}
