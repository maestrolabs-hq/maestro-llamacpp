//! The llama.cpp router-mode surface, which is what a llama.cpp client speaks.
//!
//! Distinct from `/v1/models`, which is the `OpenAI` shape and answers a
//! different question. A client in router mode asks `/models` for a catalogue
//! carrying each entry's *status*, and `/props` for what the server itself
//! does, and decides from those whether it is talking to a router at all. Its
//! check is narrow and worth stating exactly: every element must carry a
//! string `id` and a string `status.value`, or it reports that the server is
//! not running in router mode.

use serde_json::Value;

mod support;
use support::{MODEL, ModelsRoot, get, request, serving, status};

/// Two entries, so a catalogue has something to distinguish.
const CATALOG: &str = concat!(
    "version = 1\n",
    "\n[defaults]\n",
    "context_size = 4096\n",
    "residency = \"on-demand\"\n",
    "memory_estimate_mib = 512\n",
    "startup_timeout_seconds = 30\n",
    "\n[models.gemma3]\n",
    "path = \"cache/gemma/gemma-3-1b.gguf\"\n",
);

/// The JSON body of a reply, or a panic naming what arrived instead.
fn body(reply: &str) -> Value {
    let body = reply.split_once("\r\n\r\n").map_or_else(
        || panic!("a reply with a body, got:\n{reply}"),
        |(_, body)| body,
    );
    serde_json::from_str(body).unwrap_or_else(|error| {
        panic!("a JSON body ({error}), got:\n{body}");
    })
}

#[test]
fn the_catalogue_carries_an_id_and_a_status_for_every_entry() {
    let serving = serving(CATALOG, ModelsRoot::with(&[MODEL]));
    let reply = request(serving.address(), &get("/models"));

    assert_eq!(status(&reply), Some(200), "got:\n{reply}");

    let payload = body(&reply);
    let data = payload["data"]
        .as_array()
        .unwrap_or_else(|| panic!("a 'data' array, got:\n{payload}"));
    assert_eq!(data.len(), 1, "one entry in the catalog, one in the reply");

    // Exactly the client's own test. Anything less and it reports that the
    // server is not running in router mode, which is a confusing way to say
    // that a field is missing.
    for entry in data {
        assert!(
            entry["id"].is_string(),
            "every entry carries a string id: {entry}"
        );
        assert!(
            entry["status"]["value"].is_string(),
            "every entry carries a string status.value: {entry}"
        );
    }
    assert_eq!(data[0]["id"], "gemma3");
    assert_eq!(
        data[0]["status"]["value"], "unloaded",
        "nothing has been asked for, so nothing is loaded -- and listing what \
         could be served is not a reason to start serving it"
    );
}

#[test]
fn the_properties_say_whether_the_router_loads_on_demand() {
    let serving = serving(CATALOG, ModelsRoot::with(&[MODEL]));
    let reply = request(serving.address(), &get("/props"));

    assert_eq!(status(&reply), Some(200), "got:\n{reply}");
    assert_eq!(
        body(&reply)["models_autoload"],
        Value::Bool(true),
        "this router starts a model when one is asked for, which is the whole \
         of what a client reads here"
    );
}

#[test]
fn an_entry_that_is_loaded_says_so() {
    let serving = serving(CATALOG, ModelsRoot::with(&[MODEL]));

    // Ask for it the ordinary way, which starts it.
    let served = request(serving.address(), &get("/models/gemma3/v1/echo"));
    assert_eq!(status(&served), Some(200), "got:\n{served}");

    let payload = body(&request(serving.address(), &get("/models")));
    assert_eq!(
        payload["data"][0]["status"]["value"], "loaded",
        "a client watches this to know what it need not wait for"
    );
}

#[test]
fn an_unloaded_entry_is_still_selectable_by_a_client() {
    // A client will not offer an unloaded model unless three things hold at
    // once: the server autoloads, the entry is not marked failed, and its
    // source is a preset. Miss the last and every model this router has not
    // yet started is invisible, while `/models` still looks perfectly valid --
    // which is the confusing failure this pins.
    let serving = serving(CATALOG, ModelsRoot::with(&[MODEL]));
    let payload = body(&request(serving.address(), &get("/models")));
    let entry = &payload["data"][0];

    assert_eq!(
        entry["status"]["value"], "unloaded",
        "nothing has been asked for yet"
    );
    assert_eq!(
        entry["source"], "preset",
        "an entry a catalog declares is a preset: it is configured here and \
         waiting, not downloaded and not discovered at runtime"
    );
    assert_eq!(
        entry["status"]["failed"],
        Value::Bool(false),
        "stated rather than absent, so a client reading it finds a boolean"
    );
}

#[test]
fn an_entry_carries_the_context_it_was_configured_with() {
    // A client sizes its own window from this rather than guessing, and a
    // guess here is the difference between using a 128K model and truncating
    // to whatever the client's default happens to be.
    let serving = serving(CATALOG, ModelsRoot::with(&[MODEL]));
    let payload = body(&request(serving.address(), &get("/models")));

    assert_eq!(
        payload["data"][0]["meta"]["n_ctx"], 4096,
        "the context_size the catalog states for this entry"
    );
}
