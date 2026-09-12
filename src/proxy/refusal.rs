//! Why a request was refused, and what a client is told about it.
//!
//! A refusal is a cause and the prose about it. The cause is what a program
//! reads: it fixes the status, a code that names the cause and nothing else,
//! and whether waiting can change the answer. The prose is what a person
//! reads, and it can be reworded without any client noticing, because no
//! client has to match it.
//!
//! Split from `reply` along that seam when the two together grew past the
//! module-size gate: this is a value with a table behind it, and `reply` is
//! the wire. Every module that refuses a request builds one of these and hands
//! it to `answer`, which is the one place a status is still possible.

use std::fmt;

use crate::launch::Failure;

/// Why a request was refused.
///
/// One variant per cause a client can act on differently, and no more: the
/// status and the code are read off this, so two causes a client would treat
/// the same way share a variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Cause {
    /// The head could not be read as a request at all.
    MalformedRequest,
    /// The head exceeded a bound this router reads within.
    HeadTooLarge,
    /// The caller stopped sending before the router had what it needed.
    RequestTimeout,
    /// The path is no shape this router serves.
    PathNotFound,
    /// The path or the body names a model the catalog does not carry.
    ModelNotFound,
    /// The method is none a model can be asked anything with. Carries the
    /// methods the endpoint does answer, which the reply says in `Allow`.
    MethodNotAllowed(&'static str),
    /// The request announced chunked framing, which is not implemented.
    ChunkedBody,
    /// The `Content-Length` will not parse.
    MalformedLength,
    /// The generic endpoint was sent no declared body.
    LengthRequired,
    /// The body is larger than the router will read.
    BodyTooLarge,
    /// The body ended before the length it declared.
    BodyIncomplete,
    /// The body is not JSON.
    BodyNotJson,
    /// The body names no model, on the endpoint that routes on one.
    ModelMissing,
    /// The child could not be started at all.
    ChildUnavailable,
    /// The child did not become ready inside its startup budget.
    StartupTimeout,
    /// The room this model needs is held by something that may finish.
    RoomContended,
    /// Nothing can be unloaded to make room.
    NoRoom,
}

impl Cause {
    /// The status and the code, in one place so they cannot disagree.
    fn describe(self) -> (u16, &'static str) {
        match self {
            Self::MalformedRequest => (400, "malformed_request"),
            Self::HeadTooLarge => (431, "request_head_too_large"),
            Self::RequestTimeout => (408, "request_timeout"),
            Self::PathNotFound => (404, "path_not_found"),
            Self::ModelNotFound => (404, "model_not_found"),
            Self::MethodNotAllowed(_) => (405, "method_not_allowed"),
            Self::ChunkedBody => (501, "chunked_body_not_implemented"),
            Self::MalformedLength => (400, "malformed_content_length"),
            Self::LengthRequired => (411, "content_length_required"),
            Self::BodyTooLarge => (413, "body_too_large"),
            Self::BodyIncomplete => (400, "body_incomplete"),
            Self::BodyNotJson => (400, "body_not_json"),
            Self::ModelMissing => (400, "model_missing"),
            Self::ChildUnavailable => (502, "child_unavailable"),
            Self::StartupTimeout => (504, "startup_timeout"),
            Self::RoomContended => (503, "room_contended"),
            Self::NoRoom => (503, "insufficient_room"),
        }
    }

    pub(super) fn status(self) -> u16 {
        self.describe().0
    }

    pub(super) fn code(self) -> &'static str {
        self.describe().1
    }

    /// How long a client should wait before trying again, when waiting is
    /// what changes the answer.
    ///
    /// Only the contended room earns this: the request that took the room
    /// first is loading or answering, and when it is done the room is free.
    /// Nothing else here is improved by waiting. A refusal for want of room
    /// names what holds the memory so the reader can decide; a timeout that
    /// is retried is the same timeout again.
    pub(super) fn retry_after_seconds(self) -> Option<u32> {
        match self {
            Self::RoomContended => Some(1),
            _ => None,
        }
    }

    /// The family an OpenAI-compatible client reads first.
    fn family(self) -> &'static str {
        if self.status() < 500 {
            "invalid_request_error"
        } else {
            "server_error"
        }
    }
}

/// A refusal the router authored: the cause, and the prose about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Refusal {
    cause: Cause,
    message: String,
}

impl Refusal {
    pub(super) fn new(cause: Cause, message: impl Into<String>) -> Self {
        Self {
            cause,
            message: message.into(),
        }
    }

    pub(super) fn cause(&self) -> Cause {
        self.cause
    }

    /// The envelope, as the body of the reply: the shape OpenAI-compatible
    /// clients already parse.
    pub(super) fn envelope(&self) -> String {
        serde_json::json!({
            "error": {
                "message": self.message,
                "type": self.cause.family(),
                "code": self.cause.code(),
            }
        })
        .to_string()
    }
}

/// The prose alone, for the unit tests that read a refusal as text.
impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

/// A launcher's failure, as the refusal it earns.
///
/// Distinguished by variant and never by prose: the wording of a failure is
/// not an interface, and the launch module says which of its variants means
/// what.
impl From<Failure> for Refusal {
    fn from(failure: Failure) -> Self {
        let cause = match &failure {
            Failure::NotReady(_) => Cause::StartupTimeout,
            Failure::Unavailable(_) => Cause::ChildUnavailable,
            Failure::Contended(_) => Cause::RoomContended,
            Failure::Refused(_) => Cause::NoRoom,
        };
        Self::new(cause, failure.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_contended_room_is_told_when_to_retry_and_a_permanent_refusal_is_not() {
        let contended = Refusal::from(Failure::Contended("held by another request".to_owned()));
        let permanent = Refusal::from(Failure::Refused("more than the whole budget".to_owned()));

        assert_eq!(contended.cause(), Cause::RoomContended);
        assert!(
            contended.cause().retry_after_seconds().is_some(),
            "the request that took the room first will finish, and waiting \
             for it is what changes this answer"
        );
        assert_eq!(permanent.cause(), Cause::NoRoom);
        assert!(
            permanent.cause().retry_after_seconds().is_none(),
            "nothing can be unloaded, so a client told to retry would retry \
             forever"
        );
        assert_eq!(
            (contended.cause().status(), permanent.cause().status()),
            (503, 503),
            "both are the service declining for now, told apart by code \
             rather than by status"
        );
    }

    #[test]
    fn a_launch_failure_keeps_its_prose_and_gains_a_code() {
        let refusal = Refusal::from(Failure::NotReady("entry 'x': 2 seconds".to_owned()));

        assert_eq!(refusal.cause(), Cause::StartupTimeout);
        assert_eq!(
            refusal.to_string(),
            "entry 'x': 2 seconds",
            "the wording is the launcher's own, untouched"
        );
        assert_eq!(
            Refusal::from(Failure::Unavailable(String::new())).cause(),
            Cause::ChildUnavailable
        );
    }

    #[test]
    fn the_envelope_carries_the_message_the_family_and_the_code() {
        let refusal = Refusal::new(Cause::ModelNotFound, "no model called 'x'");

        let parsed: serde_json::Value =
            serde_json::from_str(&refusal.envelope()).expect("the envelope is JSON");

        assert_eq!(parsed["error"]["message"], "no model called 'x'");
        assert_eq!(parsed["error"]["type"], "invalid_request_error");
        assert_eq!(parsed["error"]["code"], "model_not_found");
        assert_eq!(
            Refusal::new(Cause::ChildUnavailable, "").envelope(),
            r#"{"error":{"code":"child_unavailable","message":"","type":"server_error"}}"#,
            "a failure on the router's side of the conversation is a server \
             error, whatever the reader did"
        );
    }
}
