//! The shape of a catalog: its version, its defaults, and its entries.
//!
//! This module knows what a catalog is made of. What any single field means,
//! and how to complain about it, belongs to [`super::field`]. Problems are
//! collected onto one list rather than returned at the first, so a reader
//! fixes a whole file in one pass.

use std::collections::{BTreeMap, BTreeSet};

use toml::{Table, Value};

use super::field::{
    as_location, as_positive, as_residency, as_runtime, as_text, flags, optional, problem,
    report_unknown, required, table_at,
};
use super::{Entry, Report, Residency};

/// Fields an entry may carry.
const ENTRY_FIELDS: &[&str] = &[
    "path",
    "draft_path",
    "projector_path",
    "context_size",
    "residency",
    "memory_estimate_mib",
    "reasoning_format",
    "runtime",
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
    "runtime",
    "reasoning_effort",
    "startup_timeout_seconds",
    "flags",
];

/// The budget an entry gets when neither it nor the defaults table names one.
///
/// Generous rather than tight, on purpose: a budget that expires on a healthy
/// model teaches people to raise it without reading it, and then it protects
/// nothing.
pub(super) const DEFAULT_STARTUP_TIMEOUT_SECONDS: u32 = 300;

/// How the defaults table is named in its own problems.
const DEFAULTS: &str = "catalog defaults";

/// Settings an entry inherits when it does not set them.
#[derive(Debug, Default)]
pub(super) struct Defaults {
    pub context_size: Option<u32>,
    pub residency: Option<Residency>,
    pub memory_estimate_mib: Option<u32>,
    pub reasoning_format: Option<String>,
    pub runtime: Option<String>,
    pub reasoning_effort: Option<String>,
    pub startup_timeout_seconds: Option<u32>,
    pub flags: BTreeMap<String, String>,
}

/// Everything one walk of the text produced.
///
/// The estimate is the one field the text may leave out, because it can be
/// derived from the files -- which needs a models root the text does not
/// have. An entry that leaves it out is named in `undeclared` and carries a
/// zero until a caller settles it, and no entry leaves this module's callers
/// with that zero still in it: `parse` refuses the entry, and `read` derives
/// the figure. Named by identifier even when the entry failed for other
/// reasons, so a reader fixing one entry sees every fault it has at once.
#[derive(Debug, Default)]
pub(super) struct Drafts {
    pub version: Option<u32>,
    pub defaults: Defaults,
    pub entries: Vec<Entry>,
    pub undeclared: BTreeSet<String>,
    pub problems: Vec<String>,
}

/// Walks the text once, collecting every problem rather than the first.
///
/// # Errors
///
/// Text that is not TOML stops the walk and is the one problem returned,
/// because nothing further can be read. Every other problem is on the
/// [`Drafts`] for the caller to judge alongside its own.
pub(super) fn drafts(text: &str) -> Result<Drafts, Report> {
    let table = text
        .parse::<Table>()
        .map_err(|e| Report::single(format!("the catalog is not valid TOML: {e}")))?;
    let mut drafts = Drafts::default();
    drafts.version = version(&table, &mut drafts.problems);
    drafts.defaults = defaults(&table, &mut drafts.problems);
    let defaults = std::mem::take(&mut drafts.defaults);
    drafts.entries = entries(&table, &defaults, &mut drafts);
    drafts.defaults = defaults;
    Ok(drafts)
}

fn version(table: &Table, problems: &mut Vec<String>) -> Option<u32> {
    required(table, "catalog", "version", problems, as_positive)
}

fn defaults(table: &Table, problems: &mut Vec<String>) -> Defaults {
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
        runtime: optional(inner, DEFAULTS, "runtime", problems, as_runtime),
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

fn entries(table: &Table, defaults: &Defaults, out: &mut Drafts) -> Vec<Entry> {
    if table.get("models").is_none() {
        out.problems
            .push(problem("catalog", "models", "is required"));
        return Vec::new();
    }
    let Some(models) = table_at(table, "catalog", "models", &mut out.problems) else {
        return Vec::new();
    };

    // `toml::Table` iterates in sorted order, so entries come out stable.
    models
        .iter()
        .filter_map(|(id, value)| entry(id, value, defaults, out))
        .collect()
}

fn entry(id: &str, value: &Value, defaults: &Defaults, out: &mut Drafts) -> Option<Entry> {
    let problems = &mut out.problems;
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
    // Not settled here: absence is a problem for `parse` and a derivation
    // for `read`, and only the caller knows which it is.
    let memory_estimate_mib = optional(table, &scope, "memory_estimate_mib", problems, as_positive)
        .or(defaults.memory_estimate_mib);
    let residency = optional(table, &scope, "residency", problems, as_residency)
        .or(defaults.residency)
        .unwrap_or(Residency::OnDemand);
    let reasoning_format = optional(table, &scope, "reasoning_format", problems, as_text)
        .or_else(|| defaults.reasoning_format.clone());
    let runtime = optional(table, &scope, "runtime", problems, as_runtime)
        .or_else(|| defaults.runtime.clone());
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

    // Recorded before the entry can fail on another field, so an entry that
    // is wrong three ways is reported all three at once.
    if memory_estimate_mib.is_none() {
        out.undeclared.insert(id.to_owned());
    }

    Some(Entry {
        id: id.to_owned(),
        path: path?,
        draft_path,
        projector_path,
        context_size: context_size?,
        residency,
        memory_estimate_mib: memory_estimate_mib.unwrap_or_default(),
        reasoning_format,
        runtime,
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
