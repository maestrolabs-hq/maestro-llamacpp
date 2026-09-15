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

/// The failure this catches: an estimate that drifted had nowhere to show it.
///
/// The router measures every child as it becomes ready and holds the figure
/// beside the catalog's claim, but said so only on a stdout the service sends
/// to `/dev/null`. An entry declared 2431 MiB above what it held stayed that
/// way until somebody benched it by hand. The catalogue is where a client --
/// or an operator with `curl` -- can see both numbers without stopping
/// anything.
#[test]
fn an_entry_carries_what_it_was_estimated_to_hold_and_what_it_held() {
    let serving = serving(CATALOG, ModelsRoot::with(&[MODEL]));

    let before = body(&request(serving.address(), &get("/models")));
    assert_eq!(
        before["data"][0]["memory"]["declared_mib"], 512,
        "the estimate admission compares against, which the catalog states \
         whether or not anything is loaded:\n{before}"
    );
    assert!(
        before["data"][0]["memory"]["held_mib"].is_null(),
        "nothing has been loaded, so nothing has been measured:\n{before}"
    );

    request(serving.address(), &get("/models/gemma3/v1/echo"));
    let after = body(&request(serving.address(), &get("/models")));

    assert_eq!(
        after["data"][0]["status"]["value"], "loaded",
        "the request above started it:\n{after}"
    );
    assert!(
        after["data"][0]["memory"].get("held_mib").is_some(),
        "a loaded entry carries the measurement it was taken at, present and \
         null where no card could be read rather than missing:\n{after}"
    );
    assert_eq!(
        after["data"][0]["memory"]["declared_mib"], 512,
        "beside the estimate, which is what makes a drifted one visible:\n{after}"
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

/// A catalog carrying one entry that generates and one that only embeds.
///
/// The projector is named but never read: the router reports what an entry is
/// configured with, and `check` is the command that reads the files.
const MIXED: &str = concat!(
    "version = 1\n",
    "\n[defaults]\n",
    "context_size = 4096\n",
    "residency = \"on-demand\"\n",
    "memory_estimate_mib = 512\n",
    "startup_timeout_seconds = 30\n",
    "\n[models.gemma3]\n",
    "path = \"cache/gemma/gemma-3-1b.gguf\"\n",
    "projector_path = \"cache/gemma/mmproj.gguf\"\n",
    "\n[models.embed]\n",
    "path = \"cache/gemma/gemma-3-1b.gguf\"\n",
    "[models.embed.flags]\n",
    "embeddings = \"true\"\n",
);

#[test]
fn an_entry_that_cannot_hold_a_conversation_is_not_offered_as_one() {
    // A client reading this surface is choosing a model to talk to. An
    // embedding server answers one forward pass and returns a vector; a
    // reranker scores a pair. Offering either is offering a selection that can
    // only fail, and the client has no field it reads that would let it tell.
    //
    // Its own filter is `status`, `source` and `failed` -- none of which can
    // say "this is not a chat model" without lying about one of them. So the
    // entry is left out of this surface rather than described wrongly on it.
    // `/v1/models` still carries it, because a caller wanting an embedding
    // needs to find it there.
    let serving = serving(MIXED, ModelsRoot::with(&[MODEL]));
    let reply = request(serving.address(), &get("/models"));

    assert_eq!(status(&reply), Some(200), "got:\n{reply}");
    let payload = body(&reply);
    let ids: Vec<&str> = payload["data"]
        .as_array()
        .unwrap_or_else(|| panic!("a 'data' array, got:\n{payload}"))
        .iter()
        .filter_map(|entry| entry["id"].as_str())
        .collect();

    assert_eq!(
        ids,
        vec!["gemma3"],
        "only the entry that generates belongs on the router-mode surface"
    );
}

#[test]
fn an_entry_with_a_projector_says_it_takes_images() {
    // The client reads `architecture.input_modalities` and nothing else to
    // decide whether it may send an image. Left out, it assumes text, and a
    // model that was given a projector is offered as though it had none.
    //
    // Reported here rather than configured in the client, because the catalog
    // is the thing that knows: it names the projector. Any client reading this
    // surface gets the answer, not just the one that happens to be configured.
    let serving = serving(MIXED, ModelsRoot::with(&[MODEL]));
    let payload = body(&request(serving.address(), &get("/models")));
    let entry = payload["data"][0].clone();

    assert_eq!(
        entry["architecture"]["input_modalities"],
        serde_json::json!(["text", "image"]),
        "an entry naming a projector takes images: {entry}"
    );
}

#[test]
fn the_openai_listing_still_carries_what_the_router_surface_leaves_out() {
    // The counterpart to the test above, and the reason leaving an entry out
    // of one surface is not the same as hiding it. `/v1/models` is a
    // catalogue: a caller wanting an embedding looks it up by name and has
    // nowhere else to look. Filtering both would make the entry unreachable
    // rather than unoffered.
    let serving = serving(MIXED, ModelsRoot::with(&[MODEL]));
    let payload = body(&request(serving.address(), &get("/v1/models")));
    let mut ids: Vec<&str> = payload["data"]
        .as_array()
        .unwrap_or_else(|| panic!("a 'data' array, got:\n{payload}"))
        .iter()
        .filter_map(|entry| entry["id"].as_str())
        .collect();
    ids.sort_unstable();

    assert_eq!(
        ids,
        vec!["embed", "gemma3"],
        "the OpenAI catalogue carries every entry, including the ones the \
         router-mode menu does not offer"
    );
}
