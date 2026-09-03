//! Reading one field, and naming it when it is wrong.
//!
//! Two readers and a handful of converters, split that way on purpose. The
//! readers know where a value lives and who to blame for it; the converters
//! know what a value must look like and say so in one phrase. Keeping them
//! apart is what stops every field from carrying its own copy of "fetch it,
//! check it, complain about it".
//!
//! Problems are pushed onto a shared list rather than returned, which is why
//! the readers yield `Option` and not `Result`: `None` means "already
//! reported", never "stop reading".

use std::collections::BTreeMap;

use toml::{Table, Value};

use super::{RelativePath, Residency};

/// One problem, phrased the same way every time.
pub(super) fn problem(scope: &str, field: &str, complaint: &str) -> String {
    format!("{scope}: field '{field}' {complaint}")
}

/// Names any field the schema does not define. A misspelled setting that
/// parsed silently would be a setting that never applied.
pub(super) fn report_unknown(
    table: &Table,
    known: &[&str],
    scope: &str,
    problems: &mut Vec<String>,
) {
    for key in table.keys() {
        if !known.contains(&key.as_str()) {
            problems.push(problem(scope, key, "is not recognised"));
        }
    }
}

/// A nested table. Absent and malformed are different: only the second is a
/// problem, and the caller decides what absence means.
pub(super) fn table_at<'a>(
    table: &'a Table,
    scope: &str,
    field: &str,
    problems: &mut Vec<String>,
) -> Option<&'a Table> {
    let value = table.get(field)?;
    let inner = value.as_table();
    if inner.is_none() {
        problems.push(problem(scope, field, "must be a table"));
    }
    inner
}

/// A field the catalog may omit, converted by `parse`.
pub(super) fn optional<T>(
    table: &Table,
    scope: &str,
    field: &str,
    problems: &mut Vec<String>,
    parse: impl FnOnce(&Value) -> Result<T, String>,
) -> Option<T> {
    match parse(table.get(field)?) {
        Ok(value) => Some(value),
        Err(complaint) => {
            problems.push(problem(scope, field, &complaint));
            None
        }
    }
}

/// A field the catalog must carry, converted by `parse`.
pub(super) fn required<T>(
    table: &Table,
    scope: &str,
    field: &str,
    problems: &mut Vec<String>,
    parse: impl FnOnce(&Value) -> Result<T, String>,
) -> Option<T> {
    if table.get(field).is_none() {
        problems.push(problem(scope, field, "is required"));
        return None;
    }
    optional(table, scope, field, problems, parse)
}

pub(super) fn as_text(value: &Value) -> Result<String, String> {
    value
        .as_str()
        .map(ToOwned::to_owned)
        .ok_or_else(|| "must be a string".to_owned())
}

/// Zero is refused rather than defaulted: a context of no tokens, or a model
/// that costs no memory, are both claims the router would have to disbelieve
/// later.
pub(super) fn as_positive(value: &Value) -> Result<u32, String> {
    let Some(number) = value.as_integer() else {
        return Err("must be a whole number".to_owned());
    };
    match u32::try_from(number) {
        Ok(n) if n > 0 => Ok(n),
        _ => Err(format!("must be greater than zero, but is {number}")),
    }
}

pub(super) fn as_residency(value: &Value) -> Result<Residency, String> {
    let text = as_text(value)?;
    Residency::parse(&text).ok_or_else(|| {
        format!(
            "must be one of {}, but is '{text}'",
            Residency::NAMES.join(" or ")
        )
    })
}

/// The refusal reason comes from [`RelativePath`], which owns what makes a
/// location unacceptable.
pub(super) fn as_location(value: &Value) -> Result<RelativePath, String> {
    RelativePath::new(&as_text(value)?)
}

/// Server settings passed through untouched. Their meaning belongs to the
/// server, and this router does not pretend to know it: the shape is checked,
/// the content is not. That is what keeps a change to the server's flag
/// surface a catalog edit rather than a code change.
pub(super) fn flags(
    table: &Table,
    scope: &str,
    problems: &mut Vec<String>,
) -> BTreeMap<String, String> {
    let Some(inner) = table_at(table, scope, "flags", problems) else {
        return BTreeMap::new();
    };

    let mut out = BTreeMap::new();
    for (key, value) in inner {
        match as_text(value) {
            Ok(text) => {
                out.insert(key.clone(), text);
            }
            Err(_) => problems.push(problem(
                scope,
                "flags",
                &format!("carries '{key}', which must be a string"),
            )),
        }
    }
    out
}
