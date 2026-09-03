//! Where catalog locations resolve against.
//!
//! Its own target rather than a case inside the supervision tests: this stays
//! red until the launch command exists, while supervision turns green before
//! it, and one target that is half green tells a reader nothing.

use std::path::PathBuf;

use maestro_llamacpp::launch::models_root;

const VARIABLE: &str = "MAESTRO_MODELS_ROOT";

/// The variable each platform keeps a home directory in.
fn home_variable() -> &'static str {
    if cfg!(windows) { "USERPROFILE" } else { "HOME" }
}

/// One test function, deliberately.
///
/// Environment variables are process-global and Rust runs the tests in a
/// binary concurrently, so two functions mutating the same variable would
/// race and fail for reasons that have nothing to do with the code. Putting
/// both cases in one function removes the race rather than papering over it
/// with a lock.
#[test]
fn the_root_comes_from_the_environment_and_falls_back_to_the_home_directory() {
    let original = std::env::var_os(VARIABLE);

    // SAFETY: this binary carries exactly one test, so nothing else in the
    // process is reading or writing the environment while these lines run.
    unsafe { std::env::set_var(VARIABLE, "/somewhere/models") };
    assert_eq!(
        models_root().expect("a configured root is used as given"),
        PathBuf::from("/somewhere/models"),
        "the variable wins when it is set"
    );

    unsafe { std::env::remove_var(VARIABLE) };
    let home = std::env::var_os(home_variable()).expect("this machine has a home directory");
    assert_eq!(
        models_root().expect("the fallback applies"),
        PathBuf::from(home).join("models"),
        "otherwise 'models' under the home directory, which is where the \
         current router already looks"
    );

    unsafe {
        match original {
            Some(value) => std::env::set_var(VARIABLE, value),
            None => std::env::remove_var(VARIABLE),
        }
    }
}
