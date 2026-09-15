//! What a llama.cpp client reads, which the `OpenAI` listing does not carry.
//!
//! Two replies that share one rule with the listing beside them in `reply`:
//! saying what *could* be served is never a reason to start serving it. A
//! client asking what models exist, or whether this server loads them on
//! demand, must not cause a load -- so neither of these reaches a slot for
//! anything but a read.
//!
//! Separate from `reply::listing` because the two answer different clients.
//! `/v1/models` is the `OpenAI` shape and carries only names. `/models` is
//! what a llama.cpp client reads in router mode, and carries each entry's
//! status, its source and the context it was configured with -- the fields
//! that decide whether that client will offer the model at all. The framing
//! they share is `reply::json`; what remains here is only the part that
//! differs, which is the whole reason there are two.

use std::net::TcpStream;

use super::Shared;
use super::reply;

/// Every entry that can hold a conversation, with whether it is loaded, in
/// the shape a llama.cpp client reads in router mode.
///
/// A client's whole test for "is this a router" is that each element carries
/// a string `id` and a string `status.value`, so both are always present and
/// neither is ever null. The status vocabulary is the server's own: an entry
/// is `loaded` when a child is holding it and `unloaded` otherwise. This
/// router has no third state -- it does not sleep a model or download one --
/// and saying so plainly is better than inventing a word a client would have
/// to guess at.
///
/// Entries that generate nothing are left out. A client reading this surface
/// is choosing something to talk to, and its filter is `status`, `source` and
/// `failed` -- none of which can say "this is not a chat model" without lying
/// about one of them. An embedding server offered here is a selection that can
/// only fail. `/v1/models` still carries them, because a caller wanting an
/// embedding has to find it somewhere, and that surface is a catalogue rather
/// than a menu.
pub(super) fn catalogue(
    stream: &mut TcpStream,
    shared: &Shared,
    head_only: bool,
) -> std::io::Result<()> {
    let loaded = shared.slots.loaded(&shared.catalog);
    let held: std::collections::HashMap<String, Option<u64>> =
        shared.slots.memory(&shared.catalog).into_iter().collect();

    let data: Vec<serde_json::Value> = shared
        .catalog
        .entries
        .iter()
        .filter(|entry| entry.generates())
        .map(|entry| {
            let status = if loaded.contains(&entry.id) {
                "loaded"
            } else {
                "unloaded"
            };
            serde_json::json!({
                "id": entry.id,
                "object": "model",
                "owned_by": "maestro-llamacpp",
                // `failed` is stated rather than left out so a client reading
                // it finds a boolean. This router has no failed state to
                // report: a model that will not start is a refusal to the
                // request that asked for it, not a lasting mark on the entry.
                "status": { "value": status, "failed": false },
                // Every entry here is configured and waiting, which is what a
                // preset is. A client will not offer an *unloaded* model at
                // all unless it says so -- the three conditions are autoload,
                // not failed, and this -- so leaving it out hides exactly the
                // models the router exists to start on demand.
                "source": "preset",
                // The window the entry was configured with, so a client sizes
                // itself from the catalog rather than from its own default.
                "meta": { "n_ctx": entry.context_size },
                // What the entry can be sent. A client reads this and nothing
                // else before deciding whether an image may go in the request,
                // so an entry given a projector and not saying so is offered
                // as though it were text-only. The catalog is what knows --
                // it names the projector -- and reporting it here answers for
                // every client rather than for one that was configured by hand.
                "architecture": { "input_modalities": entry.accepts() },
                // What the entry was estimated to hold, and what it was
                // measured holding once it was loaded. Admission compares the
                // first against the budget; the second is what the card said
                // the child actually took. They are reported together because
                // an estimate only drifts visibly when both are in one place,
                // and until now the only place was a line on a stdout the
                // service sends to /dev/null. `held_mib` is null for an entry
                // that is not loaded, and for one whose card could not be
                // read -- neither of which is a measurement of nothing.
                "memory": {
                    "declared_mib": entry.memory_estimate_mib,
                    "held_mib": held.get(&entry.id).copied().flatten(),
                },
            })
        })
        .collect();

    reply::json(
        stream,
        &serde_json::json!({ "object": "list", "data": data }),
        head_only,
    )
}

/// What this server does, as the one field a client reads from it.
///
/// `models_autoload` is true and is not a setting: a request for a model that
/// is not running starts it, which is the whole point of the router. A client
/// that reads this decides not to ask for a load before a completion, and it
/// would be right.
pub(super) fn properties(stream: &mut TcpStream, head_only: bool) -> std::io::Result<()> {
    reply::json(
        stream,
        &serde_json::json!({ "models_autoload": true }),
        head_only,
    )
}
