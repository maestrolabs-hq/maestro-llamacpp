//! What the flags say about the way an entry is served.
//!
//! The file describes a model. It cannot say how the model will be run, and
//! three of the ways it can be run cost the device nothing like what the file
//! implies: a server told to embed keeps no cache, a draft named as a
//! prediction head keeps no cache of its own, and layers pinned to the
//! processor are not on the device at all.
//!
//! Each is read from the flag `llama-server` itself keys on rather than
//! inferred from the metadata, because in every one of these cases the
//! metadata is either silent or actively misleading -- a prediction head
//! reports its parent's layer count, and a model pinned to the processor
//! reports the same shape it would have had on the device.
//!
//! They live together because they answer one question in three forms: what
//! this way of running it will actually cost, as against what the weights
//! suggest it could.

use std::collections::BTreeMap;

/// Whether the entry is served in a way that keeps no key-value cache.
///
/// A server started with `embeddings` answers one forward pass at a time and
/// keeps nothing between them; one started with `reranking` scores a query
/// against a passage and forgets both. Neither has a conversation to remember,
/// so the cache a generative entry holds for the length of a session is never
/// allocated -- and the context size, which for those entries is the largest
/// term in the sum, here only bounds how long one passage may be.
///
/// Read from the flags the server itself keys on, as `predicts_tokens` beside
/// it reads `spec-type`. Both answer the same shape of question: the file says
/// what a model *could* cost, and the flags say what this way of running it
/// actually will.
///
/// Measured on this estate: bge-m3 at a context of 8192 derived 2428 MiB
/// against the 880 MiB it was found to hold, and the whole of that gap was a
/// cache the child never asked the device for.
pub(super) fn keeps_no_cache(flags: &BTreeMap<String, String>) -> bool {
    ["embeddings", "embedding", "reranking", "rerank"]
        .iter()
        .find_map(|name| flags.get(*name))
        .is_some_and(|value| !matches!(value.trim().to_ascii_lowercase().as_str(), "false" | "0"))
}

/// Whether the entry keeps every layer on the processor.
///
/// `n-gpu-layers = 0` offloads nothing: the weights stay in host memory, the
/// cache is allocated beside them, and the device is left holding the context
/// and the compute buffers alone. The file cannot say this -- it describes a
/// model, not a way of running one -- so it is read from the flag the server
/// keys on, as `keeps_no_cache` and `predicts_tokens` beside it are.
///
/// Only an explicit zero counts. A missing flag means the default, which the
/// catalog sets to 999, and a partial offload is not modelled: splitting a
/// model across both memories costs the device a fraction no flag states, and
/// guessing it would put a figure below the true one into the one term the
/// budget cannot afford to under-read.
///
/// Measured on this estate: qwen3-4b derived 9285 MiB against the 811 MiB it
/// was found to hold, and qwen3-06b 6145 against 795.
pub(super) fn runs_on_processor(flags: &BTreeMap<String, String>) -> bool {
    ["n-gpu-layers", "gpu-layers", "ngl"]
        .iter()
        .find_map(|name| flags.get(*name))
        .is_some_and(|value| value.trim() == "0")
}

/// Whether the draft is a prediction head rather than a model of its own.
///
/// Read from `spec-type`, which is what the server itself keys on, rather than
/// guessed from the draft's size or its layer count -- both of which a sidecar
/// reports as its parent's. An independent draft model, which does keep its
/// own cache, says nothing here and is sized normally.
pub(super) fn predicts_tokens(flags: &BTreeMap<String, String>) -> bool {
    ["spec-type", "speculative-type"]
        .iter()
        .find_map(|name| flags.get(*name))
        .is_some_and(|spelling| spelling.trim().to_ascii_lowercase().contains("mtp"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_an_explicit_zero_pins_an_entry_to_the_processor() {
        let mut flags = BTreeMap::new();
        assert!(
            !runs_on_processor(&flags),
            "an entry that asks for nothing takes the catalog default, which \
             offloads every layer"
        );

        flags.insert("n-gpu-layers".to_owned(), "0".to_owned());
        assert!(
            runs_on_processor(&flags),
            "the server keys on this, so this does too"
        );

        flags.insert("n-gpu-layers".to_owned(), " 0 ".to_owned());
        assert!(runs_on_processor(&flags), "read as the flags are elsewhere");

        flags.insert("n-gpu-layers".to_owned(), "999".to_owned());
        assert!(!runs_on_processor(&flags), "every layer on the device");

        flags.insert("n-gpu-layers".to_owned(), "20".to_owned());
        assert!(
            !runs_on_processor(&flags),
            "a partial offload still costs the device a share, and no flag \
             says how much -- so it is charged in full rather than guessed at \
             below the truth"
        );
    }

    #[test]
    fn a_multiple_token_prediction_draft_is_a_head_not_a_model() {
        let mut flags = BTreeMap::new();
        assert!(
            !predicts_tokens(&flags),
            "an entry that asks for nothing keeps a draft cache, because an \
             independent draft model does have one"
        );

        flags.insert("spec-type".to_owned(), "draft-mtp".to_owned());
        assert!(
            predicts_tokens(&flags),
            "the server keys on spec-type, so this does too -- a sidecar's own \
             metadata cannot be trusted for it, since it reports the parent's \
             layer count"
        );

        flags.insert("spec-type".to_owned(), "  DRAFT-MTP ".to_owned());
        assert!(predicts_tokens(&flags), "read as the flags are elsewhere");

        flags.insert("spec-type".to_owned(), "draft".to_owned());
        assert!(
            !predicts_tokens(&flags),
            "a plain draft is a model of its own and keeps its own cache"
        );
    }
}
