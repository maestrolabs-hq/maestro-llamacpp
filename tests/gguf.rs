//! What the router reads out of a model file.
//!
//! A GGUF file begins with its metadata, and the estimate that decides whether
//! a model fits is derived from six of those keys. These tests fix what the
//! reader must return, against files written in the format's own layout, so
//! that a file lying about its lengths cannot make the router allocate its way
//! into trouble.

use maestro_llamacpp::gguf::Metadata;

mod fixtures;
use fixtures::{Gguf, Scratch, Value};

#[test]
fn the_keys_an_estimate_needs_are_read_back() {
    let scratch = Scratch::new("gguf-keys");
    let path = scratch.path().join("model.gguf");
    Gguf::model("qwen3", 28, 40_960, 1024)
        .with("qwen3.attention.head_count", Value::U32(16))
        .with("qwen3.attention.head_count_kv", Value::U32(8))
        .with("qwen3.attention.key_length", Value::U32(128))
        .with("qwen3.attention.value_length", Value::U32(128))
        .with("general.name", Value::Text("A model".to_owned()))
        .write(&path, 0);

    let metadata = Metadata::read(&path).expect("a well-formed file is read");

    assert_eq!(metadata.architecture(), Some("qwen3"));
    assert_eq!(metadata.of_model("block_count"), Some(28));
    assert_eq!(metadata.of_model("context_length"), Some(40_960));
    assert_eq!(metadata.of_model("embedding_length"), Some(1024));
    assert_eq!(metadata.of_model("attention.head_count"), Some(16));
    assert_eq!(metadata.of_model("attention.head_count_kv"), Some(8));
    assert_eq!(metadata.of_model("attention.key_length"), Some(128));
    assert_eq!(metadata.of_model("attention.value_length"), Some(128));
    assert_eq!(
        metadata.of_model("attention.sliding_window"),
        None,
        "a key the file does not carry is absent rather than zero"
    );
    assert_eq!(metadata.split_count(), None, "not a shard");
}

#[test]
fn a_per_layer_setting_is_read_as_its_largest_value() {
    let scratch = Scratch::new("gguf-array");
    let path = scratch.path().join("model.gguf");
    Gguf::model("deci", 4, 4096, 256)
        .with(
            "deci.attention.head_count_kv",
            Value::U32s(vec![2, 8, 0, 4]),
        )
        .write(&path, 0);

    let metadata = Metadata::read(&path).expect("a well-formed file is read");

    assert_eq!(
        metadata.of_model("attention.head_count_kv"),
        Some(8),
        "the largest layer decides, because the cache is sized for the worst \
         layer and an average would undercount it"
    );
}

#[test]
fn every_value_type_is_stepped_over_to_reach_the_keys_after_it() {
    let scratch = Scratch::new("gguf-types");
    let path = scratch.path().join("model.gguf");
    Gguf::v3()
        .with(
            "tokenizer.ggml.tokens",
            Value::Texts(vec!["a".to_owned(); 300]),
        )
        .with("general.file_type", Value::U32(7))
        .with("tokenizer.ggml.bos_token_id", Value::U32(1))
        .with("some.float", Value::F32(1.5))
        .with("some.flag", Value::Bool(true))
        .with("some.wide", Value::U64(1 << 40))
        .with("split.count", Value::U16(4))
        .with("general.architecture", Value::Text("llama".to_owned()))
        .with("llama.block_count", Value::U32(32))
        .write(&path, 0);

    let metadata = Metadata::read(&path).expect("every type before the keys is skipped");

    assert_eq!(metadata.architecture(), Some("llama"));
    assert_eq!(metadata.of_model("block_count"), Some(32));
    assert_eq!(metadata.split_count(), Some(4));
}

#[test]
fn the_previous_format_version_is_read_the_same_way() {
    let scratch = Scratch::new("gguf-v2");
    let path = scratch.path().join("model.gguf");
    let mut file = Gguf::v2();
    file = file.with("general.architecture", Value::Text("gemma3".to_owned()));
    file = file.with("gemma3.block_count", Value::U32(26));
    file.write(&path, 0);

    let metadata = Metadata::read(&path).expect("version 2 lays its metadata out the same way");

    assert_eq!(metadata.of_model("block_count"), Some(26));
}

#[test]
fn a_file_that_is_not_gguf_is_refused_by_name() {
    let scratch = Scratch::new("gguf-magic");
    let path = scratch.path().join("weights.gguf");
    std::fs::write(
        &path,
        b"PK\x03\x04 this is a zip, whatever its extension says",
    )
    .expect("write");

    let fault = Metadata::read(&path)
        .expect_err("not a GGUF file")
        .to_string();

    assert!(
        fault.contains("GGUF"),
        "the refusal says what the file was supposed to begin with:\n{fault}"
    );
}

#[test]
fn a_file_cut_short_is_refused_rather_than_read_as_empty() {
    let scratch = Scratch::new("gguf-truncated");
    let path = scratch.path().join("model.gguf");
    let bytes = Gguf::model("llama", 32, 4096, 4096).bytes();
    std::fs::write(&path, &bytes[..bytes.len() - 3]).expect("write");

    assert!(
        Metadata::read(&path).is_err(),
        "a file that ends inside a value is not a file that says nothing"
    );
}

/// The one property that matters for a file the router did not write: a
/// length field claiming a terabyte must not become an allocation. The reader
/// steps over what it does not keep, so a lie about a length is a read past
/// the end of the file rather than a request for that much memory.
#[test]
fn a_length_the_file_cannot_back_is_a_fault_not_an_allocation() {
    let scratch = Scratch::new("gguf-liar");
    let path = scratch.path().join("model.gguf");
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"GGUF");
    bytes.extend_from_slice(&3u32.to_le_bytes());
    bytes.extend_from_slice(&0u64.to_le_bytes());
    bytes.extend_from_slice(&2u64.to_le_bytes());
    // A key of a plausible length, then a string value claiming a terabyte.
    bytes.extend_from_slice(&7u64.to_le_bytes());
    bytes.extend_from_slice(b"general");
    bytes.extend_from_slice(&8u32.to_le_bytes());
    bytes.extend_from_slice(&(1u64 << 40).to_le_bytes());
    bytes.extend_from_slice(b"short");
    std::fs::write(&path, &bytes).expect("write");

    let started = std::time::Instant::now();
    assert!(
        Metadata::read(&path).is_err(),
        "the file cannot back the length it claims"
    );
    assert!(
        started.elapsed() < std::time::Duration::from_secs(5),
        "refused promptly, without touching a terabyte"
    );
}
