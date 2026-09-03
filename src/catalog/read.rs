//! The shape of a catalog: its version, its defaults, and its entries.
//!
//! This module knows what a catalog is made of. What any single field means,
//! and how to complain about it, belongs to [`super::field`]. Problems are
//! collected onto one list rather than returned at the first, so a reader
//! fixes a whole file in one pass.

use std::collections::BTreeMap;

use toml::{Table, Value};

use super::field::{
    as_location, as_positive, as_residency, as_text, flags, optional, problem, report_unknown,
    required, table_at,
};
use super::{Entry, Residency};

/// Fields an entry may carry.
const ENTRY_FIELDS: &[&str] = &[
    "path",
    "draft_path",
    "projector_path",
    "context_size",
    "residency",
    "memory_estimate_mib",
    "reasoning_format",
    "reasoning_effort",
    "flags",
];

/// Fields the defaults table may carry. A location is deliberately absent:
/// two models never share one file, so a default path could only be wrong.
const DEFAULT_FIELDS: &[&str] = &[
    "context_size",
    "residency",
    "memory_estimate_mib",
    "reasoning_format",
    "reasoning_effort",
    "flags",
];

/// How the defaults table is named in its own problems.
const DEFAULTS: &str = "catalog defaults";

/// Settings an entry inherits when it does not set them.
#[derive(Debug, Default)]
pub(super) struct Defaults {
    context_size: Option<u32>,
    residency: Option<Residency>,
    memory_estimate_mib: Option<u32>,
    reasoning_format: Option<String>,
    reasoning_effort: Option<String>,
    flags: BTreeMap<String, String>,
}

pub(super) fn version(table: &Table, problems: &mut Vec<String>) -> Option<u32> {
    required(table, "catalog", "version", problems, as_positive)
}

pub(super) fn defaults(table: &Table, problems: &mut Vec<String>) -> Defaults {
    let Some(inner) = table_at(table, "catalog", "defaults", problems) else {
        return Defaults::default();
    };
    report_unknown(inner, DEFAULT_FIELDS, DEFAULTS, problems);

    Defaults {
        context_size: optional(inner, DEFAULTS, "context_size", problems, as_positive),
        residency: optional(inner, DEFAULTS, "residency", problems, as_residency),
        memory_estimate_mib: optional(
            inner,
            DEFAULTS,
            "memory_estimate_mib",
            problems,
            as_positive,
        ),
        reasoning_format: optional(inner, DEFAULTS, "reasoning_format", problems, as_text),
        reasoning_effort: optional(inner, DEFAULTS, "reasoning_effort", problems, as_text),
        flags: flags(inner, DEFAULTS, problems),
    }
}

pub(super) fn entries(
    table: &Table,
    defaults: &Defaults,
    problems: &mut Vec<String>,
) -> Vec<Entry> {
    if table.get("models").is_none() {
        problems.push(problem("catalog", "models", "is required"));
        return Vec::new();
    }
    let Some(models) = table_at(table, "catalog", "models", problems) else {
        return Vec::new();
    };

    // `toml::Table` iterates in sorted order, so entries come out stable.
    models
        .iter()
        .filter_map(|(id, value)| entry(id, value, defaults, problems))
        .collect()
}

fn entry(
    id: &str,
    value: &Value,
    defaults: &Defaults,
    problems: &mut Vec<String>,
) -> Option<Entry> {
    let scope = format!("entry '{id}'");
    let Some(table) = value.as_table() else {
        problems.push(format!("{scope}: must be a table"));
        return None;
    };
    report_unknown(table, ENTRY_FIELDS, &scope, problems);

    // The entry's own flags win; the rest are inherited.
    let mut merged = defaults.flags.clone();
    merged.extend(flags(table, &scope, problems));

    Some(Entry {
        id: id.to_owned(),
        path: required(table, &scope, "path", problems, as_location)?,
        draft_path: optional(table, &scope, "draft_path", problems, as_location),
        projector_path: optional(table, &scope, "projector_path", problems, as_location),
        context_size: inherited(
            optional(table, &scope, "context_size", problems, as_positive),
            defaults.context_size,
            &scope,
            "context_size",
            problems,
        )?,
        residency: optional(table, &scope, "residency", problems, as_residency)
            .or(defaults.residency)
            .unwrap_or(Residency::OnDemand),
        memory_estimate_mib: inherited(
            optional(table, &scope, "memory_estimate_mib", problems, as_positive),
            defaults.memory_estimate_mib,
            &scope,
            "memory_estimate_mib",
            problems,
        )?,
        reasoning_format: optional(table, &scope, "reasoning_format", problems, as_text)
            .or_else(|| defaults.reasoning_format.clone()),
        reasoning_effort: optional(table, &scope, "reasoning_effort", problems, as_text)
            .or_else(|| defaults.reasoning_effort.clone()),
        flags: merged,
    })
}

/// What the entry set, else what it inherits, else a problem naming both the
/// field and the fact that no default covered it.
fn inherited(
    own: Option<u32>,
    default: Option<u32>,
    scope: &str,
    field: &str,
    problems: &mut Vec<String>,
) -> Option<u32> {
    let value = own.or(default);
    if value.is_none() {
        problems.push(problem(
            scope,
            field,
            "is required, and no default supplies it",
        ));
    }
    value
}
