//! How fast an entry answers, in the unit that entry answers in.
//!
//! A generative entry is measured in tokens a second, and the figure is the
//! server's own: `llama-server` reports `timings.predicted_per_second` on
//! every completion, measured across generation alone. Timing it from out here
//! would fold in the connection, the prompt evaluation and this process's
//! scheduling, and would disagree with every other figure anyone has for these
//! models.
//!
//! An embedding or reranking entry has no such number, because it generates
//! nothing: there is no prediction to time, and the server reports no rate for
//! a forward pass. Those are timed from this side, out of necessity rather
//! than preference, and the figure therefore includes the round trip. It is
//! honest about what it is -- a throughput a caller would see, not a model's
//! intrinsic speed -- and it is comparable between runs of this command, which
//! is what a catalog decision needs.
//!
//! The two are kept in one type so that a report can print a column without
//! knowing which kind of entry produced the row, and cannot print a passage
//! rate under a heading that says tokens.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

use std::collections::BTreeMap;

use crate::catalog::Entry;

/// How long to wait for the measuring request before giving up.
///
/// Generous because a large model on a cold page cache is slow, and this is a
/// command someone runs deliberately rather than a request path.
const REPLY_TIMEOUT: Duration = Duration::from_secs(300);

/// How many tokens to generate when measuring a generative entry.
///
/// Enough that the rate is not dominated by the first token, short enough that
/// four large models are minutes rather than an afternoon.
const TOKENS: u32 = 128;

/// The prompt every generative entry is measured on.
///
/// Fixed so runs are comparable to each other. That makes them incomparable to
/// anyone else's benchmark, which is the trade this takes deliberately:
/// internal comparability is what decides a catalog number.
const PROMPT: &str = "Write a short paragraph explaining what a memory budget \
                      is and why a program might need one.";

/// How many passages an embedding or reranking entry is measured over.
///
/// One request carrying many, rather than many requests carrying one: a
/// reranker is asked for a whole candidate list in practice, and measuring it
/// a passage at a time would report the round trip rather than the model.
const PASSAGES: u32 = 32;

/// What one entry managed, in the unit it works in.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Throughput {
    /// Tokens a second, as the server itself reported them.
    Generated(f64),
    /// Passages a second, timed from here because nothing reports it.
    Scored(f64),
}

impl Throughput {
    /// The figure and the unit it is in, for a report that prints both.
    #[must_use]
    pub fn parts(self) -> (f64, &'static str) {
        match self {
            Self::Generated(rate) => (rate, "tok/s"),
            Self::Scored(rate) => (rate, "seq/s"),
        }
    }
}

/// Which endpoint answers for this entry, and so how it can be timed.
///
/// Read from the flags the server itself keys on, for the same reason the
/// estimator reads them: the file describes a model, and only the flags say
/// which way of running it is in front of us. An entry started with
/// `embeddings` will refuse a completion, and one started with `reranking`
/// will refuse both.
///
/// This asks a different question from the estimator's `keeps_no_cache`, which
/// happens to read the same two flags. That one decides what an entry costs;
/// this one decides how to address it. They are separate because they would
/// diverge the moment a server grew a mode that kept no cache and still
/// generated.
fn answers(flags: &BTreeMap<String, String>) -> Answers {
    let set = |name: &str| {
        flags.get(name).is_some_and(|value| {
            !matches!(value.trim().to_ascii_lowercase().as_str(), "false" | "0")
        })
    };
    if set("reranking") || set("rerank") {
        Answers::Reranking
    } else if set("embeddings") || set("embedding") {
        Answers::Embeddings
    } else {
        Answers::Completions
    }
}

/// The endpoint that will answer, and the unit the answer comes back in.
enum Answers {
    Completions,
    Embeddings,
    Reranking,
}

/// Measures the entry, in whichever unit it works in.
///
/// Returns `None` rather than an error: a rate that could not be read is worth
/// less than the memory reading beside it, and losing both would be worse.
pub(super) fn of(endpoint: std::net::SocketAddr, model: &Entry) -> Option<Throughput> {
    match answers(&model.flags) {
        Answers::Completions => generated(endpoint, &model.id),
        Answers::Embeddings => scored(endpoint, "/v1/embeddings", &embeddings(&model.id)),
        Answers::Reranking => scored(endpoint, "/v1/rerank", &reranking(&model.id)),
    }
}

/// Asks the server to generate, and reads the rate it reports.
fn generated(endpoint: std::net::SocketAddr, id: &str) -> Option<Throughput> {
    let body = serde_json::json!({
        "model": id,
        "messages": [{ "role": "user", "content": PROMPT }],
        "max_tokens": TOKENS,
        "stream": false,
        // Deterministic, so that a rate is not quietly measured against a
        // different amount of work each run.
        "temperature": 0.0,
    })
    .to_string();

    let reply = ask(endpoint, "/v1/chat/completions", &body).ok()?;
    let parsed: serde_json::Value = serde_json::from_str(body_of(&reply)?).ok()?;
    let rate = parsed
        .get("timings")?
        .get("predicted_per_second")?
        .as_f64()?;
    Some(Throughput::Generated(rate))
}

/// Times one request carrying every passage, and divides.
///
/// The reply is parsed before the clock is read from, so that a server which
/// answers an error quickly is not recorded as a fast one.
fn scored(endpoint: std::net::SocketAddr, path: &str, body: &str) -> Option<Throughput> {
    let started = Instant::now();
    let reply = ask(endpoint, path, body).ok()?;
    let elapsed = started.elapsed();
    let parsed: serde_json::Value = serde_json::from_str(body_of(&reply)?).ok()?;
    if parsed.get("error").is_some() {
        return None;
    }
    let seconds = elapsed.as_secs_f64();
    (seconds > 0.0).then(|| Throughput::Scored(f64::from(PASSAGES) / seconds))
}

/// A fixed batch, so that two runs measure the same work.
fn passages() -> Vec<String> {
    (0..PASSAGES)
        .map(|index| format!("Passage {index}. {PROMPT}"))
        .collect()
}

fn embeddings(id: &str) -> String {
    serde_json::json!({ "model": id, "input": passages() }).to_string()
}

fn reranking(id: &str) -> String {
    serde_json::json!({
        "model": id,
        "query": "what is a memory budget",
        "documents": passages(),
    })
    .to_string()
}

/// One HTTP round trip, hand-written for the same reason the router's is.
fn ask(endpoint: std::net::SocketAddr, path: &str, body: &str) -> std::io::Result<String> {
    let mut stream = TcpStream::connect(endpoint)?;
    stream.set_read_timeout(Some(REPLY_TIMEOUT))?;

    write!(
        stream,
        "POST {path} HTTP/1.1\r\n\
         Host: {endpoint}\r\n\
         Content-Type: application/json\r\n\
         Content-Length: {}\r\n\
         Connection: close\r\n\
         \r\n\
         {body}",
        body.len()
    )?;
    stream.flush()?;

    // `Connection: close` means end-of-file is the end of the reply, so the
    // length header does not have to be honoured to know when to stop.
    let mut reply = String::new();
    stream.read_to_string(&mut reply)?;
    Ok(reply)
}

/// Whatever followed the blank line.
fn body_of(reply: &str) -> Option<&str> {
    reply.split_once("\r\n\r\n").map(|(_, body)| body)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flags(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    #[test]
    fn an_entry_is_addressed_at_the_endpoint_its_flags_imply() {
        assert!(
            matches!(answers(&flags(&[])), Answers::Completions),
            "an entry that asks for neither mode generates"
        );
        assert!(
            matches!(
                answers(&flags(&[("embeddings", "true")])),
                Answers::Embeddings
            ),
            "a server told to embed will refuse a completion"
        );
        assert!(
            matches!(
                answers(&flags(&[("reranking", "true")])),
                Answers::Reranking
            ),
            "and one told to rerank refuses both"
        );
        assert!(
            matches!(
                answers(&flags(&[("embeddings", "false")])),
                Answers::Completions
            ),
            "read as the flags are elsewhere: an explicit false is not the mode"
        );
    }

    #[test]
    fn a_throughput_carries_the_unit_it_was_measured_in() {
        assert_eq!(Throughput::Generated(12.5).parts(), (12.5, "tok/s"));
        assert_eq!(
            Throughput::Scored(12.5).parts(),
            (12.5, "seq/s"),
            "a passage rate must not be printable under a heading that says \
             tokens -- they are different work and differ by orders of \
             magnitude"
        );
    }
}
