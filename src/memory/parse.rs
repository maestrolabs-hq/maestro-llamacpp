//! Reading numbers out of what the platform's own tools print.
//!
//! Every function here takes captured text and returns a figure or `None`,
//! and nothing here runs anything: the commands that produce the text are
//! [`super::command`]'s business. Kept apart so each parser can be proven on
//! output captured from a real machine without that machine being present.
//!
//! Every parser fails to `None`. A figure this module cannot read is a probe
//! that reports "unknown", never a wrong number, because a wrong number here
//! becomes a decision to start a model there is no room for.

use super::DeviceMemory;

/// One mebibyte in kibibytes, the unit `ps` and `/proc/meminfo` speak.
pub(super) const KIB_PER_MIB: u64 = 1024;

/// One mebibyte in bytes, the unit `sysctl` and PowerShell speak.
pub(super) const BYTES_PER_MIB: u64 = 1024 * 1024;

/// What `nvidia-smi --query-gpu=memory.total,memory.used` printed, as one
/// device.
///
/// One line per device, `total, used`, both in mebibytes. Several devices are
/// summed, because `llama-server` splits a model across every device it can
/// see by default -- so what fits is what they hold together.
pub(super) fn device(text: &str) -> Option<DeviceMemory> {
    let mut total = 0u64;
    let mut used = 0u64;
    let mut devices = 0;
    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        let (left, right) = line.split_once(',')?;
        total += left.trim().parse::<u64>().ok()?;
        used += right.trim().parse::<u64>().ok()?;
        devices += 1;
    }
    (devices > 0).then_some(DeviceMemory {
        total_mib: total,
        used_mib: used,
    })
}

/// What one process holds on the device, from
/// `nvidia-smi --query-compute-apps=pid,used_memory`.
///
/// One line per process per device, `pid, used`, in mebibytes. A process on
/// several devices appears once per device, so its lines are summed. A
/// process that appears on no line is unknown rather than zero: the query
/// prints nothing at all on some platforms, and reading that silence as "this
/// child holds nothing on the device" would let eviction free room that is
/// not there.
pub(super) fn compute_apps(text: &str, pid: u32) -> Option<u64> {
    let mut used = None;
    for line in text.lines() {
        let Some((left, right)) = line.split_once(',') else {
            continue;
        };
        if left.trim().parse::<u32>().ok() != Some(pid) {
            continue;
        }
        let mib = right.trim().parse::<u64>().ok()?;
        used = Some(used.unwrap_or(0) + mib);
    }
    used
}

/// `MemTotal` from `/proc/meminfo`, in mebibytes.
pub(super) fn meminfo_total(text: &str) -> Option<u64> {
    let line = text
        .lines()
        .find_map(|line| line.strip_prefix("MemTotal:"))?;
    scaled(line.split_whitespace().next()?, KIB_PER_MIB)
}

/// One whole number on its own, in some unit `per_mib` of which make a
/// mebibyte: bytes from `sysctl -n hw.memsize` and PowerShell, kibibytes
/// padded with spaces from `ps -o rss=`, or nothing at all when the process
/// asked about is gone.
pub(super) fn scaled(text: &str, per_mib: u64) -> Option<u64> {
    let count: u64 = text.trim().parse().ok()?;
    Some(count / per_mib)
}

/// A resident set from `tasklist /FI "PID eq <pid>" /FO CSV /NH`, in
/// mebibytes.
///
/// One quoted CSV line whose last field is the working set as `12,345 K`.
/// When nothing matches, `tasklist` prints a sentence beginning `INFO:`
/// rather than an empty result, which is why the last field has to carry the
/// unit to count. Fields are split on the quote-comma-quote between them
/// rather than on the comma alone, because the figure carries a thousands
/// separator and splitting on that would read `345` out of `12,345`.
pub(super) fn tasklist_rss(text: &str) -> Option<u64> {
    let line = text.lines().find(|line| line.starts_with('"'))?;
    let field = line.rsplit("\",\"").next()?.trim().trim_matches('"');
    let digits: String = field.chars().filter(char::is_ascii_digit).collect();
    if digits.is_empty() || !field.ends_with('K') {
        return None;
    }
    let kib: u64 = digits.parse().ok()?;
    Some(kib / KIB_PER_MIB)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Captured from an RTX 5090 under WSL, where the router was written.
    const ONE_DEVICE: &str = "32607, 7903\n";

    #[test]
    fn a_device_query_reads_total_and_used_from_one_line() {
        let read = device(ONE_DEVICE).expect("one device");
        assert_eq!((read.total_mib, read.used_mib), (32607, 7903));
        assert_eq!(read.free_mib(), 24704, "free is what is not used");
    }

    #[test]
    fn several_devices_are_summed_because_a_model_is_split_across_them() {
        let read = device("24576, 1000\n24576, 2000\n").expect("two devices");
        assert_eq!((read.total_mib, read.used_mib), (49152, 3000));
    }

    #[test]
    fn a_device_query_that_says_nothing_is_unknown_not_zero() {
        assert!(device("").is_none(), "no line means no device");
        assert!(
            device("[N/A], [N/A]\n").is_none(),
            "a driver that cannot say is not a device holding nothing"
        );
        assert!(device("Failed to initialize NVML: Driver/library version mismatch\n").is_none());
    }

    #[test]
    fn a_process_query_sums_the_lines_naming_the_pid_and_ignores_the_rest() {
        let text = "12345, 4523\n777, 900\n12345, 100\n";
        assert_eq!(compute_apps(text, 12345), Some(4623));
        assert_eq!(compute_apps(text, 777), Some(900));
    }

    #[test]
    fn a_process_the_query_does_not_name_is_unknown_not_zero() {
        assert_eq!(
            compute_apps("777, 900\n", 12345),
            None,
            "the query prints nothing for every process on some platforms, \
             and silence must not become a claim that a child holds nothing"
        );
        assert_eq!(compute_apps("", 12345), None);
    }

    #[test]
    fn meminfo_reads_the_total_in_mebibytes() {
        let text = "MemTotal:       49331796 kB\nMemFree:        10103280 kB\n";
        assert_eq!(meminfo_total(text), Some(48175));
        assert_eq!(meminfo_total("MemFree: 1 kB\n"), None);
    }

    #[test]
    fn a_number_on_its_own_reads_in_mebibytes_at_the_stated_unit() {
        assert_eq!(scaled("51539607552\n", BYTES_PER_MIB), Some(49152), "bytes");
        assert_eq!(scaled(" 3624\n", KIB_PER_MIB), Some(3), "kibibytes, padded");
        assert_eq!(scaled("7598996\n", KIB_PER_MIB), Some(7420));
        assert_eq!(
            scaled("sysctl: unknown oid 'hw.memsize'\n", BYTES_PER_MIB),
            None
        );
        assert_eq!(
            scaled("", KIB_PER_MIB),
            None,
            "a process that is gone prints nothing"
        );
    }

    #[test]
    fn a_resident_set_from_tasklist_reads_the_last_field() {
        let text = "\"stub-llama-server.exe\",\"1234\",\"Console\",\"1\",\"12,345 K\"\r\n";
        assert_eq!(tasklist_rss(text), Some(12));
        assert_eq!(
            tasklist_rss("INFO: No tasks are running which match the specified criteria.\r\n"),
            None
        );
    }
}
