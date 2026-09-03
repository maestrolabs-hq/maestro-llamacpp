//! Which endpoint a path addressed.
//!
//! Split from the head beside it for two reasons. The file was at 206 code
//! lines against a limit of 250, and a second path shape would have taken it
//! over. More than that, which endpoint a path names is a different question
//! from what a head contains: one is about routing, the other about framing,
//! and the gate exposed the seam rather than inventing it.
//!
//! Pure translation, like the head: a string in, a shape out. No socket, no
//! process, no catalog. Whether the model a shape names actually exists is the
//! caller's question, asked against the catalog after this has answered.

use crate::launch::Failure;

/// The path shape a dedicated endpoint carries.
const DEDICATED: &str = "/models/";

/// The path shape the generic endpoint carries.
const GENERIC: &str = "/v1/";

/// The path the router answers from its own catalog.
const LISTING: &str = "/v1/models";

/// Which endpoint a path addressed, and what the child is asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Endpoint {
    /// `/models/<id>/<suffix>`: the path names the model.
    Dedicated { id: String, suffix: String },
    /// `/v1/<suffix>`: the body names the model.
    Generic { suffix: String },
    /// `/v1/models`: the router answers this itself.
    Listing,
}

impl Endpoint {
    /// The shape a path addresses.
    ///
    /// # Errors
    ///
    /// Returns a [`Failure`] when the path is neither shape, or when it names
    /// a model with nothing after it.
    pub(super) fn of(path: &str) -> Result<Self, Failure> {
        if let Some(rest) = path.strip_prefix(DEDICATED) {
            return match rest.split_once('/') {
                Some((id, suffix)) if !id.is_empty() && !suffix.is_empty() => Ok(Self::Dedicated {
                    id: id.to_owned(),
                    suffix: format!("/{suffix}"),
                }),
                _ => Err(malformed(&format!(
                    "'{path}' names a model with nothing after it: the shape \
                     is {DEDICATED}<model>/<path>"
                ))),
            };
        }

        // Before the generic shape, because the listing sits inside it and is
        // answered from the catalog rather than by a child. Trailing slashes
        // are the same request: a client that adds one is not asking for
        // something else.
        if path.trim_end_matches('/') == LISTING {
            return Ok(Self::Listing);
        }

        if path.starts_with(GENERIC) {
            return Ok(Self::Generic {
                suffix: path.to_owned(),
            });
        }

        Err(malformed(&format!(
            "'{path}' is not a path this router serves: the shapes are \
             {DEDICATED}<model>/<path> and {GENERIC}<path>"
        )))
    }

    /// What the child is asked for.
    ///
    /// The generic endpoint strips nothing: the caller's own path is what the
    /// child receives, because the model was named in the body rather than
    /// taken out of the path.
    pub(super) fn suffix(&self) -> &str {
        match self {
            Self::Dedicated { suffix, .. } | Self::Generic { suffix } => suffix,
            // Never sent upstream: the router answers a listing itself. Given
            // its own path rather than an empty string so the value is honest
            // if anything ever reads it.
            Self::Listing => LISTING,
        }
    }
}

/// A request this router does not serve, and the shapes of the ones it does.
fn malformed(reason: &str) -> Failure {
    Failure::Unavailable(reason.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_models_path_names_the_model_and_what_follows_it() {
        assert_eq!(
            Endpoint::of("/models/gemma3/v1/chat/completions").expect("a dedicated endpoint"),
            Endpoint::Dedicated {
                id: "gemma3".to_owned(),
                suffix: "/v1/chat/completions".to_owned(),
            }
        );
    }

    #[test]
    fn a_v1_path_is_generic_and_keeps_the_whole_path() {
        assert_eq!(
            Endpoint::of("/v1/chat/completions").expect("the generic endpoint"),
            Endpoint::Generic {
                suffix: "/v1/chat/completions".to_owned(),
            },
            "the generic endpoint strips no prefix: the model was named in the \
             body, so the path the caller sent is the path the child gets"
        );
    }

    #[test]
    fn the_models_listing_is_answered_by_the_router_itself() {
        for path in [LISTING, "/v1/models/"] {
            assert_eq!(
                Endpoint::of(path).expect("a listing"),
                Endpoint::Listing,
                "'{path}' lists the catalog, which needs no child"
            );
        }
    }

    #[test]
    fn a_models_path_with_nothing_after_the_identifier_is_refused() {
        for path in ["/models/gemma3", "/models/gemma3/"] {
            let refusal = Endpoint::of(path)
                .expect_err("an identifier with nothing after it asks for nothing");
            assert!(
                refusal.to_string().contains("gemma3"),
                "the refusal names what was asked for: {refusal}"
            );
        }
    }

    #[test]
    fn a_path_that_is_neither_shape_is_refused_naming_both() {
        let refusal =
            Endpoint::of("/health").expect_err("the router serves two shapes and no others");
        let text = refusal.to_string();

        assert!(
            text.contains(DEDICATED) && text.contains(GENERIC),
            "the refusal names both shapes the router serves: {text}"
        );
    }

    #[test]
    fn the_suffix_is_what_the_child_is_asked_for() {
        let dedicated = Endpoint::of("/models/gemma3/v1/chat/completions").expect("dedicated");
        let generic = Endpoint::of("/v1/chat/completions").expect("generic");

        assert_eq!(dedicated.suffix(), "/v1/chat/completions");
        assert_eq!(
            generic.suffix(),
            "/v1/chat/completions",
            "both reach the child at the same path by different routes"
        );
    }
}
