//! The memory budget, read from the environment.
//!
//! Its own target rather than a case inside the eviction tests, and for the
//! same reason `models_root` has one: this is the only place in the slice that
//! touches a process-global variable. The eviction tests state their budget
//! directly with `Budget::new`, so nothing there races this.

use maestro_llamacpp::admission::Budget;

const VARIABLE: &str = "MAESTRO_MEMORY_BUDGET_MIB";

/// One test function, deliberately.
///
/// Environment variables are process-global and Rust runs the tests in a
/// binary concurrently, so four functions mutating the same variable would
/// race and fail for reasons that have nothing to do with the code. Putting
/// every case in one function removes the race rather than papering over it
/// with a lock.
#[test]
fn the_budget_comes_from_the_environment_and_is_absent_when_unset() {
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

    unsafe { std::env::set_var(VARIABLE, "") };
    assert_eq!(
        Budget::configured()
            .expect("an empty value is unset, not an error")
            .limit_mib(),
        None,
        "`export MAESTRO_MEMORY_BUDGET_MIB=` is a slip, and reading it as a \
         budget of nothing would refuse every model on the machine"
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
    assert_eq!(
        Budget::configured()
            .expect("an unset budget is not an error")
            .limit_mib(),
        None,
        "unset means no budget, which means nothing is ever evicted"
    );

    unsafe {
        match original {
            Some(value) => std::env::set_var(VARIABLE, value),
            None => std::env::remove_var(VARIABLE),
        }
    }
}
