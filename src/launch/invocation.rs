//! The command line one catalog entry becomes.
//!
//! Pure translation: an entry, a models root and a port in, arguments out.
//! Nothing here spawns, reads a file, or touches the network, which is what
//! lets the parity rules below be asserted directly rather than inferred from
//! a running process.
//!
//! Parity with the current Python router is the whole point of this module,
//! so the rules are read from `llama_switchyard/core.py` and `config/models.ini`
//! rather than recalled:
//!
//! | Catalog field | Argument | Evidence |
//! | --- | --- | --- |
//! | `path` | `--model` | `models.ini`, `model =` |
//! | `draft_path` | `--model-draft` | `models.ini`, `model-draft =` |
//! | `projector_path` | `--mmproj` | `models.ini`, `mmproj =` |
//! | `context_size` | `--ctx-size` | `SHORT_FLAGS`, `c` maps to `--ctx-size` |
//! | `reasoning_format` | `--reasoning-format` | `models.ini` |
//! | `reasoning_effort` | `--reasoning-effort` | `models.ini` |
//! | identifier | `--alias` | `model_command`, `--alias model.alias` |
//!
//! The flags table follows `_option_arguments`, which has three cases and one
//! lookup: `true` yields the bare flag, `false` yields `--no-` and the key as
//! written, anything else yields the flag and the value as two arguments.
//! Five keys are short forms with different long spellings.

// Nothing outside the unit tests below calls into this module until `start`
// spawns a child with what it builds. Removed in that commit; until then the
// alternative is contrived wiring written only to satisfy a lint.
#![allow(dead_code)]

use std::ffi::OsString;
use std::path::Path;

use crate::catalog::Entry;

/// Every child binds loopback. The current router refuses a remote bind
/// without a separate security design, and that refusal is carried over here
/// rather than re-decided.
pub(super) const HOST: &str = "127.0.0.1";

/// The command line for one entry, without the binary that runs it.
pub(super) fn of(_entry: &Entry, _root: &Path, _port: u16) -> Vec<OsString> {
    todo!("the translation arrives with the invocation")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::{RelativePath, Residency};
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    /// A synthetic root, per the estate's path rule: it names a shape rather
    /// than a machine, and nothing here touches the filesystem.
    const ROOT: &str = "/somewhere/models";
    const PORT: u16 = 8080;

    fn entry() -> Entry {
        Entry {
            id: "gemma3".to_owned(),
            path: RelativePath::new("cache/gemma/gemma-3-1b.gguf").expect("relative"),
            draft_path: None,
            projector_path: None,
            context_size: 4096,
            residency: Residency::OnDemand,
            memory_estimate_mib: 512,
            reasoning_format: None,
            reasoning_effort: None,
            startup_timeout_seconds: 300,
            flags: BTreeMap::new(),
        }
    }

    fn line(entry: &Entry) -> Vec<String> {
        of(entry, Path::new(ROOT), PORT)
            .iter()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect()
    }

    /// Whether the flag and its value sit next to each other.
    ///
    /// Scanning for the pair rather than looking a flag up once: `--ctx-size`
    /// can legitimately appear twice, from the field and from a `c` flag, and
    /// an assertion that found only the first would pass while reading the
    /// wrong one.
    fn has_pair(line: &[String], flag: &str, value: &str) -> bool {
        line.windows(2)
            .any(|pair| pair[0] == flag && pair[1] == value)
    }

    fn resolved(relative: &str) -> String {
        PathBuf::from(ROOT)
            .join(relative)
            .to_string_lossy()
            .into_owned()
    }

    #[test]
    fn the_fixed_arguments_follow_the_current_router() {
        let line = line(&entry());

        assert!(
            has_pair(&line, "--model", &resolved("cache/gemma/gemma-3-1b.gguf")),
            "the model resolves against the root:\n{line:?}"
        );
        assert!(
            has_pair(&line, "--ctx-size", "4096"),
            "context_size becomes --ctx-size:\n{line:?}"
        );
        assert!(
            has_pair(&line, "--alias", "gemma3"),
            "the identifier becomes --alias, so a reply names the model asked for:\n{line:?}"
        );
        assert!(
            has_pair(&line, "--host", "127.0.0.1"),
            "loopback only:\n{line:?}"
        );
        assert!(
            has_pair(&line, "--port", "8080"),
            "the port the caller chose:\n{line:?}"
        );
    }

    #[test]
    fn an_omitted_location_is_absent_rather_than_empty() {
        let line = line(&entry());
        assert!(
            !line.iter().any(|a| a == "--model-draft"),
            "no draft model, so no flag at all:\n{line:?}"
        );
        assert!(
            !line.iter().any(|a| a == "--mmproj"),
            "no projector, so no flag at all:\n{line:?}"
        );
    }

    #[test]
    fn the_optional_locations_resolve_against_the_root_when_present() {
        let mut entry = entry();
        entry.draft_path = Some(RelativePath::new("llm/qwen/mtp.gguf").expect("relative"));
        entry.projector_path = Some(RelativePath::new("llm/qwen/mmproj.gguf").expect("relative"));
        let line = line(&entry);

        assert!(
            has_pair(&line, "--model-draft", &resolved("llm/qwen/mtp.gguf")),
            "the speculative draft:\n{line:?}"
        );
        assert!(
            has_pair(&line, "--mmproj", &resolved("llm/qwen/mmproj.gguf")),
            "the multimodal projector:\n{line:?}"
        );
    }

    #[test]
    fn reasoning_settings_appear_only_when_the_entry_sets_them() {
        let bare = line(&entry());
        assert!(
            !bare.iter().any(|a| a == "--reasoning-format"),
            "a model that does not reason carries neither:\n{bare:?}"
        );
        assert!(!bare.iter().any(|a| a == "--reasoning-effort"));

        let mut entry = entry();
        entry.reasoning_format = Some("deepseek".to_owned());
        entry.reasoning_effort = Some("low".to_owned());
        let line = line(&entry);

        assert!(
            has_pair(&line, "--reasoning-format", "deepseek"),
            "{line:?}"
        );
        assert!(has_pair(&line, "--reasoning-effort", "low"), "{line:?}");
    }

    #[test]
    fn a_true_flag_becomes_the_bare_flag() {
        let mut entry = entry();
        entry.flags.insert("jinja".to_owned(), "true".to_owned());
        let line = line(&entry);

        assert!(
            line.iter().any(|a| a == "--jinja"),
            "true means the flag on its own:\n{line:?}"
        );
        assert!(
            !line.iter().any(|a| a == "true"),
            "and never the word as a value:\n{line:?}"
        );
    }

    #[test]
    fn a_false_flag_becomes_the_negated_flag_spelled_from_the_key() {
        let mut entry = entry();
        entry.flags.insert("mmap".to_owned(), "false".to_owned());
        entry.flags.insert("fa".to_owned(), "false".to_owned());
        let line = line(&entry);

        assert!(
            line.iter().any(|a| a == "--no-mmap"),
            "false negates the flag:\n{line:?}"
        );
        assert!(
            line.iter().any(|a| a == "--no-fa"),
            "and negation is spelled from the key as written, not its long \
             form, which is what the current router sends:\n{line:?}"
        );
    }

    #[test]
    fn any_other_value_becomes_a_flag_and_its_value() {
        let mut entry = entry();
        entry
            .flags
            .insert("n-gpu-layers".to_owned(), "999".to_owned());
        let line = line(&entry);

        assert!(has_pair(&line, "--n-gpu-layers", "999"), "{line:?}");
    }

    #[test]
    fn the_five_short_forms_use_their_long_spellings() {
        let mut entry = entry();
        for (key, value) in [
            ("c", "999"),
            ("ctk", "q8_0"),
            ("ctv", "q8_0"),
            ("fa", "auto"),
            ("np", "1"),
        ] {
            entry.flags.insert(key.to_owned(), value.to_owned());
        }
        let line = line(&entry);

        for (long, value) in [
            ("--ctx-size", "999"),
            ("--cache-type-k", "q8_0"),
            ("--cache-type-v", "q8_0"),
            ("--flash-attn", "auto"),
            ("--parallel", "1"),
        ] {
            assert!(
                has_pair(&line, long, value),
                "the short form must be spelled '{long}':\n{line:?}"
            );
        }
    }

    /// `on` is neither `true` nor `false`, so it takes the value branch. The
    /// current router sends `--flash-attn on` today, and a boolean reading
    /// here would silently drop the value.
    #[test]
    fn flash_attention_on_is_a_value_not_a_boolean() {
        let mut entry = entry();
        entry.flags.insert("fa".to_owned(), "on".to_owned());
        let line = line(&entry);

        assert!(has_pair(&line, "--flash-attn", "on"), "{line:?}");
        assert!(
            !line.iter().any(|a| a == "--no-fa"),
            "'on' is not falsehood:\n{line:?}"
        );
    }

    #[test]
    fn the_command_line_is_stable_across_runs() {
        let mut entry = entry();
        for key in ["zebra", "alpha", "mike"] {
            entry.flags.insert(key.to_owned(), "1".to_owned());
        }
        assert_eq!(
            line(&entry),
            line(&entry),
            "two runs of the same entry produce the same command line"
        );
    }
}
