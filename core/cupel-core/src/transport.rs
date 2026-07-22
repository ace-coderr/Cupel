//! The transport seam.
//!
//! `cupel-core` never makes a network call itself. It describes the call it
//! wants and hands it to a [`Transport`] the caller supplies. The wasm shim
//! implements this with `waki`; tests implement it with [`MockTransport`].
//!
//! This is the single decision that makes the rest of the crate host-testable.
//! `waki` only compiles for wasm, so a core that called it directly could never
//! satisfy the bounty's "cargo test with no wasm toolchain" requirement, and
//! could never be tested without hitting a live RPC.

use std::collections::HashMap;

/// A JSON-over-HTTP POST. The only capability `cupel-core` needs from the
/// outside world.
pub trait Transport {
    /// POST `body` to `url` as `application/json` and return the response body.
    ///
    /// Implementations must return `Err` for any non-success status, network
    /// failure, or timeout. Callers treat every `Err` as unverifiable, which
    /// renders as a `FAIL` verdict — never a permissive default.
    fn post_json(&self, url: &str, body: &str) -> Result<String, String>;
}

/// A transport that answers from a fixed table, keyed by RPC method name.
///
/// Used throughout the test suite so no test ever touches the network.
#[derive(Debug, Default, Clone)]
pub struct MockTransport {
    responses: HashMap<String, String>,
    /// When set, every call fails with this message regardless of method.
    failure: Option<String>,
}

impl MockTransport {
    pub fn new() -> Self {
        Self::default()
    }

    /// Answer any request whose body mentions `method` with `response`.
    #[must_use]
    pub fn with(mut self, method: &str, response: &str) -> Self {
        self.responses.insert(method.to_string(), response.to_string());
        self
    }

    /// Make every call fail, so tests can exercise the unverifiable path.
    #[must_use]
    pub fn failing(mut self, message: &str) -> Self {
        self.failure = Some(message.to_string());
        self
    }
}

impl Transport for MockTransport {
    fn post_json(&self, _url: &str, body: &str) -> Result<String, String> {
        if let Some(failure) = &self.failure {
            return Err(failure.clone());
        }
        for (method, response) in &self.responses {
            if body.contains(method) {
                return Ok(response.clone());
            }
        }
        Err(format!("MockTransport has no response registered for: {body}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mock_matches_on_method_name() {
        let t = MockTransport::new().with("getLatestBlockhash", r#"{"result":"ok"}"#);
        let out = t
            .post_json("https://ignored", r#"{"method":"getLatestBlockhash"}"#)
            .expect("registered method answers");
        assert!(out.contains("ok"));
    }

    #[test]
    fn unregistered_method_is_an_error_not_a_default() {
        let t = MockTransport::new().with("getLatestBlockhash", "{}");
        assert!(t.post_json("https://ignored", r#"{"method":"simulateTransaction"}"#).is_err());
    }

    #[test]
    fn failing_transport_fails_every_call() {
        let t = MockTransport::new()
            .with("getLatestBlockhash", "{}")
            .failing("RPC returned 429");
        let err = t
            .post_json("https://ignored", r#"{"method":"getLatestBlockhash"}"#)
            .expect_err("a failing transport must fail even registered methods");
        assert!(err.contains("429"));
    }
}
