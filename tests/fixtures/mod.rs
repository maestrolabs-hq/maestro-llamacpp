//! Synthetic GGUF files, written in the format's own layout.
//!
//! The parser under test reads real model files, which are gigabytes each and
//! cannot ship with a repository. These are the same bytes at the head of such
//! a file -- magic, version, counts, typed key-value pairs -- with nothing
//! after them, so a test can say exactly what a file claims and check what the
//! parser read back. A file's size is set separately and sparsely, because an
//! estimate is derived from it and a test wants to name the figure.
//!
//! Shared by every test target that needs a model file with something inside
//! it. Each target compiles the whole module and uses a subset, for the same
//! reason `support` does: Rust has no partially used module.
#![allow(dead_code)]

use std::fs;
use std::path::Path;

/// One metadata value, in the subset of GGUF types these tests write.
#[derive(Debug, Clone)]
pub enum Value {
    U16(u16),
    U32(u32),
    U64(u64),
    F32(f32),
    Bool(bool),
    Text(String),
    /// A `UINT32` array, which is how a per-layer setting is stored.
    U32s(Vec<u32>),
    /// A `STRING` array, which is how a tokenizer's vocabulary is stored.
    Texts(Vec<String>),
}

/// A file being composed. Pairs are written in the order they were added.
#[derive(Debug, Clone)]
pub struct Gguf {
    version: u32,
    pairs: Vec<(String, Value)>,
}

impl Gguf {
    /// An empty file of the current format version.
    #[must_use]
    pub fn v3() -> Self {
        Self {
            version: 3,
            pairs: Vec::new(),
        }
    }

    /// The previous version, which differs only in its number.
    #[must_use]
    pub fn v2() -> Self {
        Self {
            version: 2,
            pairs: Vec::new(),
        }
    }

    /// The pairs a model of one architecture carries, so a test that wants
    /// "a plausible model" does not spell out six keys each time.
    #[must_use]
    pub fn model(architecture: &str, blocks: u32, context: u32, embedding: u32) -> Self {
        Self::v3()
            .with("general.architecture", Value::Text(architecture.to_owned()))
            .with(&format!("{architecture}.block_count"), Value::U32(blocks))
            .with(
                &format!("{architecture}.context_length"),
                Value::U32(context),
            )
            .with(
                &format!("{architecture}.embedding_length"),
                Value::U32(embedding),
            )
    }

    #[must_use]
    pub fn with(mut self, key: &str, value: Value) -> Self {
        self.pairs.push((key.to_owned(), value));
        self
    }

    /// The bytes, laid out as the format specifies.
    #[must_use]
    pub fn bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(b"GGUF");
        out.extend_from_slice(&self.version.to_le_bytes());
        out.extend_from_slice(&0u64.to_le_bytes());
        out.extend_from_slice(&(self.pairs.len() as u64).to_le_bytes());
        for (key, value) in &self.pairs {
            push_text(&mut out, key);
            push_value(&mut out, value);
        }
        out
    }

    /// Writes the file, then extends it sparsely to `size` bytes when that is
    /// larger than the header, so a test can name a file size in mebibytes
    /// without writing them.
    pub fn write(&self, path: &Path, size: u64) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("a writable temporary directory");
        }
        let bytes = self.bytes();
        fs::write(path, &bytes).expect("a writable model file");
        if size > bytes.len() as u64 {
            let file = fs::OpenOptions::new()
                .write(true)
                .open(path)
                .expect("the file just written");
            file.set_len(size).expect("a sparse extension");
        }
    }
}

/// A string as GGUF stores it: a length, then the bytes, no terminator.
fn push_text(out: &mut Vec<u8>, text: &str) {
    out.extend_from_slice(&(text.len() as u64).to_le_bytes());
    out.extend_from_slice(text.as_bytes());
}

/// The type tag, then the value, as the format numbers them.
fn push_value(out: &mut Vec<u8>, value: &Value) {
    match value {
        Value::U16(n) => {
            out.extend_from_slice(&2u32.to_le_bytes());
            out.extend_from_slice(&n.to_le_bytes());
        }
        Value::U32(n) => {
            out.extend_from_slice(&4u32.to_le_bytes());
            out.extend_from_slice(&n.to_le_bytes());
        }
        Value::F32(n) => {
            out.extend_from_slice(&6u32.to_le_bytes());
            out.extend_from_slice(&n.to_le_bytes());
        }
        Value::Bool(b) => {
            out.extend_from_slice(&7u32.to_le_bytes());
            out.push(u8::from(*b));
        }
        Value::Text(text) => {
            out.extend_from_slice(&8u32.to_le_bytes());
            push_text(out, text);
        }
        Value::U32s(items) => {
            out.extend_from_slice(&9u32.to_le_bytes());
            out.extend_from_slice(&4u32.to_le_bytes());
            out.extend_from_slice(&(items.len() as u64).to_le_bytes());
            for n in items {
                out.extend_from_slice(&n.to_le_bytes());
            }
        }
        Value::Texts(items) => {
            out.extend_from_slice(&9u32.to_le_bytes());
            out.extend_from_slice(&8u32.to_le_bytes());
            out.extend_from_slice(&(items.len() as u64).to_le_bytes());
            for text in items {
                push_text(out, text);
            }
        }
        Value::U64(n) => {
            out.extend_from_slice(&10u32.to_le_bytes());
            out.extend_from_slice(&n.to_le_bytes());
        }
    }
}

/// A directory under the system temporary directory, removed when dropped.
///
/// The estate's path rule allows the temporary directory because it names a
/// platform rather than a machine. Distinct from `support::ModelsRoot`, which
/// creates empty placeholder files: these tests need files with bytes in them.
pub struct Scratch {
    root: std::path::PathBuf,
}

impl Scratch {
    #[must_use]
    pub fn new(label: &str) -> Self {
        let unique = format!(
            "maestro-llamacpp-{label}-{}-{:?}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("a clock after 1970")
                .as_nanos(),
            std::thread::current().id()
        );
        let root = std::env::temp_dir().join(unique);
        fs::create_dir_all(&root).expect("a writable temporary directory");
        Self { root }
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.root
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        drop(fs::remove_dir_all(&self.root));
    }
}
