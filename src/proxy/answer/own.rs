//! What the router answers from its own catalog, without touching a child.
//!
//! Three replies that share one rule: saying what *could* be served is never a
//! reason to start serving it. A client listing models, or asking whether this
//! server loads them on demand, must not cause a load -- so none of these
//! reaches a slot for anything but a read.
//!
//! Two shapes of the same question live here, because two clients ask it
//! differently. `/v1/models` is the `OpenAI` listing and carries only names.
//! `/models` is what a llama.cpp client reads in router mode, and carries each
//! entry's status, its source, and the context it was configured with -- the
//! fields that decide whether that client will offer the model at all.

use std::net::TcpStream;

use crate::catalog::Entry;

use super::Shared;
use super::reply;

/// Every entry the catalog carries, in the shape a client expects.
///
/// Answered from the catalog and nothing else: listing what can be served is
/// not a reason to start serving it, so no child is touched.
pub(super) fn list(stream: &mut TcpStream, shared: &Shared) -> std::io::Result<()> {
    listing(stream, &shared.catalog.entries, |entry| {
        serde_json::json!({
            "id": entry.id,
            "object": "model",
            "owned_by": "maestro-llamacpp",
        })
    })
}

/// Every entry, with whether it is loaded, in the shape a llama.cpp client
/// reads in router mode.
///
/// A client's whole test for "is this a router" is that each element carries
/// a string `id` and a string `status.value`, so both are always present and
/// neither is ever null. The status vocabulary is the server's own: an entry
/// is `loaded` when a child is holding it and `unloaded` otherwise. This
/// router has no third state -- it does not sleep a model or download one --
/// and saying so plainly is better than inventing a word a client would have
/// to guess at.
pub(super) fn catalogue(stream: &mut TcpStream, shared: &Shared) -> std::io::Result<()> {
    let loaded = shared.slots.loaded(&shared.catalog);

    listing(stream, &shared.catalog.entries, |entry| {
        let status = if loaded.contains(&entry.id) {
            "loaded"
        } else {
            "unloaded"
        };
        serde_json::json!({
            "id": entry.id,
            "object": "model",
            "owned_by": "maestro-llamacpp",
            // `failed` is stated rather than left out so a client reading it
            // finds a boolean. This router has no failed state to report: a
            // model that will not start is a refusal to the request that
            // asked for it, not a lasting mark on the entry.
            "status": { "value": status, "failed": false },
            // Every entry here is configured and waiting, which is what a
            // preset is. A client will not offer an *unloaded* model at all
            // unless it says so -- the three conditions are autoload, not
            // failed, and this -- so leaving it out hides exactly the models
            // the router exists to start on demand.
            "source": "preset",
            // The window the entry was configured with, so a client sizes
            // itself from the catalog rather than from its own default.
            "meta": { "n_ctx": entry.context_size },
        })
    })
}

/// One listing, as both shapes of it are: an envelope of `data`, and an
/// entry-shaped thing inside it per model.
///
/// The envelope is the whole of what the two share -- which is why it is here
/// and the per-entry shape is not. What each client reads out of an element is
/// exactly where they differ, and a single function taking every field would
/// hide that rather than express it.
fn listing(
    stream: &mut TcpStream,
    entries: &[Entry],
    shape: impl Fn(&Entry) -> serde_json::Value,
) -> std::io::Result<()> {
    let data: Vec<serde_json::Value> = entries.iter().map(shape).collect();
    let body = serde_json::json!({ "object": "list", "data": data }).to_string();

    reply(stream, 200, "OK", "application/json", &body)
}

/// What this server does, as the one field a client reads from it.
///
/// `models_autoload` is true and is not a setting: a request for a model that
/// is not running starts it, which is the whole point of the router. A client
/// that reads this decides not to ask for a load before a completion, and it
/// would be right.
pub(super) fn properties(stream: &mut TcpStream) -> std::io::Result<()> {
    let body = serde_json::json!({ "models_autoload": true }).to_string();
    reply(stream, 200, "OK", "application/json", &body)
}
