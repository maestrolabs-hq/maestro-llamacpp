//! What loading an entry is expected to cost, worked out from its files.
//!
//! An estimate decides what fits, and one written by hand is the figure most
//! likely to be wrong: the audit that led here found the shipped ones two to
//! four and a half times below what the same models measured once loaded,
//! because nothing derived the cache from the context size. This derives it.
//!
//! The figure is the sum of four terms, in bytes, rounded up to whole
//! mebibytes:
//!
//! - **weights**: the size of every file the entry names -- all the shards of
//!   a split model, the draft model, and the projector;
//! - **cache**: for the model and again for the draft, keys and values for
//!   every layer at the configured context:
//!   `layers x context x kv_heads x (key_length + value_length) x bytes`,
//!   where a missing key length is the embedding width over the head count,
//!   a missing value length is the key length, and bytes per element follows
//!   the cache-type flags (`ctk`/`ctv`, or their long spellings): f16 and
//!   bf16 hold 2, f32 holds 4, `q8_0` holds 1.0625, `q4_0` holds 0.5625, and
//!   any other spelling is read as f16;
//! - **fragmentation**: five percent of the weights, for the allocator's
//!   rounding and the padding between tensors;
//! - **overhead**: a fixed 1024 MiB for the device context and the compute
//!   buffers, which no file records and which a resident that holds no
//!   layers on the device was still measured to pay.
//!
//! A file whose metadata cannot be read is estimated from its size alone: a
//! quarter again on top, plus the overhead. It is rougher, and it is said.
//!
//! Every term errs high on purpose. An estimate above the cost leaves memory
//! idle; one below it admits a model the machine cannot hold, which is the
//! failure the budget exists to prevent.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::gguf::Metadata;

use super::Entry;

const MIB: u64 = 1024 * 1024;

/// The device context and compute buffers.
const OVERHEAD_BYTES: u64 = 1024 * MIB;

/// Bytes per cached element, in sixteenths, so a quantised cache is exact.
const F16: u64 = 32;
const F32: u64 = 64;
const Q8_0: u64 = 17;
const Q4_0: u64 = 9;

/// What the figure was worked out from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Basis {
    /// The metadata the model file carries about itself.
    Metadata,
    /// The size of the files, because the metadata could not be read.
    Size,
}

/// A derived estimate, and what it rests on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Derived {
    pub mib: u32,
    pub basis: Basis,
}

/// Derives the estimate for one entry from the files under `root`.
///
/// # Errors
///
/// Returns the reason when the model file itself cannot be sized, which is
/// the one input without which there is nothing to derive from. A draft or a
/// projector that is not there contributes nothing; the start will name it.
pub(super) fn derive(entry: &Entry, root: &Path) -> Result<Derived, String> {
    let model = entry.path.resolve(root);
    let metadata = Metadata::read(&model);
    let mut weights = weights_of(&model, metadata.as_ref().ok())?;
    let mut cache = 0u128;

    if let Some(draft) = &entry.draft_path {
        let draft = draft.resolve(root);
        weights += size_of(&draft).unwrap_or(0);
        if let Ok(metadata) = Metadata::read(&draft) {
            let keys = sixteenths(&entry.flags, ["ctkd", "cache-type-k-draft"]);
            let values = sixteenths(&entry.flags, ["ctvd", "cache-type-v-draft"]);
            cache += cache_bytes(&metadata, entry.context_size, keys, values).unwrap_or(0);
        }
    }
    if let Some(projector) = &entry.projector_path {
        weights += size_of(&projector.resolve(root)).unwrap_or(0);
    }

    let keys = sixteenths(&entry.flags, ["ctk", "cache-type-k"]);
    let values = sixteenths(&entry.flags, ["ctv", "cache-type-v"]);
    let weights = u128::from(weights);
    let (bytes, basis) = match metadata
        .ok()
        .and_then(|metadata| cache_bytes(&metadata, entry.context_size, keys, values))
    {
        Some(model_cache) => (
            weights + model_cache + cache + weights * 5 / 100 + u128::from(OVERHEAD_BYTES),
            Basis::Metadata,
        ),
        None => (
            weights + weights / 4 + u128::from(OVERHEAD_BYTES),
            Basis::Size,
        ),
    };
    let mib = u32::try_from(bytes.div_ceil(u128::from(MIB))).unwrap_or(u32::MAX);
    Ok(Derived { mib, basis })
}

/// Keys and values for every layer at this context, or `None` when the
/// metadata does not say enough to work it out.
fn cache_bytes(metadata: &Metadata, context: u32, keys: u64, values: u64) -> Option<u128> {
    let layers = metadata.of_model("block_count")?;
    let heads = metadata.of_model("attention.head_count");
    let kv_heads = metadata.of_model("attention.head_count_kv").or(heads)?;
    let key_length = match metadata.of_model("attention.key_length") {
        Some(length) => length,
        None => metadata.of_model("embedding_length")?.checked_div(heads?)?,
    };
    let value_length = metadata
        .of_model("attention.value_length")
        .unwrap_or(key_length);
    let per_token = u128::from(kv_heads)
        * (u128::from(key_length) * u128::from(keys)
            + u128::from(value_length) * u128::from(values));
    Some(u128::from(layers) * u128::from(context) * per_token / 16)
}

/// The bytes per cached element the flags ask for, in sixteenths.
fn sixteenths(flags: &BTreeMap<String, String>, names: [&str; 2]) -> u64 {
    let spelling = names
        .iter()
        .find_map(|name| flags.get(*name))
        .map_or("f16", String::as_str);
    match spelling {
        "f32" => F32,
        "q8_0" => Q8_0,
        "q4_0" => Q4_0,
        _ => F16,
    }
}

/// The size of the model file, plus every other shard when it is split.
fn weights_of(model: &Path, metadata: Option<&Metadata>) -> Result<u64, String> {
    let mut total =
        size_of(model).ok_or_else(|| format!("no model file at '{}'", model.display()))?;
    let shards = metadata.and_then(Metadata::split_count).unwrap_or(1);
    if let Some(shard) = model
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(shard_of)
    {
        for index in 1..=shards {
            if index != shard.index {
                total += size_of(&shard.sibling(model, index)).unwrap_or(0);
            }
        }
    }
    Ok(total)
}

fn size_of(path: &Path) -> Option<u64> {
    fs::metadata(path)
        .ok()
        .filter(fs::Metadata::is_file)
        .map(|m| m.len())
}

/// One shard's place in a split model, read from a name such as
/// `model-00001-of-00004.gguf`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Shard<'a> {
    /// The name before the shard suffix, which names the model.
    pub stem: &'a str,
    pub index: u64,
    total: &'a str,
    extension: &'a str,
}

impl Shard<'_> {
    /// The path of another shard of the same model, beside this one.
    fn sibling(&self, model: &Path, index: u64) -> PathBuf {
        let width = self.total.len();
        model.with_file_name(format!(
            "{}-{index:0width$}-of-{}.{}",
            self.stem, self.total, self.extension
        ))
    }
}

/// Reads a shard suffix off a file name, if it carries one.
pub(super) fn shard_of(name: &str) -> Option<Shard<'_>> {
    let (rest, extension) = name.rsplit_once('.')?;
    let (head, total) = rest.rsplit_once("-of-")?;
    let (stem, index) = head.rsplit_once('-')?;
    if total.is_empty() || !total.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    if index.len() != total.len() || !index.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some(Shard {
        stem,
        index: index.parse().ok()?,
        total,
        extension,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_shard_suffix_is_read_and_its_siblings_named() {
        let shard = shard_of("Big-Model-00002-of-00004.gguf").expect("a shard");
        assert_eq!(shard.stem, "Big-Model");
        assert_eq!(shard.index, 2);
        assert_eq!(
            shard.sibling(Path::new("x/Big-Model-00002-of-00004.gguf"), 4),
            Path::new("x").join("Big-Model-00004-of-00004.gguf")
        );
        assert_eq!(shard_of("model.gguf"), None, "no suffix");
        assert_eq!(shard_of("model-1-of-x.gguf"), None, "not digits");
        assert_eq!(shard_of("model-1-of-04.gguf"), None, "widths differ");
    }

    #[test]
    fn a_cache_type_the_flags_do_not_name_is_read_as_f16() {
        let mut flags = BTreeMap::new();
        assert_eq!(sixteenths(&flags, ["ctk", "cache-type-k"]), F16);
        flags.insert("cache-type-k".to_owned(), "q4_0".to_owned());
        assert_eq!(
            sixteenths(&flags, ["ctk", "cache-type-k"]),
            Q4_0,
            "either spelling is honoured"
        );
        flags.insert("ctk".to_owned(), "something-new".to_owned());
        assert_eq!(
            sixteenths(&flags, ["ctk", "cache-type-k"]),
            F16,
            "a spelling this does not know is costed as f16, the safe side"
        );
    }
}
