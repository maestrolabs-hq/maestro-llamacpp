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
    "startup_timeout_seconds",
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
    "startup_timeout_seconds",
    "flags",
];

/// The budget an entry gets when neither it nor the defaults table names one.
///
/// Generous rather than tight, on purpose: a budget that expires on a healthy
/// model teaches people to raise it without reading it, and then it protects
/// nothing.
const DEFAULT_STARTUP_TIMEOUT_SECONDS: u32 = 300;

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
    startup_timeout_seconds: Option<u32>,
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
        startup_timeout_seconds: optional(
            inner,
            DEFAULTS,
            "startup_timeout_seconds",
            problems,
            as_positive,
        ),
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

    // Every field is read before any one of them is allowed to fail the
    // entry. Short-circuiting on the first would hide the rest until the
    // reader had fixed it and run again, one mistake per run.
    let path = required(table, &scope, "path", problems, as_location);
    let draft_path = optional(table, &scope, "draft_path", problems, as_location);
    let projector_path = optional(table, &scope, "projector_path", problems, as_location);
    let context_size = settled(
        table,
        optional(table, &scope, "context_size", problems, as_positive).or(defaults.context_size),
        &scope,
        "context_size",
        problems,
    );
    let memory_estimate_mib = settled(
        table,
        optional(table, &scope, "memory_estimate_mib", problems, as_positive)
            .or(defaults.memory_estimate_mib),
        &scope,
        "memory_estimate_mib",
        problems,
    );
    let residency = optional(table, &scope, "residency", problems, as_residency)
        .or(defaults.residency)
        .unwrap_or(Residency::OnDemand);
    let reasoning_format = optional(table, &scope, "reasoning_format", problems, as_text)
        .or_else(|| defaults.reasoning_format.clone());
    let reasoning_effort = optional(table, &scope, "reasoning_effort", problems, as_text)
        .or_else(|| defaults.reasoning_effort.clone());
    let startup_timeout_seconds = optional(
        table,
        &scope,
        "startup_timeout_seconds",
        problems,
        as_positive,
    )
    .or(defaults.startup_timeout_seconds)
    .unwrap_or(DEFAULT_STARTUP_TIMEOUT_SECONDS);

    Some(Entry {
        id: id.to_owned(),
        path: path?,
        draft_path,
        projector_path,
        context_size: context_size?,
        residency,
        memory_estimate_mib: memory_estimate_mib?,
        reasoning_format,
        reasoning_effort,
        startup_timeout_seconds,
        flags: merged,
    })
}

/// A value the entry has one way or another, or a problem saying it has none.
///
/// Absence and invalidity are different failures and only one of them belongs
/// here. A value that was present but wrong has already been described by its
/// converter, and calling it missing as well would send the reader looking for
/// a field that is sitting in front of them.
fn settled(
    table: &Table,
    value: Option<u32>,
    scope: &str,
    field: &str,
    problems: &mut Vec<String>,
) -> Option<u32> {
    if value.is_none() && table.get(field).is_none() {
        problems.push(problem(
            scope,
            field,
            "is required, and no default supplies it",
        ));
    }
    value
}
