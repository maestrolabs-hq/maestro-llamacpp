//! The memory budget, read from the environment.
//!
//! Its own target rather than a case inside the eviction tests, and for the
//! same reason `models_root` has one: this is the only place in the slice that
//! touches a process-global variable. The eviction tests state their budget
//! directly with `Budget::new`, so nothing there races this.
//!
//! What an unset variable means depends on the machine: the router asks it
//! what it holds and sets a ceiling from the answer. That figure cannot be
//! asserted here without asserting about whichever machine runs the tests,
//! so what is asserted is agreement -- an unset variable yields exactly what
//! asking the machine yields -- and that a machine which can answer at all
//! does not end up with no budget.

use maestro_llamacpp::admission::Budget;
use maestro_llamacpp::memory::{DeviceMemory, Fixed, Probe};

/// A machine that answers exactly this, and runs nothing to do it.
fn stating(device: Option<DeviceMemory>, system_total_mib: Option<u64>) -> Probe {
    Probe::Fixed(Fixed {
        device,
        system_total_mib,
        ..Fixed::default()
    })
}

const VARIABLE: &str = "MAESTRO_MEMORY_BUDGET_MIB";

/// One test function, deliberately.
///
/// Environment variables are process-global and Rust runs the tests in a
/// binary concurrently, so four functions mutating the same variable would
/// race and fail for reasons that have nothing to do with the code. Putting
/// every case in one function removes the race rather than papering over it
/// with a lock.
#[test]
fn the_budget_comes_from_the_environment_and_from_the_machine_when_unset() {
    let original = std::env::var_os(VARIABLE);

    // SAFETY: this binary carries exactly one test, so nothing else in the
    // process is reading or writing the environment while these lines run.
    unsafe { std::env::set_var(VARIABLE, "24576") };
    assert_eq!(
        Budget::configured(Probe::none())
            .expect("a numeric budget is accepted")
            .limit_mib(),
        Some(24576),
        "the variable is used as given, whatever the machine would have said"
    );

    // The machine is stated, not read. Every figure the budget derives from
    // now comes from the probe handed in, so what follows asserts about the
    // budget rather than about whichever card the test happens to run beside.
    //
    // It used to read the real machine and compare two budgets, which meant
    // comparing two moments: a tool that is missing, hangs, or is starved for
    // an instant answers `None` rather than failing -- the degradation this
    // module is built around -- so the two readings could legitimately differ
    // and the assertion failed on a machine that had merely changed its mind.
    // No reading, no moments, no flake.

    unsafe { std::env::set_var(VARIABLE, "") };
    assert_eq!(
        Budget::configured(stating(
            Some(DeviceMemory {
                total_mib: 8192,
                used_mib: 0,
            }),
            None,
        ))
        .expect("an empty value is unset, not an error")
        .limit_mib(),
        Some(7168),
        "`export MAESTRO_MEMORY_BUDGET_MIB=` is a slip, and reading it as a \
         budget of nothing would refuse every model on the machine; it means \
         what unset means, which on this stated machine is 8192 less the floor"
    );

    unsafe { std::env::set_var(VARIABLE, "plenty") };
    let failure = Budget::configured(Probe::none())
        .expect_err("a budget someone typed wrongly must not become no budget")
        .to_string();
    assert!(
        failure.contains(VARIABLE) && failure.contains("plenty"),
        "the refusal names the variable and what it carried: {failure}"
    );

    // Unset, on a machine with a device. A tenth of 8192 is 819, under the
    // 1024 floor, so the floor is the margin and the ceiling is 7168.
    unsafe { std::env::remove_var(VARIABLE) };
    let on_a_device = Budget::configured(stating(
        Some(DeviceMemory {
            total_mib: 8192,
            used_mib: 0,
        }),
        Some(16384),
    ))
    .expect("an unset budget is not an error");
    assert_eq!(
        on_a_device.limit_mib(),
        Some(7168),
        "the device is asked first and its margin is the floor, not a tenth: {}",
        on_a_device.source()
    );

    // Unset, with no device to ask: four fifths of 16384 is 13107.
    let on_system_memory =
        Budget::configured(stating(None, Some(16384))).expect("an unset budget is not an error");
    assert_eq!(
        on_system_memory.limit_mib(),
        Some(13107),
        "with nothing on the device side, system memory sets it: {}",
        on_system_memory.source()
    );

    // Unset, on a machine that answers nothing at all. This is the case the
    // old test could not state: it had to find a machine that was silent.
    let silent = Budget::configured(stating(None, None)).expect("an unset budget is not an error");
    assert_eq!(
        silent.limit_mib(),
        None,
        "a machine that cannot say what it holds is left with no ceiling \
         rather than a guessed one: {}",
        silent.source()
    );

    unsafe {
        match original {
            Some(value) => std::env::set_var(VARIABLE, value),
            None => std::env::remove_var(VARIABLE),
        }
    }
}
