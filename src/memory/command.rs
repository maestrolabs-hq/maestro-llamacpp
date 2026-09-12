//! Running the platform's own tools, bounded, and handing back what they
//! printed.
//!
//! Which tool answers which question differs by operating system, and that is
//! the only thing decided here: the numbers are read by [`super::parse`], and
//! what to do with them is nobody's business below `memory`.
//!
//! Every run is bounded. A driver tool that hangs -- and `nvidia-smi` can,
//! on a machine whose driver is mid-update -- would otherwise hold the
//! admission lock for as long as it liked, and every load on the router with
//! it. Past the bound the tool is killed and the answer is "unknown", which
//! is what every caller already handles.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use super::parse;
use crate::launch::on_search_path;

/// How long one tool may take before its answer is treated as no answer.
///
/// Generous against a cold start of the driver, which takes a second or two
/// under WSL, and still short enough that a request waiting on admission
/// learns its fate rather than sitting on a probe.
const TIMEOUT: Duration = Duration::from_secs(5);

/// How often a running tool is asked whether it has finished.
const POLL: Duration = Duration::from_millis(20);

/// The device tool's name, with the platform's executable suffix.
const NVIDIA_SMI: &str = "nvidia-smi";

/// Where `nvidia-smi` is, if this machine has one.
///
/// The search path first, then the places it lives when it is not on the
/// search path: under WSL the driver's tools are mounted at a fixed
/// platform location that names the platform rather than a machine, and on
/// Windows they sit beside the system, in directories only the environment
/// can name.
pub(super) fn nvidia_smi() -> Option<PathBuf> {
    on_search_path(NVIDIA_SMI).or_else(|| {
        known_locations()
            .into_iter()
            .find(|candidate| candidate.is_file())
    })
}

/// Where `nvidia-smi` lives when the search path does not carry it.
fn known_locations() -> Vec<PathBuf> {
    let file = format!("{NVIDIA_SMI}{}", std::env::consts::EXE_SUFFIX);
    if cfg!(windows) {
        let system =
            std::env::var_os("SystemRoot").map(|root| PathBuf::from(root).join("System32"));
        let vendor = std::env::var_os("ProgramFiles").map(|programs| {
            PathBuf::from(programs)
                .join("NVIDIA Corporation")
                .join("NVSMI")
        });
        system
            .into_iter()
            .chain(vendor)
            .map(|directory| directory.join(&file))
            .collect()
    } else {
        vec![Path::new("/usr/lib/wsl/lib").join(&file)]
    }
}

/// The query for what the device holds in total and in use.
pub(super) const DEVICE_QUERY: &str = "--query-gpu=memory.total,memory.used";

/// The query for what each process holds on the device.
pub(super) const PROCESS_QUERY: &str = "--query-compute-apps=pid,used_memory";

/// What the device tool prints for one query, as bare comma-separated
/// numbers with no header and no units.
pub(super) fn query(nvidia_smi: &Path, query: &str) -> Option<String> {
    output(Command::new(nvidia_smi).args([query, "--format=csv,noheader,nounits"]))
}

/// What this machine has in system memory, in mebibytes.
///
/// A file on Linux, a tool everywhere else. Each branch compiles on every
/// platform and only the one for this platform runs, so a mistake in the
/// Windows branch is caught by the Linux build rather than by an operator.
pub(super) fn system_total_mib() -> Option<u64> {
    if cfg!(target_os = "linux") {
        let text = std::fs::read_to_string("/proc/meminfo").ok()?;
        parse::meminfo_total(&text)
    } else if cfg!(target_os = "macos") {
        let text = output(Command::new("sysctl").args(["-n", "hw.memsize"]))?;
        parse::scaled(&text, parse::BYTES_PER_MIB)
    } else if cfg!(windows) {
        let text = output(Command::new("powershell").args([
            "-NoProfile",
            "-Command",
            "(Get-CimInstance Win32_ComputerSystem).TotalPhysicalMemory",
        ]))?;
        parse::scaled(&text, parse::BYTES_PER_MIB)
    } else {
        None
    }
}

/// One process's resident set, in mebibytes.
pub(super) fn resident_mib(pid: u32) -> Option<u64> {
    if cfg!(windows) {
        let filter = format!("PID eq {pid}");
        let text = output(Command::new("tasklist").args(["/FI", &filter, "/FO", "CSV", "/NH"]))?;
        parse::tasklist_rss(&text)
    } else {
        let text = output(Command::new("ps").args(["-o", "rss=", "-p", &pid.to_string()]))?;
        parse::scaled(&text, parse::KIB_PER_MIB)
    }
}

/// What a tool printed, or `None` when it could not be run, did not finish
/// in time, or finished unhappily.
///
/// Its output is drained on a thread of its own while this one watches the
/// clock, because a pipe nobody reads fills, and a tool blocked on a full
/// pipe never finishes. When the bound passes the tool is killed, which
/// closes the pipe and ends the reader.
fn output(command: &mut Command) -> Option<String> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut text = String::new();
        drop(stdout.read_to_string(&mut text));
        text
    });

    let deadline = Instant::now() + TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let text = reader.join().ok()?;
                return status.success().then_some(text);
            }
            Ok(None) if Instant::now() < deadline => std::thread::sleep(POLL),
            _ => {
                drop(child.kill());
                drop(child.wait());
                drop(reader.join());
                return None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tool that is not there is an unknown figure, not a failure. Proven
    /// with a name no machine carries, so the test is about absence rather
    /// than about whichever tools this machine happens to have.
    #[test]
    fn a_tool_that_is_not_there_answers_nothing() {
        assert_eq!(
            output(&mut Command::new("no-such-tool-on-any-machine")),
            None
        );
    }

    /// A tool that exits unhappily answers nothing, so an error message is
    /// never read as a number. Driven through this test binary itself, which
    /// exits non-zero when handed a flag it does not know -- a filter that
    /// matches no test would not do, because running nothing is a success.
    #[test]
    fn a_tool_that_fails_answers_nothing() {
        let own = std::env::current_exe().expect("this test binary's own path");
        assert_eq!(
            output(Command::new(own).arg("--no-such-flag-in-any-test-harness")),
            None
        );
    }
}
