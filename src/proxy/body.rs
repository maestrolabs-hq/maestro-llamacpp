//! The request body, and the model it names.
//!
//! The generic endpoint learns which model answers from the body rather than
//! the path, so this is the one thing the router reads that it used to copy.
//! The direction matters and is easy to misread: the **request** body is
//! buffered here, and the **response** relay is untouched. A request body is
//! complete before it is sent, so nothing streams into the router; a response
//! is produced token by token, and copying it without interpretation is what
//! makes it arrive as it is produced.
//!
//! Parsed with a library rather than scanned by hand, which is the opposite of
//! the decision the relay made, for the opposite reason. The relay must *not*
//! interpret a response, so writing it by hand removed the risk. Here the
//! router *must* interpret, correctly, and these are the cases a hand-written
//! search gets wrong:
//!
//! ```json
//! {"messages":[{"role":"user","content":"my model is gemma3"}],"model":"qwen38"}
//! {"messages":[{"role":"user","content":"say \"model\": \"x\""}],"model":"qwen38"}
//! {"response_format":{"model":"json"},"model":"qwen38"}
//! ```
//!
//! A key inside a string value, a key behind an escape, and a decoy at depth.
//! Getting any of them wrong routes a request to the wrong model, which then
//! answers plausibly and nothing downstream notices.

use std::io::Read;

use serde_json::Value;

use crate::launch::Failure;

/// The most a request body may declare before it is refused.
///
/// Far above a conversation and far below a size worth allocating on a
/// stranger's say-so. A multimodal request carrying a large base64 image is
/// the realistic way to approach it, which is why the refusal names the limit
/// rather than leaving the reader to guess what to change.
pub(super) const MAX_BODY_BYTES: usize = 8 * 1024 * 1024;

/// Reads exactly the declared body, and says which model it names.
///
/// Returns the bytes alongside the name so the caller forwards what it already
/// has rather than re-serialising a body it does not own.
///
/// # Errors
///
/// Returns a [`Failure`] when the declared length exceeds [`MAX_BODY_BYTES`],
/// when the body ends early, when it is not JSON, or when it names no model.
pub(super) fn read(
    reader: &mut impl Read,
    content_length: usize,
) -> Result<(Vec<u8>, String), Failure> {
    // Refused before anything is allocated. Checking after reading would mean
    // taking the memory first and objecting to it afterwards, which is the
    // bug the bound exists to prevent.
    if content_length > MAX_BODY_BYTES {
        return Err(refused(&format!(
            "a request body of {content_length} bytes is larger than the \
             {MAX_BODY_BYTES} this router will read"
        )));
    }

    let mut bytes = vec![0u8; content_length];
    reader.read_exact(&mut bytes).map_err(|error| {
        refused(&format!(
            "a request body declaring {content_length} bytes ended early: {error}"
        ))
    })?;

    // Parsed as a whole value rather than scanned, because the top-level key
    // is only findable by something that tracks strings, escapes and depth.
    let parsed: Value = serde_json::from_slice(&bytes)
        .map_err(|error| refused(&format!("a request body that is not JSON: {error}")))?;

    let model = parsed
        .get("model")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            refused(
                "the generic endpoint routes on the 'model' field of the \
                 request body, and this request carries none; name a model \
                 there, or address a model directly at /models/<model>/<path>",
            )
        })?
        .to_owned();

    Ok((bytes, model))
}

/// A body this router will not route on, and why.
fn refused(reason: &str) -> Failure {
    Failure::Unavailable(reason.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    /// Reads a whole body, the way a caller with a complete request does.
    fn model_of(body: &str) -> Result<String, Failure> {
        let mut source = Cursor::new(body.as_bytes().to_vec());
        read(&mut source, body.len()).map(|(_, model)| model)
    }

    #[test]
    fn a_body_names_the_model_it_carries() {
        assert_eq!(
            model_of(r#"{"model":"qwen38","messages":[]}"#).expect("a routed body"),
            "qwen38"
        );
    }

    #[test]
    fn the_bytes_come_back_as_they_arrived() {
        let body = r#"{"model":"qwen38","messages":[]}"#;
        let mut source = Cursor::new(body.as_bytes().to_vec());

        let (bytes, _) = read(&mut source, body.len()).expect("a routed body");

        assert_eq!(
            bytes,
            body.as_bytes(),
            "forwarded as received: the router is a relay, and re-serialising \
             would change a body it does not own"
        );
    }

    /// One rule -- only the top-level key routes -- against the three ways a
    /// decoy hides from a scanner that does not parse.
    ///
    /// Cases rather than three functions because the rule is one rule and only
    /// the input differs; three tests asserting the same thing about different
    /// text is data pretending to be behaviour. Each case carries what it
    /// would defeat, so a failure still names the trap that was walked into.
    #[test]
    fn only_the_top_level_model_key_routes_the_request() {
        let cases = [
            (
                r#"{"messages":[{"role":"user","content":"my model is gemma3"}],"model":"qwen38"}"#,
                "a scan for the word would have found the one in the conversation",
            ),
            (
                r#"{"messages":[{"role":"user","content":"say \"model\": \"gemma3\""}],"model":"qwen38"}"#,
                "a scan that did not track escapes would have found the escaped one",
            ),
            (
                r#"{"response_format":{"model":"json"},"model":"qwen38"}"#,
                "a scan that did not track depth would have found the decoy",
            ),
        ];

        for (body, defeated) in cases {
            assert_eq!(
                model_of(body).expect("a routed body"),
                "qwen38",
                "{defeated}: {body}"
            );
        }
    }

    #[test]
    fn a_body_naming_no_model_is_refused_and_says_what_to_do() {
        let refusal = model_of(r#"{"messages":[]}"#)
            .expect_err("the generic endpoint has nothing else to route on");
        let text = refusal.to_string();

        assert!(
            text.contains("model") && text.contains("/models/"),
            "the refusal says what is missing and names the endpoint that \
             needs no such field: {text}"
        );
    }

    #[test]
    fn a_body_that_is_not_json_is_refused_and_says_so() {
        let refusal = model_of("not json at all").expect_err("a body that cannot be read");

        assert!(
            refusal.to_string().contains("JSON"),
            "the refusal says what was wrong with it: {refusal}"
        );
    }

    #[test]
    fn a_declared_length_over_the_bound_is_refused_before_it_is_read() {
        // An empty reader: if the bound were checked after reading, this would
        // fail for ending early instead, which is a different message.
        let mut nothing = Cursor::new(Vec::new());

        let refusal = read(&mut nothing, MAX_BODY_BYTES + 1)
            .expect_err("a bad client cannot make the router allocate");

        assert!(
            refusal.to_string().contains(&MAX_BODY_BYTES.to_string()),
            "the refusal names the limit, so the reader knows what to change: {refusal}"
        );
    }

    #[test]
    fn a_body_shorter_than_it_declared_is_refused_rather_than_parsed() {
        let body = r#"{"model":"qwen38"}"#;
        let mut source = Cursor::new(body.as_bytes().to_vec());

        read(&mut source, body.len() + 10)
            .expect_err("a truncated body is not a body, whatever arrived of it");
    }
}
