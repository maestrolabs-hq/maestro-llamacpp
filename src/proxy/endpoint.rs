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

use super::refusal::{Cause, Refusal};

/// The path shape a dedicated endpoint carries.
const DEDICATED: &str = "/models/";

/// The path shape the generic endpoint carries.
const GENERIC: &str = "/v1/";

/// The path the router answers from its own catalog.
const LISTING: &str = "/v1/models";

/// The path a llama.cpp client reads the catalogue from, in router mode.
///
/// The same word as [`DEDICATED`] without a model after it, which is what
/// makes the two tell apart cleanly: a dedicated request always carries a
/// model *and* a path after it, so `/models` alone can only be this.
const CATALOGUE: &str = "/models";

/// The path a llama.cpp client reads the server's own settings from.
const PROPERTIES: &str = "/props";

/// Which endpoint a path addressed, and what the child is asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Endpoint {
    /// `/models/<id>/<suffix>`: the path names the model.
    Dedicated { id: String, suffix: String },
    /// `/v1/<suffix>`: the body names the model.
    Generic { suffix: String },
    /// `/v1/models`: the router answers this itself.
    Listing,
    /// `/models`: the same question in the shape a llama.cpp client asks it,
    /// which carries each entry's status rather than only its name.
    Catalogue,
    /// `/props`: what the server itself does, which is how a client decides
    /// whether it is talking to a router at all.
    Properties,
    /// `/models/<id>`: the entry itself rather than something under it. What
    /// can be asked of it is whether it is resident -- see ADR 0002.
    Residency { id: String },
}

impl Endpoint {
    /// The shape a path addresses.
    ///
    /// # Errors
    ///
    /// Returns a [`Refusal`] when the path is neither shape, or when it names
    /// a model with nothing after it.
    pub(super) fn of(path: &str) -> Result<Self, Refusal> {
        // Before the dedicated shape, which would otherwise read `/models/`
        // as a model named nothing. A trailing slash is the same request
        // either way: a client that adds one is not asking for something
        // else.
        let bare = path.trim_end_matches('/');
        if bare == CATALOGUE {
            return Ok(Self::Catalogue);
        }
        if bare == PROPERTIES {
            return Ok(Self::Properties);
        }

        if let Some(rest) = path.strip_prefix(DEDICATED) {
            let named = rest.trim_end_matches('/');
            return match rest.split_once('/') {
                Some((id, suffix)) if !id.is_empty() && !suffix.is_empty() => Ok(Self::Dedicated {
                    id: id.to_owned(),
                    suffix: format!("/{suffix}"),
                }),
                // `/models/<id>`, with or without a trailing slash: the entry
                // and nothing under it. This was a refusal until residency
                // became something an operator could ask about, which is why
                // the spelling was free to take.
                _ if !named.is_empty() && !named.contains('/') => Ok(Self::Residency {
                    id: named.to_owned(),
                }),
                _ => Err(malformed(&format!(
                    "'{path}' names a model with nothing after it: the shape \
                     is {DEDICATED}<model>/<path>"
                ))),
            };
        }

        // Before the generic shape, because the listing sits inside it and is
        // answered from the catalog rather than by a child. Trailing slashes
        // and a query are the same request: a client that adds either is not
        // asking for something else, and a client library that pages its
        // model list adds a query without being asked.
        let bare = path.split('?').next().unwrap_or(path);
        if bare.trim_end_matches('/') == LISTING {
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

    /// The methods this endpoint answers, as an `Allow` header says them.
    ///
    /// A model is asked something with `POST`, and asked about itself with
    /// `GET`; the listing is read, and `HEAD` reads its head. A `DELETE`
    /// under a model prefix is refused for the reason it always was -- the
    /// only thing it could do there is start a model that then answers 404,
    /// which is a load nobody asked for. It means something only at the one
    /// path that names an entry and nothing under it, where it can be
    /// answered without starting anything.
    pub(super) fn allowed(&self) -> &'static str {
        match self {
            // The three the router answers out of its own catalog. Nothing is
            // sent upstream and nothing is written, so they take the same
            // read-only set.
            Self::Listing | Self::Catalogue | Self::Properties => "GET, HEAD, OPTIONS",
            Self::Dedicated { .. } | Self::Generic { .. } => "GET, POST, OPTIONS",
            Self::Residency { .. } => "DELETE, OPTIONS",
        }
    }

    /// Whether a method is one this endpoint answers.
    pub(super) fn allows(&self, method: &str) -> bool {
        self.allowed().split(", ").any(|allowed| allowed == method)
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
            // Residency shares the catalogue's prefix because the honest
            // answer, `/models/<id>`, is not a static string -- and like the
            // three above it, this is never what a child is asked for.
            Self::Catalogue | Self::Residency { .. } => CATALOGUE,
            Self::Properties => PROPERTIES,
        }
    }
}

/// A request this router does not serve, and the shapes of the ones it does.
fn malformed(reason: &str) -> Refusal {
    Refusal::new(Cause::PathNotFound, reason)
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
        for path in [LISTING, "/v1/models/", "/v1/models?limit=100"] {
            assert_eq!(
                Endpoint::of(path).expect("a listing"),
                Endpoint::Listing,
                "'{path}' lists the catalog, which needs no child"
            );
        }
    }

    #[test]
    fn a_query_stays_on_a_path_a_child_answers() {
        let dedicated = Endpoint::of("/models/gemma3/v1/chat/completions?x=1").expect("dedicated");

        assert_eq!(
            dedicated.suffix(),
            "/v1/chat/completions?x=1",
            "only the listing is the router's to read; what a child is asked \
             for is the caller's own path, query and all"
        );
    }

    #[test]
    fn only_the_methods_a_model_can_be_asked_with_are_allowed() {
        let dedicated = Endpoint::of("/models/gemma3/v1/chat/completions").expect("dedicated");

        assert!(dedicated.allows("POST") && dedicated.allows("GET"));
        assert!(
            !dedicated.allows("DELETE") && !dedicated.allows("HEAD"),
            "a method that could not be an inference is refused before a \
             child is started for it"
        );
        assert!(
            Endpoint::Listing.allows("HEAD") && !Endpoint::Listing.allows("POST"),
            "the listing is read, never asked"
        );
    }

    /// This path was a refusal until ADR 0002 gave it a meaning: an identifier
    /// with nothing after it names the entry itself, and residency is what can
    /// be asked of it. A trailing slash is the same request.
    #[test]
    fn a_models_path_with_nothing_after_the_identifier_names_the_entry_itself() {
        for path in ["/models/gemma3", "/models/gemma3/"] {
            assert_eq!(
                Endpoint::of(path).expect("the entry itself"),
                Endpoint::Residency {
                    id: "gemma3".to_owned()
                },
                "'{path}' names the entry and nothing under it"
            );
        }
        assert_eq!(
            Endpoint::Residency {
                id: "gemma3".to_owned()
            }
            .allowed(),
            "DELETE, OPTIONS",
            "and the only thing it answers is being given up: a GET here would \
             be the catalogue's job, and a POST a load nobody asked for"
        );
    }

    /// `/models//` is not here: trailing slashes are trimmed before this and
    /// it reads as the catalogue, which it did before residency existed.
    #[test]
    fn a_models_path_naming_no_entry_at_all_is_still_refused() {
        let refusal =
            Endpoint::of("/models//v1/echo").expect_err("an empty identifier asks for nothing");

        assert!(
            refusal.to_string().contains(DEDICATED),
            "the refusal names the shape it wanted: {refusal}"
        );
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
