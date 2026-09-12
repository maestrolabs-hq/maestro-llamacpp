//! What a file's name says: whether it is a model on its own, and what the
//! entry it becomes is called.
//!
//! Split from the module above it along the seam the size gate exposed: that
//! module walks the root and builds entries, and this decides two things
//! about one name without touching the file behind it.

use std::collections::BTreeSet;
use std::path::Path;

use super::super::estimate::shard_of;

/// Identifiers the proxy keeps for its own paths under `/models`.
pub(super) const RESERVED: [&str; 5] = ["load", "unload", "sse", "props", "models"];

/// Whether a file name is a model on its own, by the rules the module above
/// lays out.
pub(super) fn is_model(name: &str) -> bool {
    let is_gguf = Path::new(name)
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("gguf"));
    let lower = name.to_ascii_lowercase();
    if !is_gguf || lower.contains("mmproj") {
        return false;
    }
    if lower
        .split(|c: char| !c.is_ascii_alphanumeric())
        .any(|segment| segment.ends_with("mtp"))
    {
        return false;
    }
    shard_of(name).is_none_or(|shard| shard.index == 1)
}

/// The identifier a file gets, told apart from any name already taken.
pub(super) fn identifier(
    file: &Path,
    taken: &mut BTreeSet<String>,
    notes: &mut Vec<String>,
) -> Option<String> {
    let name = file.file_name()?.to_str()?;
    let stem = match shard_of(name) {
        Some(shard) => shard.stem,
        None => name.rsplit_once('.').map_or(name, |(stem, _)| stem),
    };
    let wanted = sanitised(stem);
    let parent = file
        .parent()
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
        .map(sanitised)
        .unwrap_or_default();
    let id = if wanted.is_empty() {
        return None;
    } else if !taken.contains(&wanted) {
        wanted
    } else {
        let instead = format!("{parent}-{wanted}");
        if parent.is_empty() || taken.contains(&instead) {
            notes.push(format!(
                "skipped '{}': '{wanted}' is already taken, and so is '{instead}'",
                file.display()
            ));
            return None;
        }
        notes.push(format!(
            "discovered '{}' as '{instead}', because '{wanted}' is already taken",
            file.display()
        ));
        instead
    };
    taken.insert(id.clone());
    Some(id)
}

/// Lowercase letters and digits, with every other run collapsed to a hyphen.
fn sanitised(text: &str) -> String {
    let mut out = String::new();
    for c in text.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_matches('-').to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_name_says_whether_it_is_a_model_on_its_own() {
        assert!(is_model("Qwen3-4B-Q4_K_M.gguf"));
        assert!(is_model("Tiny.GGUF"), "the extension is read in any case");
        assert!(
            is_model("Big-00001-of-00003.gguf"),
            "the first shard names the model"
        );
        assert!(
            !is_model("Big-00002-of-00003.gguf"),
            "the rest are its weights"
        );
        assert!(!is_model("mmproj-F16.gguf"), "a projector");
        assert!(!is_model("mtp-Qwen3.8-27B-Q4_0.gguf"), "a draft");
        assert!(
            !is_model("Qwen3.8-27B-FastMTP-32K.gguf"),
            "a draft, named differently"
        );
        assert!(!is_model("README.md"));
    }

    #[test]
    fn an_identifier_is_the_stem_lowercased_and_hyphenated() {
        assert_eq!(sanitised("Qwen3.8-27B-UD-Q6_K"), "qwen3-8-27b-ud-q6-k");
        assert_eq!(sanitised("--Odd__Name--"), "odd-name");
        assert_eq!(
            sanitised("..."),
            "",
            "nothing left is nothing, not a hyphen"
        );
    }
}
