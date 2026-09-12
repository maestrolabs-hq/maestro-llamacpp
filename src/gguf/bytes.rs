//! The format's primitives: fixed-width integers, strings, and stepping over
//! what is not kept.
//!
//! Split from the module above it along the seam the size gate exposed: that
//! module decides which values are worth keeping, and this reads one value's
//! bytes without caring what it is for. Every function here takes the reader
//! and hands back one value or steps past it; none of them knows a key name.

use std::fs::File;
use std::io::{BufReader, Read};

use super::Fault;

pub(super) type Reader = BufReader<File>;

/// A read that ended inside a value, which is what every I/O failure past
/// the open amounts to.
impl From<std::io::Error> for Fault {
    fn from(error: std::io::Error) -> Self {
        Self(format!("ends inside its metadata: {error}"))
    }
}

/// Exactly `N` bytes, or the fault for a file that ends inside them.
fn bytes_at<const N: usize>(reader: &mut Reader) -> Result<[u8; N], Fault> {
    let mut bytes = [0u8; N];
    reader.read_exact(&mut bytes)?;
    Ok(bytes)
}

pub(super) fn u32_at(reader: &mut Reader) -> Result<u32, Fault> {
    Ok(u32::from_le_bytes(bytes_at(reader)?))
}

pub(super) fn u64_at(reader: &mut Reader) -> Result<u64, Fault> {
    Ok(u64::from_le_bytes(bytes_at(reader)?))
}

/// The bytes one scalar of this kind occupies, or a fault for a kind the
/// format does not define.
pub(super) fn width(kind: u32) -> Result<u64, Fault> {
    match kind {
        0 | 1 | 7 => Ok(1),
        2 | 3 => Ok(2),
        4..=6 => Ok(4),
        10..=12 => Ok(8),
        _ => Err(Fault(format!(
            "uses value type {kind}, which the format does not define"
        ))),
    }
}

/// One scalar, as an unsigned integer when it is a non-negative integer and
/// `None` when it is a float, a flag, or negative.
pub(super) fn scalar_at(reader: &mut Reader, kind: u32) -> Result<Option<u64>, Fault> {
    let mut bytes = [0u8; 8];
    // Never more than eight, so the conversion cannot fail; written as one so
    // no cast has to be vouched for.
    let taken = usize::try_from(width(kind)?).unwrap_or(8);
    reader.read_exact(&mut bytes[..taken])?;
    Ok(unsigned(kind, bytes))
}

/// The integer a scalar's bytes spell, when they spell a non-negative one.
///
/// A negative integer converts to nothing, which is right: no count the
/// estimate reads can be below zero, and a file saying so is not believed.
fn unsigned(kind: u32, bytes: [u8; 8]) -> Option<u64> {
    match kind {
        0 | 2 | 4 | 10 => Some(u64::from_le_bytes(bytes)),
        // A flag, as one or nothing. Read because a per-layer array of them
        // is how a model says which of its layers attend to a window rather
        // than to the whole context, and that decides most of the cache.
        7 => Some(u64::from(bytes[0] != 0)),
        1 => u64::try_from(i8::from_le_bytes([bytes[0]])).ok(),
        3 => u64::try_from(i16::from_le_bytes([bytes[0], bytes[1]])).ok(),
        5 => u64::try_from(i32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])).ok(),
        11 => u64::try_from(i64::from_le_bytes(bytes)).ok(),
        _ => None,
    }
}

/// A string, kept when it is no longer than `keep` bytes and stepped over
/// otherwise. A limit of zero keeps nothing.
pub(super) fn text_at(reader: &mut Reader, keep: u64) -> Result<Option<String>, Fault> {
    let length = u64_at(reader)?;
    if length > keep {
        skip(reader, length)?;
        return Ok(None);
    }
    let mut bytes = vec![0u8; usize::try_from(length).unwrap_or_default()];
    reader.read_exact(&mut bytes)?;
    Ok(Some(String::from_utf8_lossy(&bytes).into_owned()))
}

/// Steps over bytes without reading them, so a claimed length costs nothing
/// until something after it is read.
pub(super) fn skip(reader: &mut Reader, bytes: u64) -> Result<(), Fault> {
    let offset =
        i64::try_from(bytes).map_err(|_| Fault("claims an impossible length".to_owned()))?;
    reader.seek_relative(offset)?;
    Ok(())
}
