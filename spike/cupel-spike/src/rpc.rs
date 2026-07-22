//! Pure JSON-RPC core. No wasm dependency, no network: this module compiles and
//! tests on the host with a plain `cargo test`, which is what the bounty's
//! "host-run tests" hard requirement asks for.

use serde_json::Value;

/// Default public endpoint used when the operator has not configured one.
pub const DEFAULT_RPC_URL: &str = "https://api.devnet.solana.com";

/// Shaped result of a `getLatestBlockhash` call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Blockhash {
    pub blockhash: String,
    pub last_valid_block_height: u64,
}

/// Build the JSON-RPC request body for `getLatestBlockhash`.
pub fn build_request() -> Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "getLatestBlockhash",
        "params": [{ "commitment": "confirmed" }],
    })
}

/// Resolve the RPC endpoint from the host-injected `__config` section.
///
/// The host strips any caller-supplied `__config` before injecting the real
/// one, so this value cannot be spoofed by the model.
pub fn resolve_rpc_url(config: &std::collections::HashMap<String, String>) -> String {
    config
        .get("rpc_url")
        .map(String::as_str)
        .filter(|u| !u.is_empty())
        .unwrap_or(DEFAULT_RPC_URL)
        .to_string()
}

/// Parse a `getLatestBlockhash` response.
///
/// Fails closed: any missing field, wrong type, or JSON-RPC error object
/// produces an `Err` rather than a partially-trusted value.
pub fn parse_response(body: &Value) -> Result<Blockhash, String> {
    if let Some(err) = body.get("error") {
        let message = err
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("unknown rpc error");
        return Err(format!("rpc error: {message}"));
    }

    let value = body
        .get("result")
        .and_then(|r| r.get("value"))
        .ok_or_else(|| "malformed response: missing result.value".to_string())?;

    let blockhash = value
        .get("blockhash")
        .and_then(Value::as_str)
        .ok_or_else(|| "malformed response: missing blockhash".to_string())?
        .to_string();

    let last_valid_block_height = value
        .get("lastValidBlockHeight")
        .and_then(Value::as_u64)
        .ok_or_else(|| "malformed response: missing lastValidBlockHeight".to_string())?;

    Ok(Blockhash {
        blockhash,
        last_valid_block_height,
    })
}

/// Shape the result into a compact line for the model.
///
/// Token discipline is the point: the RPC response is shrunk to one short
/// sentence rather than forwarded verbatim.
pub fn render(hash: &Blockhash) -> String {
    format!(
        "blockhash {} valid through block {}",
        hash.blockhash, hash.last_valid_block_height
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn request_targets_get_latest_blockhash() {
        let req = build_request();
        assert_eq!(req["method"], "getLatestBlockhash");
        assert_eq!(req["jsonrpc"], "2.0");
    }

    #[test]
    fn config_url_overrides_default() {
        let mut cfg = HashMap::new();
        cfg.insert("rpc_url".to_string(), "https://example.invalid".to_string());
        assert_eq!(resolve_rpc_url(&cfg), "https://example.invalid");
    }

    #[test]
    fn empty_config_falls_back_to_default() {
        assert_eq!(resolve_rpc_url(&HashMap::new()), DEFAULT_RPC_URL);
    }

    #[test]
    fn empty_url_falls_back_to_default() {
        let mut cfg = HashMap::new();
        cfg.insert("rpc_url".to_string(), String::new());
        assert_eq!(resolve_rpc_url(&cfg), DEFAULT_RPC_URL);
    }

    #[test]
    fn parses_a_well_formed_response() {
        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": {
                "context": { "slot": 123 },
                "value": {
                    "blockhash": "EkSnNWid2cvwEVnVx9aBqawnmiCNiDgp3gUdkDPTKN1N",
                    "lastValidBlockHeight": 3090
                }
            }
        });
        let parsed = parse_response(&body).expect("well-formed response parses");
        assert_eq!(parsed.last_valid_block_height, 3090);
        assert!(parsed.blockhash.starts_with("EkSn"));
    }

    #[test]
    fn rpc_error_object_fails_closed() {
        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "error": { "code": -32601, "message": "Method not found" }
        });
        let err = parse_response(&body).expect_err("an error object must not parse");
        assert!(err.contains("Method not found"));
    }

    #[test]
    fn missing_fields_fail_closed() {
        let body = serde_json::json!({ "result": { "value": {} } });
        assert!(parse_response(&body).is_err());
    }

    #[test]
    fn render_is_compact() {
        let hash = Blockhash {
            blockhash: "EkSnNWid2cvwEVnVx9aBqawnmiCNiDgp3gUdkDPTKN1N".to_string(),
            last_valid_block_height: 3090,
        };
        assert!(render(&hash).len() < 120);
    }
}
