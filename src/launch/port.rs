//! Choosing a loopback port for a child to bind.
//!
//! Split from `server` because it is a different question -- which number to
//! hand a child, against how to spawn one and wait for it -- and because the
//! module-size gate said so: `server` sat at exactly its 250 lines, and the
//! retry below could not be written without taking it over. The gate exposed
//! the seam rather than inventing it, as it did for `endpoint` beside `head`.

use std::net::TcpListener;

use super::invocation;

/// A loopback port the operating system says is free.
///
/// Binding zero, reading the assignment and closing leaves a window in which
/// something else can take the port before the child binds it. That race is
/// real and this module does not pretend otherwise.
///
/// It cannot be closed here. Handing the child the listener would close it,
/// but `llama-server` binds the port itself from `--port` and has no way to
/// accept one; passing a descriptor is not portable to Windows either. A
/// fixed base port with an offset trades this race for a worse one, against
/// whatever else on the machine already holds that range.
///
/// So the caller retries, which is [`super::Server::start`]'s job, and this
/// only ever answers with what was free a moment ago.
///
/// # Errors
///
/// Returns the error when no loopback port could be bound at all.
pub(super) fn free_port() -> std::io::Result<u16> {
    let listener = TcpListener::bind((invocation::HOST, 0))?;
    let port = listener.local_addr()?.port();
    Ok(port)
}

#[cfg(test)]
mod tests {
    use super::free_port;

    #[test]
    fn two_ports_asked_for_in_a_row_are_not_the_same_one() {
        // Not a guarantee the operating system makes, but the behaviour every
        // caller depends on: a second child must not be handed the first
        // child's port while the first is still starting.
        let first = free_port().expect("a loopback port");
        let second = free_port().expect("a second loopback port");

        assert_ne!(
            first, second,
            "consecutive calls hand out different ports, or two children race \
             for one number before either has bound it"
        );
    }
}
