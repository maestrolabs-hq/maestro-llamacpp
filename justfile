# Optional convenience task runner (https://just.systems).
# Every command below works standalone -- just is never required.

# Our runtime, ahead of whatever the ambient PATH carries. WSL appends the
# Windows PATH by default, which put a broken `just` npm shim in front of the
# real one once already. Recipes resolve tools from our own install first, so
# an inherited PATH cannot decide which binary a gate runs.
#
# Derived, never hardcoded: `home_directory()` resolves on Windows, macOS and
# Linux alike, and the separator follows the OS rather than assuming Unix.
path_sep := if os_family() == "windows" { ";" } else { ":" }
export PATH := home_directory() / ".cargo" / "bin" + path_sep + home_directory() / ".local" / "bin" + path_sep + env('PATH')

# Install the toolchain this repository needs. Idempotent.
install:
    rustup toolchain install --profile minimal 1.98.0
    rustup component add clippy rustfmt
    cargo binstall -y prek cargo-deny cargo-machete similarity-rs

# Wire the local hooks. Both types come from default_install_hook_types.
setup:
    prek install --install-hooks

# Run the quality gates. CI runs these same commands, not equivalents.
check:
    cargo fmt --all --check
    cargo clippy --all-targets --all-features -- -D warnings
    cargo test --all-targets
    cargo machete
    cargo deny check

# Format in place. `check` only verifies.
fmt:
    cargo fmt --all

# Prove the gates do not depend on the ambient PATH.
doctor:
    @echo "just    $(command -v just)"
    @echo "cargo   $(command -v cargo)"
    @echo "prek    $(command -v prek)"
    @echo "rustc   $(rustc --version)"
