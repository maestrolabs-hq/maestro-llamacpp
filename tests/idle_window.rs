//! The idle window, read from the environment.
//!
//! Its own target, for the same reason `memory_budget.rs` has one: this is
//! the only place in the slice that touches this process-global variable, and
//! a case living inside `idle::tests` would run in the same binary as every
//! other unit test, racing whichever of them happened to read or write it.

use maestro_llamacpp::idle::IdleWindow;

const VARIABLE: &str = "MAESTRO_IDLE_UNLOAD_SECONDS";

/// One test function, deliberately, for the same reason
/// `the_budget_comes_from_the_environment_and_is_absent_when_unset` is one:
/// four functions mutating the same environment variable would race each
/// other rather than the code, since Rust runs the tests in a binary
/// concurrently.
#[test]
fn the_idle_window_comes_from_the_environment_and_is_off_when_unset() {
    let original = std::env::var_os(VARIABLE);

    // SAFETY: this binary carries exactly one test, so nothing else in the
    // process is reading or writing the environment while these lines run.
    unsafe { std::env::remove_var(VARIABLE) };
    assert_eq!(
        IdleWindow::configured()
            .expect("an unset variable is not an error")
            .seconds(),
        None,
        "unset means no window, which means nothing is ever unloaded for \
         sitting idle"
    );

    unsafe { std::env::set_var(VARIABLE, "") };
    assert_eq!(
        IdleWindow::configured()
            .expect("an empty value is off, not an error")
            .seconds(),
        None,
        "`export MAESTRO_IDLE_UNLOAD_SECONDS=` is a slip, and reading it as a \
         window of zero seconds would unload every on-demand model on every \
         sweep"
    );

    unsafe { std::env::set_var(VARIABLE, "3600") };
    assert_eq!(
        IdleWindow::configured()
            .expect("a numeric window is accepted")
            .seconds(),
        Some(3600),
        "the variable is used as given"
    );

    unsafe { std::env::set_var(VARIABLE, "soon") };
    let Err(failure) = IdleWindow::configured() else {
        panic!("a window someone typed wrongly must not become no window");
    };
    let failure = failure.to_string();
    assert!(
        failure.contains(VARIABLE) && failure.contains("soon"),
        "the refusal names the variable and what it carried: {failure}"
    );

    unsafe {
        match original {
            Some(value) => std::env::set_var(VARIABLE, value),
            None => std::env::remove_var(VARIABLE),
        }
    }
}
