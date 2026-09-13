//! What an entry can be asked for, as against what it costs to hold.
//!
//! Two questions a client needs answered before it picks a model, and neither
//! is in the weights: the same file serves as a generator or as an embedder
//! depending only on how the server was started, and a model is multimodal
//! because a projector was named beside it. Both are properties of the entry,
//! so both are read from the entry.
//!
//! A third source joined them when the catalog grew entries that are not
//! llama.cpp at all. Each is read the same way: whatever *causes* the
//! behaviour is what gets read, rather than a label sitting beside it.
//!
//! Kept apart from `estimate::served`, which reads some of the same flags for
//! a different purpose. That one decides what an entry costs the device; this
//! one decides what a caller may send it. They would diverge the moment a
//! server grew a mode that kept no cache and still generated, and the reason
//! they read alike today is that llama.cpp happens to spell both with one
//! switch.

use super::Entry;

/// The runtimes that serve speech, and what each one takes.
///
/// A runtime names a binary beside `llama-server`, and for these two that
/// binary is not a build of llama.cpp: it is a shim serving transcription or
/// synthesis. So the runtime is what decides here, the way a projector decides
/// multimodality and the `embeddings` flag decides a vector -- the cause, not a
/// label beside it. The flags cannot answer: a shim ignores the flags table
/// wholesale, so llama.cpp's vocabulary has no word for what it is.
///
/// A closed list of two, which is what reading the cause costs. A build named
/// `whisper-cuda` matches nothing here and would be offered as a chat model
/// again. Reopen this when a third speech runtime arrives or a variant needs
/// naming: an entry field earns its keep once the names outnumber the cases,
/// and until then it would be a second way to state a fact the runtime already
/// settles, free to disagree with it and with no way to tell which was meant.
const SPEECH: [(&str, &str); 2] = [("whisper", "audio"), ("tts", "text")];

impl Entry {
    /// What this entry takes when a speech runtime serves it, or `None` when
    /// an ordinary server does.
    fn speech_input(&self) -> Option<&'static str> {
        let runtime = self.runtime.as_deref()?;
        SPEECH
            .iter()
            .find_map(|(name, takes)| (*name == runtime).then_some(*takes))
    }

    /// Whether this entry answers with generated text.
    ///
    /// A server started with `embeddings` returns a vector, and one started
    /// with `reranking` returns scores. Neither holds a conversation, so
    /// neither belongs on a surface a client picks a chat model from.
    ///
    /// Read from the flags the server keys on, as the estimator reads them for
    /// what an entry costs. The file cannot say this: the same weights serve
    /// either way, and only the way it is started decides which.
    ///
    /// Neither speech runtime generates either, for two different reasons that
    /// reach the same answer. Synthesis answers with a waveform rather than
    /// with text. Transcription does answer with text, but only ever the text
    /// of one recording: there is no chat endpoint behind that shim, so a
    /// client offered it would be choosing something it cannot then talk to.
    #[must_use]
    pub fn generates(&self) -> bool {
        self.speech_input().is_none()
            && !["embeddings", "embedding", "reranking", "rerank"]
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
    ///
    /// A speech runtime is the one case where text is not accepted at all:
    /// transcription takes a recording and nothing else. Answered here rather
    /// than left to the caller because this is the vocabulary the surface
    /// already reports, and a second one for audio would say the same thing
    /// twice.
    #[must_use]
    pub fn accepts(&self) -> Vec<&'static str> {
        if let Some(takes) = self.speech_input() {
            vec![takes]
        } else if self.projector_path.is_some() {
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

    /// The same fixture with a runtime name, which is all a speech entry has
    /// to say what it is.
    fn speech(runtime: &str) -> Entry {
        Entry {
            runtime: Some(runtime.to_owned()),
            ..entry(&[], false)
        }
    }

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
    fn a_speech_service_holds_no_conversation() {
        assert!(
            !speech("whisper").generates(),
            "transcription answers once with the text of an utterance; there \
             is no chat endpoint behind that shim to hold a conversation with"
        );
        assert!(
            !speech("tts").generates(),
            "and synthesis answers with a waveform, not with text at all"
        );
    }

    #[test]
    fn a_speech_service_says_what_it_actually_takes() {
        assert_eq!(
            speech("whisper").accepts(),
            vec!["audio"],
            "transcription takes a recording. Saying 'text' would describe \
             the one thing it cannot be sent"
        );
        assert_eq!(
            speech("tts").accepts(),
            vec!["text"],
            "synthesis does take text -- it is what it answers with that is \
             not text, which is why it is absent from the menu rather than \
             described differently on it"
        );
    }

    #[test]
    fn a_runtime_naming_a_server_build_is_still_an_ordinary_model() {
        // The field's documented purpose: a patched llama-server, selected by
        // name. Such an entry generates text and takes text like any other,
        // and reading every runtime as a speech service would hide it.
        let patched = speech("vulkan");
        assert!(patched.generates(), "a build of the server still generates");
        assert_eq!(patched.accepts(), vec!["text"]);
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
