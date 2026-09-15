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
use maestro_llamacpp::memory::{Fixed, Probe};

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
        Budget::configured()
            .expect("a numeric budget is accepted")
            .limit_mib(),
        Some(24576),
        "the variable is used as given"
    );

    // One reading of the machine, held still. `Probe::detect` finds the tools
    // once, but `device` and `system_total_mib` run them on every call, so
    // three detections asked three times and compared whatever came back. A
    // tool that is missing, hangs, or is killed under load answers `None`
    // rather than failing -- the degradation this module is built around --
    // and two answers that differ derive two different budgets. Freezing what
    // the machine said into a `Fixed` leaves the assertions below comparing
    // figures rather than moments.
    let detected = Probe::detect();
    let device = detected.device();
    let system_total_mib = detected.system_total_mib();
    let answers = device.is_some() || system_total_mib.is_some();
    let machine = Budget::derived(Probe::Fixed(Fixed {
        device,
        system_total_mib,
        ..Fixed::default()
    }));

    unsafe { std::env::set_var(VARIABLE, "") };
    assert_eq!(
        Budget::configured()
            .expect("an empty value is unset, not an error")
            .limit_mib(),
        machine.limit_mib(),
        "`export MAESTRO_MEMORY_BUDGET_MIB=` is a slip, and reading it as a \
         budget of nothing would refuse every model on the machine; it means \
         what unset means"
    );

    unsafe { std::env::set_var(VARIABLE, "plenty") };
    let failure = Budget::configured()
        .expect_err("a budget someone typed wrongly must not become no budget")
        .to_string();
    assert!(
        failure.contains(VARIABLE) && failure.contains("plenty"),
        "the refusal names the variable and what it carried: {failure}"
    );

    unsafe { std::env::remove_var(VARIABLE) };

    // `Budget::configured` asks the machine again, inside the library, and
    // nothing here can hand it the reading held above: a second reading is
    // unavoidable, so this is the one assertion that spans two of them. Where
    // the machine answered the same way twice -- which is what `source`
    // records -- the budgets must match, and that is asserted as strictly as
    // before. Where it did not, the machine changed its mind between two
    // calls rather than the budget being wrong, so it is asked again. A
    // disagreement that is real survives being asked five times; a tool
    // starved of CPU for a moment does not.
    let mut unset = Budget::configured().expect("an unset budget is not an error");
    for _ in 0..4 {
        if unset.source() == machine.source() {
            break;
        }
        unset = Budget::configured().expect("an unset budget is not an error");
    }

    assert_eq!(
        unset.limit_mib(),
        machine.limit_mib(),
        "unset means the budget the machine sets for itself; asked directly \
         the machine said '{}', and through the unset variable '{}'",
        machine.source(),
        unset.source()
    );
    assert_eq!(
        unset.limit_mib().is_some(),
        answers,
        "a machine that can say what it holds gets a ceiling, and only a \
         machine that cannot is left with none: {}",
        unset.source()
    );

    unsafe {
        match original {
            Some(value) => std::env::set_var(VARIABLE, value),
            None => std::env::remove_var(VARIABLE),
        }
    }
}
