//! What an entry can be asked for, as against what it costs to hold.
//!
//! Two questions a client needs answered before it picks a model, and neither
//! is in the weights: the same file serves as a generator or as an embedder
//! depending only on how the server was started, and a model is multimodal
//! because a projector was named beside it. Both are properties of the entry,
//! so both are read from the entry.
//!
//! Kept apart from `estimate::served`, which reads some of the same flags for
//! a different purpose. That one decides what an entry costs the device; this
//! one decides what a caller may send it. They would diverge the moment a
//! server grew a mode that kept no cache and still generated, and the reason
//! they read alike today is that llama.cpp happens to spell both with one
//! switch.

use super::Entry;

impl Entry {
    /// Whether this entry answers with generated text.
    ///
    /// A server started with `embeddings` returns a vector, and one started
    /// with `reranking` returns scores. Neither holds a conversation, so
    /// neither belongs on a surface a client picks a chat model from.
    ///
    /// Read from the flags the server keys on, as the estimator reads them for
    /// what an entry costs. The file cannot say this: the same weights serve
    /// either way, and only the way it is started decides which.
    #[must_use]
    pub fn generates(&self) -> bool {
        !["embeddings", "embedding", "reranking", "rerank"]
            .iter()
            .filter_map(|name| self.flags.get(*name))
            .any(|value| !matches!(value.trim().to_ascii_lowercase().as_str(), "false" | "0"))
    }

    /// What may be sent to this entry, in the vocabulary a client reads.
    ///
    /// A projector is what makes a model multimodal, so naming one in the
    /// catalog is the whole condition. Text is always accepted and is always
    /// first, because a client reading the list takes it as an ordered set of
    /// what it may send rather than as a set of what it must.
    #[must_use]
    pub fn accepts(&self) -> Vec<&'static str> {
        if self.projector_path.is_some() {
            vec!["text", "image"]
        } else {
            vec!["text"]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::{RelativePath, Residency};
    use std::collections::BTreeMap;

    fn entry(flags: &[(&str, &str)], projector: bool) -> Entry {
        Entry {
            id: "alpha".to_owned(),
            path: RelativePath::new("a/model.gguf").expect("relative"),
            draft_path: None,
            projector_path: projector
                .then(|| RelativePath::new("a/mmproj.gguf").expect("relative")),
            context_size: 4096,
            residency: Residency::OnDemand,
            memory_estimate_mib: 512,
            reasoning_format: None,
            reasoning_effort: None,
            startup_timeout_seconds: 30,
            runtime: None,
            flags: flags
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect::<BTreeMap<_, _>>(),
        }
    }

    #[test]
    fn only_a_server_started_to_generate_is_offered_as_one() {
        assert!(entry(&[], false).generates(), "the ordinary case");
        assert!(
            !entry(&[("embeddings", "true")], false).generates(),
            "an embedding server returns a vector, not a conversation"
        );
        assert!(
            !entry(&[("reranking", "true")], false).generates(),
            "and a reranker returns scores"
        );
        assert!(
            entry(&[("embeddings", "false")], false).generates(),
            "an explicit false is not the mode, as the flags read elsewhere"
        );
        assert!(
            entry(&[("embeddings", "0")], false).generates(),
            "nor is a zero"
        );
    }

    #[test]
    fn a_projector_is_the_whole_condition_for_taking_an_image() {
        assert_eq!(entry(&[], false).accepts(), vec!["text"]);
        assert_eq!(
            entry(&[], true).accepts(),
            vec!["text", "image"],
            "text stays first: a client reads the list as what it may send, \
             in order, not as a set"
        );
    }
}
