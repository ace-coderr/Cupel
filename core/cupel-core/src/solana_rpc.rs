//! Solana JSON-RPC, over the injected [`Transport`].
//!
//! Two calls carry the whole design:
//!
//! - `getMultipleAccounts` — the **before** picture, plus resolving address
//!   lookup tables into real addresses.
//! - `simulateTransaction` — the **after** picture, from the validator rather
//!   than from the language model.
//!
//! Simulation runs with `sigVerify: false` and `replaceRecentBlockhash: true`.
//! An unsigned transaction sitting in an approval queue has no valid
//! signatures and often a stale blockhash, and neither has any bearing on what
//! the transaction *does*. Refusing to simulate it would mean refusing to
//! check the exact transactions that most need checking.

use serde_json::{json, Value};

use crate::message::{base64_encode, Message, Pubkey};
use crate::transport::Transport;

/// One account as the chain sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountSnapshot {
    pub address: Pubkey,
    pub lamports: u64,
    /// The program that owns this account, not the human who controls it.
    pub owner: Pubkey,
    pub data: Vec<u8>,
}

/// What the validator says happens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SimulationResult {
    /// `Some` when the transaction would fail on chain.
    pub err: Option<String>,
    pub logs: Vec<String>,
    /// Post-execution state, positionally matched to the requested addresses.
    pub accounts: Vec<Option<AccountSnapshot>>,
    pub units_consumed: Option<u64>,
}

/// Address lookup table header size; addresses begin here.
pub const LOOKUP_TABLE_META_SIZE: usize = 56;

pub struct RpcClient<'a, T: Transport> {
    transport: &'a T,
    url: String,
}

impl<'a, T: Transport> RpcClient<'a, T> {
    pub fn new(transport: &'a T, url: impl Into<String>) -> Self {
        Self {
            transport,
            url: url.into(),
        }
    }

    fn call(&self, method: &str, params: Value) -> Result<Value, String> {
        let body = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": method,
            "params": params,
        })
        .to_string();

        let raw = self.transport.post_json(&self.url, &body)?;
        let parsed: Value = serde_json::from_str(&raw)
            .map_err(|e| format!("{method} returned unparseable JSON: {e}"))?;

        if let Some(err) = parsed.get("error") {
            let message = err
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("unknown error");
            return Err(format!("{method}: {message}"));
        }

        parsed
            .get("result")
            .cloned()
            .ok_or_else(|| format!("{method} response had no result"))
    }

    /// Fetch accounts in one round trip. Positions are preserved; a missing
    /// account is `None`, not an omission.
    pub fn get_multiple_accounts(
        &self,
        addresses: &[Pubkey],
    ) -> Result<Vec<Option<AccountSnapshot>>, String> {
        if addresses.is_empty() {
            return Ok(Vec::new());
        }

        let encoded: Vec<String> = addresses.iter().map(|a| a.to_base58()).collect();
        let result = self.call(
            "getMultipleAccounts",
            json!([encoded, { "encoding": "base64", "commitment": "confirmed" }]),
        )?;

        let values = result
            .get("value")
            .and_then(Value::as_array)
            .ok_or_else(|| "getMultipleAccounts response had no value array".to_string())?;

        if values.len() != addresses.len() {
            return Err(format!(
                "asked for {} accounts, got {}",
                addresses.len(),
                values.len()
            ));
        }

        values
            .iter()
            .zip(addresses)
            .map(|(v, addr)| parse_account(v, *addr))
            .collect()
    }

    /// Simulate a transaction and capture the post-execution state of the
    /// accounts we care about.
    pub fn simulate(
        &self,
        transaction_base64: &str,
        accounts_of_interest: &[Pubkey],
    ) -> Result<SimulationResult, String> {
        let addresses: Vec<String> = accounts_of_interest
            .iter()
            .map(|a| a.to_base58())
            .collect();

        let mut config = json!({
            "encoding": "base64",
            "commitment": "confirmed",
            "sigVerify": false,
            "replaceRecentBlockhash": true,
        });

        if !addresses.is_empty() {
            config["accounts"] = json!({
                "encoding": "base64",
                "addresses": addresses,
            });
        }

        let result = self.call("simulateTransaction", json!([transaction_base64, config]))?;
        let value = result
            .get("value")
            .ok_or_else(|| "simulateTransaction response had no value".to_string())?;

        let err = match value.get("err") {
            None | Some(Value::Null) => None,
            Some(e) => Some(e.to_string()),
        };

        let logs = value
            .get("logs")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(String::from)
                    .collect()
            })
            .unwrap_or_default();

        let accounts = match value.get("accounts") {
            None | Some(Value::Null) => Vec::new(),
            Some(Value::Array(items)) => {
                if items.len() != accounts_of_interest.len() {
                    return Err(format!(
                        "simulation returned {} accounts for {} requested",
                        items.len(),
                        accounts_of_interest.len()
                    ));
                }
                items
                    .iter()
                    .zip(accounts_of_interest)
                    .map(|(v, addr)| parse_account(v, *addr))
                    .collect::<Result<Vec<_>, _>>()?
            }
            Some(_) => return Err("simulation accounts field was not an array".to_string()),
        };

        let units_consumed = value.get("unitsConsumed").and_then(Value::as_u64);

        Ok(SimulationResult {
            err,
            logs,
            accounts,
            units_consumed,
        })
    }

    /// Resolve a message's full account list, pulling addresses out of any
    /// lookup tables it references.
    ///
    /// Order matters and is not negotiable: static keys, then every table's
    /// writable entries in table order, then every table's readonly entries.
    /// [`Message::is_writable`] indexes into exactly this list.
    pub fn resolve_accounts(&self, message: &Message) -> Result<Vec<Pubkey>, String> {
        if message.lookups.is_empty() {
            return Ok(message.static_keys.clone());
        }

        let tables: Vec<Pubkey> = message.lookups.iter().map(|l| l.table).collect();
        let fetched = self.get_multiple_accounts(&tables)?;

        let mut tables_addresses = Vec::with_capacity(tables.len());
        for (snapshot, lookup) in fetched.iter().zip(&message.lookups) {
            let snapshot = snapshot.as_ref().ok_or_else(|| {
                format!("lookup table {} does not exist", lookup.table.to_base58())
            })?;
            tables_addresses.push(decode_lookup_table(&snapshot.data)?);
        }

        let mut writable = Vec::new();
        let mut readonly = Vec::new();

        for (addresses, lookup) in tables_addresses.iter().zip(&message.lookups) {
            for index in &lookup.writable_indexes {
                writable.push(index_into(addresses, *index, &lookup.table)?);
            }
            for index in &lookup.readonly_indexes {
                readonly.push(index_into(addresses, *index, &lookup.table)?);
            }
        }

        let mut resolved = message.static_keys.clone();
        resolved.extend(writable);
        resolved.extend(readonly);
        Ok(resolved)
    }
}

fn index_into(addresses: &[Pubkey], index: u8, table: &Pubkey) -> Result<Pubkey, String> {
    addresses.get(index as usize).copied().ok_or_else(|| {
        format!(
            "lookup table {} has {} addresses, transaction wants index {index}",
            table.to_base58(),
            addresses.len()
        )
    })
}

/// Read the addresses out of an address lookup table account.
pub fn decode_lookup_table(data: &[u8]) -> Result<Vec<Pubkey>, String> {
    if data.len() < LOOKUP_TABLE_META_SIZE {
        return Err(format!(
            "lookup table is {} bytes, expected at least {LOOKUP_TABLE_META_SIZE}",
            data.len()
        ));
    }

    let discriminator = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
    if discriminator != 1 {
        return Err(format!(
            "account discriminator is {discriminator}, expected 1 for a lookup table"
        ));
    }

    let body = &data[LOOKUP_TABLE_META_SIZE..];
    if body.len() % 32 != 0 {
        return Err(format!(
            "lookup table body is {} bytes, not a whole number of addresses",
            body.len()
        ));
    }

    Ok(body
        .chunks_exact(32)
        .map(|chunk| {
            let mut key = [0u8; 32];
            key.copy_from_slice(chunk);
            Pubkey(key)
        })
        .collect())
}

fn parse_account(value: &Value, address: Pubkey) -> Result<Option<AccountSnapshot>, String> {
    if value.is_null() {
        return Ok(None);
    }

    let lamports = value
        .get("lamports")
        .and_then(Value::as_u64)
        .ok_or_else(|| format!("account {} has no lamports field", address.to_base58()))?;

    let owner = value
        .get("owner")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("account {} has no owner field", address.to_base58()))
        .and_then(Pubkey::from_base58)?;

    // `data` is [base64, "base64"]; anything else means the encoding we asked
    // for is not the encoding we got, and guessing would be worse than failing.
    let data = match value.get("data") {
        Some(Value::Array(parts)) => {
            let encoded = parts
                .first()
                .and_then(Value::as_str)
                .ok_or_else(|| format!("account {} has malformed data", address.to_base58()))?;
            let encoding = parts.get(1).and_then(Value::as_str).unwrap_or("base64");
            if encoding != "base64" {
                return Err(format!("account data came back as {encoding}, wanted base64"));
            }
            crate::message::base64_decode(encoded)?
        }
        Some(Value::String(_)) => {
            return Err(format!(
                "account {} came back base58-encoded, wanted base64",
                address.to_base58()
            ))
        }
        _ => Vec::new(),
    };

    Ok(Some(AccountSnapshot {
        address,
        lamports,
        owner,
        data,
    }))
}

/// Encode a transaction for `simulateTransaction`.
pub fn encode_for_simulation(bytes: &[u8]) -> String {
    base64_encode(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::{decode_transaction, AddressTableLookup, MessageHeader, MessageVersion};
    use crate::transport::MockTransport;

    fn key(seed: u8) -> Pubkey {
        Pubkey([seed; 32])
    }

    fn account_json(lamports: u64, owner: Pubkey, data: &[u8]) -> String {
        format!(
            r#"{{"lamports":{lamports},"owner":"{}","data":["{}","base64"],"executable":false,"rentEpoch":0}}"#,
            owner.to_base58(),
            base64_encode(data)
        )
    }

    fn lookup_table_bytes(addresses: &[Pubkey]) -> Vec<u8> {
        let mut data = vec![0u8; LOOKUP_TABLE_META_SIZE];
        data[0..4].copy_from_slice(&1u32.to_le_bytes());
        for a in addresses {
            data.extend(a.0);
        }
        data
    }

    fn message_with_lookup() -> Message {
        Message {
            version: MessageVersion::V0,
            header: MessageHeader {
                num_required_signatures: 1,
                num_readonly_signed: 0,
                num_readonly_unsigned: 0,
            },
            static_keys: vec![key(1), key(2)],
            recent_blockhash: key(0xbb),
            instructions: Vec::new(),
            lookups: vec![AddressTableLookup {
                table: key(0x55),
                writable_indexes: vec![1],
                readonly_indexes: vec![0],
            }],
        }
    }

    #[test]
    fn fetches_accounts_positionally() {
        let response = format!(
            r#"{{"jsonrpc":"2.0","id":1,"result":{{"context":{{"slot":1}},"value":[{},null]}}}}"#,
            account_json(1_000, key(9), &[1, 2, 3])
        );
        let transport = MockTransport::new().with("getMultipleAccounts", &response);
        let client = RpcClient::new(&transport, "https://ignored");

        let out = client.get_multiple_accounts(&[key(1), key(2)]).unwrap();
        assert_eq!(out.len(), 2);

        let first = out[0].as_ref().expect("first account present");
        assert_eq!(first.lamports, 1_000);
        assert_eq!(first.owner, key(9));
        assert_eq!(first.data, vec![1, 2, 3]);
        assert!(out[1].is_none(), "a missing account stays a hole");
    }

    #[test]
    fn a_short_account_array_is_an_error() {
        let response = r#"{"jsonrpc":"2.0","id":1,"result":{"value":[null]}}"#;
        let transport = MockTransport::new().with("getMultipleAccounts", response);
        let client = RpcClient::new(&transport, "https://ignored");

        let err = client
            .get_multiple_accounts(&[key(1), key(2)])
            .expect_err("misaligned positions must not be silently accepted");
        assert!(err.contains("asked for 2"));
    }

    #[test]
    fn base58_account_data_is_rejected() {
        let response = format!(
            r#"{{"jsonrpc":"2.0","id":1,"result":{{"value":[{{"lamports":1,"owner":"{}","data":"someBase58"}}]}}}}"#,
            key(9).to_base58()
        );
        let transport = MockTransport::new().with("getMultipleAccounts", &response);
        let client = RpcClient::new(&transport, "https://ignored");
        assert!(client.get_multiple_accounts(&[key(1)]).is_err());
    }

    #[test]
    fn a_json_rpc_error_surfaces() {
        let response = r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32005,"message":"Node is behind"}}"#;
        let transport = MockTransport::new().with("getMultipleAccounts", response);
        let client = RpcClient::new(&transport, "https://ignored");

        let err = client.get_multiple_accounts(&[key(1)]).expect_err("rpc error");
        assert!(err.contains("Node is behind"));
    }

    #[test]
    fn a_transport_failure_surfaces() {
        let transport = MockTransport::new()
            .with("getMultipleAccounts", "{}")
            .failing("RPC returned 429");
        let client = RpcClient::new(&transport, "https://ignored");
        assert!(client.get_multiple_accounts(&[key(1)]).is_err());
    }

    #[test]
    fn simulation_reports_success_and_post_state() {
        let response = format!(
            r#"{{"jsonrpc":"2.0","id":1,"result":{{"value":{{"err":null,"logs":["Program log: ok"],"accounts":[{}],"unitsConsumed":1234}}}}}}"#,
            account_json(500, key(9), &[7, 7])
        );
        let transport = MockTransport::new().with("simulateTransaction", &response);
        let client = RpcClient::new(&transport, "https://ignored");

        let sim = client.simulate("AQAB", &[key(1)]).unwrap();
        assert!(sim.err.is_none());
        assert_eq!(sim.units_consumed, Some(1234));
        assert_eq!(sim.accounts.len(), 1);
        assert_eq!(sim.accounts[0].as_ref().unwrap().data, vec![7, 7]);
        assert_eq!(sim.logs.len(), 1);
    }

    #[test]
    fn a_failing_transaction_reports_its_error() {
        let response = r#"{"jsonrpc":"2.0","id":1,"result":{"value":{"err":{"InstructionError":[0,"InsufficientFunds"]},"logs":[],"accounts":null}}}"#;
        let transport = MockTransport::new().with("simulateTransaction", response);
        let client = RpcClient::new(&transport, "https://ignored");

        let sim = client.simulate("AQAB", &[]).unwrap();
        let err = sim.err.expect("a failing simulation reports err");
        assert!(err.contains("InsufficientFunds"));
    }

    #[test]
    fn simulation_account_count_must_match_the_request() {
        let response = r#"{"jsonrpc":"2.0","id":1,"result":{"value":{"err":null,"logs":[],"accounts":[]}}}"#;
        let transport = MockTransport::new().with("simulateTransaction", response);
        let client = RpcClient::new(&transport, "https://ignored");

        let err = client
            .simulate("AQAB", &[key(1)])
            .expect_err("positional mismatch must fail");
        assert!(err.contains("for 1 requested"));
    }

    #[test]
    fn decodes_a_lookup_table() {
        let addresses = decode_lookup_table(&lookup_table_bytes(&[key(3), key(4)])).unwrap();
        assert_eq!(addresses, vec![key(3), key(4)]);
    }

    #[test]
    fn rejects_a_non_lookup_table_account() {
        let mut data = lookup_table_bytes(&[key(3)]);
        data[0] = 2;
        assert!(decode_lookup_table(&data).is_err());
    }

    #[test]
    fn rejects_a_ragged_lookup_table() {
        let mut data = lookup_table_bytes(&[key(3)]);
        data.push(0);
        let err = decode_lookup_table(&data).expect_err("partial address must fail");
        assert!(err.contains("whole number of addresses"));
    }

    #[test]
    fn resolution_preserves_writable_then_readonly_order() {
        let response = format!(
            r#"{{"jsonrpc":"2.0","id":1,"result":{{"value":[{}]}}}}"#,
            account_json(1, key(9), &lookup_table_bytes(&[key(0xaa), key(0xbb)]))
        );
        let transport = MockTransport::new().with("getMultipleAccounts", &response);
        let client = RpcClient::new(&transport, "https://ignored");

        let resolved = client.resolve_accounts(&message_with_lookup()).unwrap();
        // statics, then writable index 1, then readonly index 0.
        assert_eq!(resolved, vec![key(1), key(2), key(0xbb), key(0xaa)]);
    }

    #[test]
    fn a_missing_lookup_table_fails_closed() {
        let response = r#"{"jsonrpc":"2.0","id":1,"result":{"value":[null]}}"#;
        let transport = MockTransport::new().with("getMultipleAccounts", response);
        let client = RpcClient::new(&transport, "https://ignored");

        let err = client
            .resolve_accounts(&message_with_lookup())
            .expect_err("an unresolvable table must not be skipped");
        assert!(err.contains("does not exist"));
    }

    #[test]
    fn an_out_of_range_lookup_index_fails_closed() {
        let response = format!(
            r#"{{"jsonrpc":"2.0","id":1,"result":{{"value":[{}]}}}}"#,
            account_json(1, key(9), &lookup_table_bytes(&[key(0xaa)]))
        );
        let transport = MockTransport::new().with("getMultipleAccounts", &response);
        let client = RpcClient::new(&transport, "https://ignored");

        let err = client
            .resolve_accounts(&message_with_lookup())
            .expect_err("index 1 into a one-address table must fail");
        assert!(err.contains("wants index 1"));
    }

    #[test]
    fn a_message_without_lookups_needs_no_round_trip() {
        let transport = MockTransport::new().failing("no call should have been made");
        let client = RpcClient::new(&transport, "https://ignored");

        let mut message = message_with_lookup();
        message.lookups.clear();

        let resolved = client.resolve_accounts(&message).unwrap();
        assert_eq!(resolved, vec![key(1), key(2)]);
    }

    #[test]
    fn simulation_requests_no_signature_check_and_a_fresh_blockhash() {
        // An unsigned, stale transaction in an approval queue must still be
        // checkable; refusing would mean refusing the ones that matter most.
        struct Spy;
        impl Transport for Spy {
            fn post_json(&self, _url: &str, body: &str) -> Result<String, String> {
                assert!(body.contains(r#""sigVerify":false"#));
                assert!(body.contains(r#""replaceRecentBlockhash":true"#));
                Ok(r#"{"jsonrpc":"2.0","id":1,"result":{"value":{"err":null,"logs":[]}}}"#
                    .to_string())
            }
        }
        let spy = Spy;
        RpcClient::new(&spy, "https://ignored")
            .simulate("AQAB", &[])
            .unwrap();
    }

    #[test]
    fn a_real_transaction_round_trips_into_simulation_encoding() {
        let mut tx = Vec::new();
        tx.push(0); // no signatures
        tx.extend([1, 0, 1]);
        tx.push(1);
        tx.extend(key(1).0);
        tx.extend(key(0xbb).0);
        tx.push(0);

        let encoded = encode_for_simulation(&tx);
        assert!(decode_transaction(&crate::message::base64_decode(&encoded).unwrap()).is_ok());
    }
}
