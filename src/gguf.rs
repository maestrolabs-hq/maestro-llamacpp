//! What a model file says about itself.
//!
//! A GGUF file begins with its metadata: a magic, a version, two counts, then
//! typed key-value pairs, and only after all of that the tensors. Everything
//! the router wants to know about a model before loading it -- how many
//! layers it has, how wide its attention is, how much context it was trained
//! for, whether it is one shard of several -- sits in those pairs, so this
//! reads them and stops.
//!
//! Three things are part of the contract rather than the implementation.
//!
//! Nothing here allocates on the file's say-so. A length is stepped over, not
//! read into memory, unless the value is one of the handful this keeps; so a
//! file claiming a terabyte-long string costs a seek past the end of the file
//! and a fault, never that much memory. The router reads files it did not
//! write, and a corrupt download must not take it down.
//!
//! A per-layer setting reads as its largest value. Some architectures vary
//! the key-value head count by layer and store an array; the cache is sized
//! for the worst layer, so the largest is the honest figure and an average
//! would undercount.
//!
//! A key the file does not carry is absent, never zero. Zero layers or zero
//! heads would make an estimate of nothing, which is the one figure a caller
//! must never be handed by mistake.
//!
//! Versions 2 and 3 are read; they lay their metadata out identically. Version
//! 1 used narrower lengths and predates every file the router will meet.

use std::collections::BTreeMap;
use std::fmt;
use std::fs::File;
use std::io::{BufReader, Read, Seek};
use std::path::Path;

mod bytes;

use bytes::{Reader, scalar_at, skip, text_at, u32_at, u64_at, width};

/// Why a file could not be read as GGUF metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fault(String);

impl fmt::Display for Fault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for Fault {}

/// The most pairs a file may declare. Real files carry a few dozen; a count
/// past this is a corrupt header, not a large model.
const MAX_PAIRS: u64 = 1 << 16;

/// The longest key kept. Keys are short dotted identifiers.
const MAX_KEY_BYTES: u64 = 1024;

/// The longest string value kept, which is only ever the architecture name.
const MAX_KEPT_TEXT_BYTES: u64 = 256;

/// The most elements a per-layer array may carry before it is refused as
/// corrupt. Models have hundreds of layers, not millions.
const MAX_LAYER_ARRAY: u64 = 1 << 20;

/// The type tags, as the format numbers them.
const STRING: u32 = 8;
const ARRAY: u32 = 9;

/// The keys read as metadata, and what they said.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Metadata {
    architecture: Option<String>,
    numbers: BTreeMap<String, u64>,
    layers: BTreeMap<String, Vec<u64>>,
}

impl Metadata {
    /// Reads the metadata at the head of a model file.
    ///
    /// # Errors
    ///
    /// Returns a [`Fault`] when the file cannot be opened, does not begin
    /// with the GGUF magic, is a version this does not read, or ends before
    /// its own metadata does.
    pub fn read(path: &Path) -> Result<Self, Fault> {
        let file = File::open(path)
            .map_err(|error| Fault(format!("cannot open '{}': {error}", path.display())))?;
        let length = file
            .metadata()
            .map_err(|error| Fault(format!("cannot size '{}': {error}", path.display())))?
            .len();
        let mut reader = BufReader::new(file);
        let mut metadata = Self {
            architecture: None,
            numbers: BTreeMap::new(),
            layers: BTreeMap::new(),
        };
        metadata
            .fill(&mut reader, length)
            .map_err(|Fault(reason)| Fault(format!("'{}': {reason}", path.display())))?;
        Ok(metadata)
    }

    /// The architecture the file names, such as `llama` or `qwen3`.
    #[must_use]
    pub fn architecture(&self) -> Option<&str> {
        self.architecture.as_deref()
    }

    /// An integer value by its full key, or the largest element when the key
    /// held a per-layer array.
    #[must_use]
    pub fn number(&self, key: &str) -> Option<u64> {
        self.numbers.get(key).copied()
    }

    /// An integer value under the architecture's own prefix, so a caller asks
    /// for `block_count` rather than spelling `qwen3.block_count`.
    #[must_use]
    pub fn of_model(&self, suffix: &str) -> Option<u64> {
        self.number(&format!("{}.{suffix}", self.architecture()?))
    }

    /// A per-layer array under the architecture's own prefix, in file order.
    ///
    /// Kept beside the largest element rather than instead of it: a caller
    /// that only wants the widest layer still asks [`Self::of_model`], and one
    /// sizing a cache layer by layer needs which layer is which. A model whose
    /// layers differ -- some attending to a window, some to the whole context,
    /// with their own head counts -- cannot be sized from a maximum.
    #[must_use]
    pub fn per_layer(&self, suffix: &str) -> Option<&[u64]> {
        self.layers
            .get(&format!("{}.{suffix}", self.architecture()?))
            .map(Vec::as_slice)
    }

    /// How many shards the model is split across, when this file is one.
    #[must_use]
    pub fn split_count(&self) -> Option<u64> {
        self.number("split.count")
    }

    fn fill(&mut self, reader: &mut Reader, length: u64) -> Result<(), Fault> {
        let mut magic = [0u8; 4];
        reader.read_exact(&mut magic)?;
        if &magic != b"GGUF" {
            return Err(Fault("does not begin with the GGUF magic".to_owned()));
        }
        let version = u32_at(reader)?;
        if !(2..=3).contains(&version) {
            return Err(Fault(format!(
                "GGUF version {version} is not one this reads"
            )));
        }
        let _tensors = u64_at(reader)?;
        let pairs = u64_at(reader)?;
        if pairs > MAX_PAIRS {
            return Err(Fault(format!("claims {pairs} metadata pairs")));
        }
        for _ in 0..pairs {
            let key = text_at(reader, MAX_KEY_BYTES)?
                .ok_or_else(|| Fault("a key longer than any the format uses".to_owned()))?;
            let kind = u32_at(reader)?;
            self.value(reader, &key, kind)?;
        }
        // Stepping over a value seeks rather than reads, so a lie about a
        // length surfaces here rather than being taken at its word.
        let position = reader.stream_position()?;
        if position > length {
            return Err(Fault("ends before its metadata does".to_owned()));
        }
        Ok(())
    }

    fn value(&mut self, reader: &mut Reader, key: &str, kind: u32) -> Result<(), Fault> {
        match kind {
            STRING => {
                let kept = text_at(reader, MAX_KEPT_TEXT_BYTES)?;
                if key == "general.architecture" {
                    self.architecture = kept;
                }
            }
            ARRAY => self.array(reader, key)?,
            _ => {
                let number = scalar_at(reader, kind)?;
                if let Some(number) = number.filter(|_| !key.starts_with("tokenizer.")) {
                    self.numbers.insert(key.to_owned(), number);
                }
            }
        }
        Ok(())
    }

    /// A per-layer array is reduced to its largest element; every other array
    /// is stepped over.
    fn array(&mut self, reader: &mut Reader, key: &str) -> Result<(), Fault> {
        let kind = u32_at(reader)?;
        let count = u64_at(reader)?;
        let per_layer = key.ends_with(".attention.head_count_kv")
            || key.ends_with(".attention.head_count")
            || key.ends_with(".attention.sliding_window_pattern");
        if per_layer && kind != STRING && kind != ARRAY {
            if count > MAX_LAYER_ARRAY {
                return Err(Fault(format!("'{key}' claims {count} layers")));
            }
            let mut largest = None;
            let mut elements = Vec::new();
            for _ in 0..count {
                let element = scalar_at(reader, kind)?;
                largest = largest.max(element);
                if let Some(element) = element {
                    elements.push(element);
                }
            }
            if let Some(largest) = largest {
                self.numbers.insert(key.to_owned(), largest);
            }
            if !elements.is_empty() {
                self.layers.insert(key.to_owned(), elements);
            }
            return Ok(());
        }
        match kind {
            STRING => {
                for _ in 0..count {
                    text_at(reader, 0)?;
                }
            }
            ARRAY => Err(Fault(format!("'{key}' nests arrays")))?,
            _ => {
                let bytes = count
                    .checked_mul(width(kind)?)
                    .ok_or_else(|| Fault(format!("'{key}' claims an impossible length")))?;
                skip(reader, bytes)?;
            }
        }
        Ok(())
    }
}
