//! What the router says about memory before it serves anything.
//!
//! Two lines, and the arithmetic behind them. They live here rather than in
//! the command that prints them because what they say is load-bearing -- an
//! operator sets a ceiling from these numbers -- and a line composed inside a
//! binary is a line no test can read.
//!
//! Reporting rather than policy, which is why this is not in `admission`:
//! nothing here decides anything. It takes what the budget and the catalog
//! already know and says it in the order an operator needs to hear it.

/// Whether anything will ever be evicted.
///
/// Said at startup rather than left to be discovered: the difference between a
/// router that swaps models and one that fills memory until a load fails is
/// this single value, and an operator who mistyped the variable would
/// otherwise find out only under load.
#[must_use]
pub fn budget(limit_mib: Option<u32>) -> String {
    match limit_mib {
        Some(limit) => format!("memory budget: {limit} MiB, so models are unloaded to make room"),
        None => "memory budget: none set, so nothing is ever unloaded \
                 (set MAESTRO_MEMORY_BUDGET_MIB)"
            .to_owned(),
    }
}

/// What the residents hold, and what that leaves for everything else.
///
/// A resident is memory the router promises never to reclaim, so a ceiling
/// that covers the residents but not the largest model beside them refuses
/// that model permanently. An operator who learns that from a refusal under
/// load learns it too late, which is what this line exists to prevent.
#[must_use]
pub fn reservation(limit_mib: Option<u32>, reserved_mib: u64) -> String {
    let Some(limit) = limit_mib.map(u64::from) else {
        return format!("residents reserve {reserved_mib} MiB, against no budget");
    };
    match limit.checked_sub(reserved_mib) {
        Some(left) => format!(
            "residents reserve {reserved_mib} MiB of {limit} MiB, \
             leaving {left} MiB for everything else"
        ),
        None => format!(
            "residents reserve {reserved_mib} MiB, more than the {limit} MiB \
             budget: what does not fit is refused"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Three branches of one function, in one case deliberately.
    ///
    /// Three functions differing only in the numbers they pass and the words
    /// they look for is the shape the duplication gate exists to catch, and
    /// splitting them would say the same thing three times without saying
    /// why. What each branch must carry is the same: the reservation, and
    /// enough of the arithmetic that an operator can act on it.
    #[test]
    fn a_reservation_says_what_it_holds_and_what_that_leaves() {
        let fits = reservation(Some(25_000), 4_096);
        assert!(
            fits.contains("4096") && fits.contains("25000") && fits.contains("20904"),
            "what is reserved, the ceiling, and what is left for everything \
             else -- the last is the number that decides whether the largest \
             entry can ever run: {fits}"
        );

        let over = reservation(Some(2_048), 4_096);
        assert!(
            over.contains("4096") && over.contains("2048") && !over.contains("leaving"),
            "a reservation larger than the budget is stated plainly rather \
             than subtracted into a number that wrapped. Both numbers survive \
             a wrapping subtraction, so what is asserted is the absence of the \
             clause that would carry the wrapped value: {over}"
        );

        let unbounded = reservation(None, 4_096);
        assert!(
            unbounded.contains("4096"),
            "residents are held whether or not a ceiling exists, so what they \
             hold is still worth saying: {unbounded}"
        );
    }

    #[test]
    fn the_budget_line_says_whether_anything_is_ever_unloaded() {
        assert!(
            budget(Some(25_000)).contains("25000"),
            "the ceiling, as configured"
        );
        assert!(
            budget(None).contains("MAESTRO_MEMORY_BUDGET_MIB"),
            "and when there is none, the variable to set -- a line saying \
             only that nothing is evicted leaves the reader nowhere to go"
        );
    }
}
