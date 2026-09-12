//! What the key-value cache costs at a given context.
//!
//! Split from the estimate beside it when the module-size gate said so, along
//! the seam the two already had: `estimate` adds up what an entry costs, and
//! this answers the one term of that sum which depends on the architecture
//! rather than on the files.

use std::collections::BTreeMap;

use crate::gguf::Metadata;

/// Bytes per cached element, in sixteenths, so a quantised cache is exact.
pub(super) const F16: u64 = 32;
const F32: u64 = 64;
const Q8_0: u64 = 17;
const Q4_0: u64 = 9;

/// Keys and values for every layer at this context, or `None` when the
/// metadata does not say enough to work it out.
pub(super) fn cache_bytes(
    metadata: &Metadata,
    context: u32,
    keys: u64,
    values: u64,
) -> Option<u128> {
    let layers = caching_layers(metadata)?;
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

/// How many of a model's layers keep a key-value cache.
///
/// Every one of them, unless the model says otherwise. A hybrid keeps a cache
/// on one layer in every `full_attention_interval` and gives the rest a
/// recurrent state whose size is fixed by the architecture rather than by the
/// context, so charging all of them a full cache overstates the larger of the
/// two terms by the interval.
///
/// Measured on this estate: Qwen3.8 27B declares sixty-five layers and an
/// interval of four. Counting all sixty-five put its cache at 17,680 MiB
/// against roughly 4,800 MiB the loaded server was found to hold, and its
/// whole estimate 14 GiB above what it measured.
///
/// The count is the floor of the division, which is what the server does: with
/// sixty-five layers and an interval of four, sixteen of them cache.
fn caching_layers(metadata: &Metadata) -> Option<u64> {
    let layers = metadata.of_model("block_count")?;
    match metadata.of_model("full_attention_interval") {
        Some(interval) if interval > 1 => Some(layers / interval),
        _ => Some(layers),
    }
}

/// The bytes per cached element the flags ask for, in sixteenths.
pub(super) fn sixteenths(flags: &BTreeMap<String, String>, names: [&str; 2]) -> u64 {
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

#[cfg(test)]
mod tests {
    use super::*;

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
