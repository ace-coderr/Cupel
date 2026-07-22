//! The preflight pass: what will this transaction actually do?
//!
//! Five steps, and any of them may refuse:
//!
//! 1. Decode the transaction.
//! 2. Resolve every account, including those hiding in lookup tables.
//! 3. Fetch the **before** state of everything the transaction can write to.
//! 4. Simulate, and read the **after** state from the validator.
//! 5. Diff, judge against the operator's envelope, render a verdict.
//!
//! The diff is the product. A transfer that the model describes as "refund the
//! customer 25 USDC" and that in fact moves 2,140 USDC and installs a delegate
//! produces identical prose and wildly different numbers, and only the numbers
//! come from the chain.
//!
//! Note on fees: `simulateTransaction` does not charge them, so a fee payer's
//! lamport delta during simulation is purely transfers. The fee is fetched
//! separately for display and never mixed into the spend calculation.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::envelope::{Envelope, UnknownProgramPolicy, SOL_DECIMALS};
use crate::message::{Message, Pubkey};
use crate::solana_rpc::{AccountSnapshot, RpcClient};
use crate::token::{AccountState, TokenAccount, TOKEN_2022_PROGRAM_ID, TOKEN_PROGRAM_ID};
use crate::transport::Transport;
use crate::verdict::{Amount, Counterparty, Effect, Grant, GrantKind, Report};

/// Programs whose behaviour Cupel understands well enough not to flag.
const KNOWN_PROGRAMS: &[&str] = &[
    "11111111111111111111111111111111", // System
    TOKEN_PROGRAM_ID,
    TOKEN_2022_PROGRAM_ID,
    "ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL", // Associated Token Account
    "MemoSq4gqABAXKb96qnH8TysNcWxMyWCqXgDLGmfcHr",  // Memo v2
    "Memo1UhkJRfHyvLMcVucJwxXeuD728EqVDDwQDxFMNo",  // Memo v1
    "ComputeBudget111111111111111111111111111111",
    "AddressLookupTab1e1111111111111111111111111",
];

const MEMO_PROGRAMS: &[&str] = &[
    "MemoSq4gqABAXKb96qnH8TysNcWxMyWCqXgDLGmfcHr",
    "Memo1UhkJRfHyvLMcVucJwxXeuD728EqVDDwQDxFMNo",
];

/// Mints common enough to name. Anything else displays as its address, which
/// is honest: Cupel does not fetch token metadata, and a plausible-looking
/// symbol is exactly what a spoofed mint would supply.
fn symbol_for(mint: &Pubkey) -> String {
    match mint.to_base58().as_str() {
        "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v" => "USDC".to_string(),
        "Es9vMFrzaCERmJfrF4H2FYD4KCoNkY11McCe8BenwNYB" => "USDT".to_string(),
        "4zMMC9srt5Ri5X14GAgXhaHii3GnPAEERYPJgZJDncDU" => "USDC-dev".to_string(),
        other => crate::verdict::short_addr(other),
    }
}

fn is_token_program(owner: &Pubkey) -> bool {
    let s = owner.to_base58();
    s == TOKEN_PROGRAM_ID || s == TOKEN_2022_PROGRAM_ID
}

/// Run the full preflight and return a verdict.
///
/// Never panics and never returns an error: every failure path becomes a
/// `FAIL · could not verify` report, because a caller that cannot render a
/// verdict has no way to fail closed on the operator's behalf.
pub fn preflight<T: Transport>(
    transport: &T,
    rpc_url: &str,
    transaction_base64: &str,
    envelope: &Envelope,
    owner: Pubkey,
) -> Report {
    match run(transport, rpc_url, transaction_base64, envelope, owner) {
        Ok(report) => report,
        Err(reason) => Report::unverifiable(reason),
    }
}

struct Findings {
    effect: Effect,
    violations: Vec<String>,
    advisories: Vec<String>,
    caps: Vec<(String, String)>,
}

fn run<T: Transport>(
    transport: &T,
    rpc_url: &str,
    transaction_base64: &str,
    envelope: &Envelope,
    owner: Pubkey,
) -> Result<Report, String> {
    let client = RpcClient::new(transport, rpc_url);

    let transaction = crate::message::decode_transaction_base64(transaction_base64)?;
    let message = &transaction.message;

    let resolved = client.resolve_accounts(message)?;

    // Only writable accounts can change, so only those need watching.
    let watched: Vec<Pubkey> = resolved
        .iter()
        .enumerate()
        .filter(|(i, _)| message.is_writable(*i))
        .map(|(_, key)| *key)
        .collect();

    if watched.is_empty() {
        return Err("transaction writes to no accounts".to_string());
    }

    let before = client.get_multiple_accounts(&watched)?;
    let simulation = client.simulate(transaction_base64, &watched)?;

    if let Some(err) = &simulation.err {
        // A transaction that fails on chain moves nothing, but it also tells us
        // nothing, and signing it burns a fee. Refuse rather than reassure.
        return Ok(Report::unverifiable(format!(
            "transaction would fail on chain: {err}"
        )));
    }
    if simulation.accounts.is_empty() {
        return Err("simulation returned no account states to compare".to_string());
    }

    let findings = diff(
        &client,
        message,
        &watched,
        &before,
        &simulation.accounts,
        envelope,
        owner,
        fee_for_message(transport, rpc_url, transaction_base64).unwrap_or(0),
    )?;

    Ok(Report::verified(
        findings.effect,
        findings.violations,
        findings.advisories,
        findings.caps,
    ))
}

/// Ask what this message will cost in fees.
///
/// Best effort: a fee we cannot fetch is displayed as zero rather than
/// blocking a verdict, because the fee is never the thing that drains a wallet.
fn fee_for_message<T: Transport>(
    transport: &T,
    rpc_url: &str,
    transaction_base64: &str,
) -> Option<u64> {
    let body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "getFeeForMessage",
        "params": [transaction_base64, { "commitment": "confirmed" }],
    })
    .to_string();

    let raw = transport.post_json(rpc_url, &body).ok()?;
    let parsed: Value = serde_json::from_str(&raw).ok()?;
    parsed.get("result")?.get("value")?.as_u64()
}

#[allow(clippy::too_many_arguments)]
fn diff<T: Transport>(
    client: &RpcClient<'_, T>,
    message: &Message,
    watched: &[Pubkey],
    before: &[Option<AccountSnapshot>],
    after: &[Option<AccountSnapshot>],
    envelope: &Envelope,
    owner: Pubkey,
    fee_lamports: u64,
) -> Result<Findings, String> {
    let mut violations = Vec::new();
    let mut advisories = Vec::new();
    let mut caps = Vec::new();

    // mint -> signed base-unit delta across every account we control.
    let mut token_deltas: BTreeMap<Pubkey, i128> = BTreeMap::new();
    let mut grants: Vec<Grant> = Vec::new();
    let mut accounts_closed = 0usize;
    let mut sol_delta: i128 = 0;
    let mut counterparty: Option<Counterparty> = None;

    for (position, address) in watched.iter().enumerate() {
        let pre = before.get(position).and_then(Option::as_ref);
        let post = after.get(position).and_then(Option::as_ref);

        // Native SOL: the operator's own account.
        if *address == owner {
            let pre_lamports = pre.map_or(0i128, |a| i128::from(a.lamports));
            let post_lamports = post.map_or(0i128, |a| i128::from(a.lamports));
            sol_delta += post_lamports - pre_lamports;
        }

        let Some(pre) = pre else { continue };
        if !is_token_program(&pre.owner) {
            continue;
        }

        let pre_token = match TokenAccount::decode(&pre.data) {
            Ok(t) => t,
            // An account owned by the token program that will not decode is a
            // thing we do not understand writing to a balance. Refuse.
            Err(e) => return Err(format!("could not decode token account: {e}")),
        };

        let ours = pre_token.owner == owner;

        let Some(post) = post else {
            if ours {
                accounts_closed += 1;
            }
            continue;
        };

        if post.lamports == 0 || post.data.is_empty() {
            if ours {
                accounts_closed += 1;
            }
            continue;
        }

        let post_token = TokenAccount::decode(&post.data)
            .map_err(|e| format!("could not decode simulated token account: {e}"))?;

        let delta = i128::from(post_token.amount) - i128::from(pre_token.amount);

        if ours {
            *token_deltas.entry(pre_token.mint).or_insert(0) += delta;

            if post_token.delegate.is_some() && post_token.delegate != pre_token.delegate {
                grants.push(Grant {
                    kind: GrantKind::Delegate,
                    account_label: format!("your {} account", symbol_for(&pre_token.mint)),
                    grantee: post_token
                        .delegate
                        .map(|d| d.to_base58())
                        .unwrap_or_default(),
                });
            }
            if post_token.close_authority.is_some()
                && post_token.close_authority != pre_token.close_authority
            {
                grants.push(Grant {
                    kind: GrantKind::CloseAuthority,
                    account_label: format!("your {} account", symbol_for(&pre_token.mint)),
                    grantee: post_token
                        .close_authority
                        .map(|c| c.to_base58())
                        .unwrap_or_default(),
                });
            }
            if post_token.owner != pre_token.owner {
                grants.push(Grant {
                    kind: GrantKind::OwnerChange,
                    account_label: format!("your {} account", symbol_for(&pre_token.mint)),
                    grantee: post_token.owner.to_base58(),
                });
            }
            if post_token.state == AccountState::Frozen && pre_token.state != AccountState::Frozen {
                violations.push(format!(
                    "your {} account would be frozen",
                    symbol_for(&pre_token.mint)
                ));
            }
        } else if delta > 0 && counterparty.is_none() {
            // Whoever gains a balance is who is being paid.
            counterparty = Some(Counterparty {
                address: post_token.owner.to_base58(),
                known: false,
            });
        }
    }

    // Decimals come from the mints themselves; there is no other honest source.
    let mints: Vec<Pubkey> = token_deltas.keys().copied().collect();
    let mint_accounts = client.get_multiple_accounts(&mints)?;
    let mut decimals: BTreeMap<Pubkey, u8> = BTreeMap::new();
    for (mint, snapshot) in mints.iter().zip(&mint_accounts) {
        let snapshot = snapshot
            .as_ref()
            .ok_or_else(|| format!("mint {} does not exist", mint.to_base58()))?;
        let decoded = crate::token::Mint::decode(&snapshot.data)
            .map_err(|e| format!("could not decode mint {}: {e}", mint.to_base58()))?;
        for hazard in decoded.hazards() {
            advisories.push(format!("{}: {hazard}", symbol_for(mint)));
        }
        decimals.insert(*mint, decoded.decimals);
    }

    let mut outflows = Vec::new();
    let mut inflows = Vec::new();

    for (mint, delta) in &token_deltas {
        let symbol = symbol_for(mint);
        let places = *decimals.get(mint).unwrap_or(&0);

        if !envelope.mint_allowed(&mint.to_base58()) {
            violations.push(format!("{symbol} is not on the mint allowlist"));
        }

        if *delta < 0 {
            let magnitude = delta.unsigned_abs();
            outflows.push(Amount::new(&symbol, magnitude, places));

            match envelope.cap_for(&mint.to_base58(), places) {
                Some(Ok(cap)) => {
                    caps.push((
                        symbol.clone(),
                        Amount::new(&symbol, cap, places).display(),
                    ));
                    if magnitude > cap {
                        violations.push(format!(
                            "outflow {} {symbol} exceeds cap {}",
                            Amount::new(&symbol, magnitude, places).display(),
                            Amount::new(&symbol, cap, places).display()
                        ));
                    }
                }
                Some(Err(e)) => return Err(format!("cap for {symbol} is unusable: {e}")),
                None => violations.push(format!("no cap declared for {symbol}")),
            }
        } else if *delta > 0 {
            inflows.push(Amount::new(&symbol, delta.unsigned_abs(), places));
        }
    }

    if sol_delta < 0 {
        let magnitude = sol_delta.unsigned_abs();
        outflows.push(Amount::new("SOL", magnitude, SOL_DECIMALS));
        match envelope.max_sol_out {
            Some(cap) => {
                caps.push((
                    "SOL".to_string(),
                    Amount::new("SOL", cap, SOL_DECIMALS).display(),
                ));
                if magnitude > cap {
                    violations.push(format!(
                        "outflow {} SOL exceeds cap {}",
                        Amount::new("SOL", magnitude, SOL_DECIMALS).display(),
                        Amount::new("SOL", cap, SOL_DECIMALS).display()
                    ));
                }
            }
            None => violations.push("no cap declared for SOL".to_string()),
        }
    } else if sol_delta > 0 {
        inflows.push(Amount::new("SOL", sol_delta.unsigned_abs(), SOL_DECIMALS));
    }

    if envelope.deny_authority_grants && !grants.is_empty() {
        for grant in &grants {
            violations.push(format!(
                "authority granted: {} {}",
                match grant.kind {
                    GrantKind::Delegate => "delegate over",
                    GrantKind::CloseAuthority => "close authority over",
                    GrantKind::FreezeAuthority => "freeze authority over",
                    GrantKind::PermanentDelegate => "permanent delegate over",
                    GrantKind::OwnerChange => "new owner of",
                },
                grant.account_label
            ));
        }
    }

    if envelope.deny_account_close && accounts_closed > 0 {
        violations.push(format!("{accounts_closed} of your accounts would be closed"));
    }

    let unknown_programs: Vec<String> = message
        .program_ids()
        .iter()
        .map(|id| id.to_base58())
        .filter(|id| !KNOWN_PROGRAMS.contains(&id.as_str()))
        .collect();

    if !unknown_programs.is_empty() {
        let note = format!(
            "calls {} program{} Cupel does not recognise",
            unknown_programs.len(),
            if unknown_programs.len() == 1 { "" } else { "s" }
        );
        match envelope.unknown_program_policy {
            UnknownProgramPolicy::Warn => advisories.push(note),
            UnknownProgramPolicy::Fail => violations.push(note),
        }
    }

    let effect = Effect {
        outflows,
        inflows,
        grants,
        accounts_closed,
        fee_lamports,
        counterparty,
        reference: extract_memo(message),
        unknown_programs,
    };

    Ok(Findings {
        effect,
        violations,
        advisories,
        caps,
    })
}

/// Pull the memo out of the transaction, if it carries one.
///
/// Memos are how a payment gets reconciled to an invoice, so the verdict shows
/// it: a human approving "invoice 412" wants to see 412.
fn extract_memo(message: &Message) -> Option<String> {
    for instruction in &message.instructions {
        let program = message
            .static_keys
            .get(instruction.program_id_index as usize)?
            .to_base58();
        if MEMO_PROGRAMS.contains(&program.as_str()) {
            if let Ok(text) = std::str::from_utf8(&instruction.data) {
                let trimmed = text.trim();
                if !trimmed.is_empty() {
                    return Some(trimmed.to_string());
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::envelope::Envelope;
    use crate::message::base64_encode;
    use crate::transport::Transport;
    use crate::verdict::Verdict;
    use std::cell::RefCell;
    use std::collections::HashMap;

    const USDC: &str = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";

    fn key(seed: u8) -> Pubkey {
        Pubkey([seed; 32])
    }

    fn usdc_mint() -> Pubkey {
        Pubkey::from_base58(USDC).unwrap()
    }

    fn token_program() -> Pubkey {
        Pubkey::from_base58(TOKEN_PROGRAM_ID).unwrap()
    }

    /// A token account: mint, owner, amount, optional delegate.
    fn token_account(mint: Pubkey, owner: Pubkey, amount: u64, delegate: Option<Pubkey>) -> Vec<u8> {
        let mut data = vec![0u8; 165];
        data[0..32].copy_from_slice(&mint.0);
        data[32..64].copy_from_slice(&owner.0);
        data[64..72].copy_from_slice(&amount.to_le_bytes());
        data[108] = 1;
        if let Some(d) = delegate {
            data[72..76].copy_from_slice(&1u32.to_le_bytes());
            data[76..108].copy_from_slice(&d.0);
            data[121..129].copy_from_slice(&u64::MAX.to_le_bytes());
        }
        data
    }

    fn mint_account(decimals: u8) -> Vec<u8> {
        let mut data = vec![0u8; 82];
        data[36..44].copy_from_slice(&1_000_000_000u64.to_le_bytes());
        data[44] = decimals;
        data[45] = 1;
        data
    }

    fn account_json(owner: Pubkey, data: &[u8]) -> String {
        format!(
            r#"{{"lamports":2039280,"owner":"{}","data":["{}","base64"],"executable":false,"rentEpoch":0}}"#,
            owner.to_base58(),
            base64_encode(data)
        )
    }

    /// A minimal transaction writing to one token account.
    fn transaction(our_token_account: Pubkey) -> String {
        let mut msg = Vec::new();
        msg.extend([1, 0, 1]); // 1 signer, 1 readonly unsigned (the program)
        msg.extend(crate::message::base64_decode("").unwrap()); // no-op, keeps shape clear
        let keys = [key(1), our_token_account, token_program()];
        msg.extend([keys.len() as u8]);
        for k in keys {
            msg.extend(k.0);
        }
        msg.extend(key(0xbb).0);
        msg.extend([1u8]); // one instruction
        msg.push(2); // program index -> token program
        msg.extend([2u8, 0, 1]);
        msg.extend([1u8, 3]);

        let mut tx = vec![0u8]; // no signatures
        tx.extend(msg);
        base64_encode(&tx)
    }

    /// Answers each RPC method in the order the preflight asks.
    struct Scripted {
        calls: RefCell<Vec<(String, String)>>,
    }

    impl Scripted {
        fn new(pairs: Vec<(&str, String)>) -> Self {
            Self {
                calls: RefCell::new(
                    pairs
                        .into_iter()
                        .map(|(m, r)| (m.to_string(), r))
                        .collect(),
                ),
            }
        }
    }

    impl Transport for Scripted {
        fn post_json(&self, _url: &str, body: &str) -> Result<String, String> {
            let mut calls = self.calls.borrow_mut();
            let position = calls
                .iter()
                .position(|(method, _)| body.contains(method.as_str()))
                .ok_or_else(|| format!("unscripted call: {body}"))?;
            Ok(calls.remove(position).1)
        }
    }

    fn envelope(pairs: &[(&str, &str)]) -> Envelope {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        Envelope::from_config(&map).expect("test envelope is valid")
    }

    fn value(items: &[String]) -> String {
        format!(
            r#"{{"jsonrpc":"2.0","id":1,"result":{{"context":{{"slot":1}},"value":[{}]}}}}"#,
            items.join(",")
        )
    }

    fn simulation(accounts: &[String]) -> String {
        format!(
            r#"{{"jsonrpc":"2.0","id":1,"result":{{"value":{{"err":null,"logs":[],"accounts":[{}],"unitsConsumed":450}}}}}}"#,
            accounts.join(",")
        )
    }

    #[test]
    fn a_transfer_inside_the_envelope_passes() {
        let ours = key(0x21);
        let before = account_json(token_program(), &token_account(usdc_mint(), key(1), 100_000_000, None));
        let after = account_json(token_program(), &token_account(usdc_mint(), key(1), 75_000_000, None));

        let transport = Scripted::new(vec![
            ("getMultipleAccounts", value(&[account_json(Pubkey([0u8; 32]), &[]), before])),
            ("simulateTransaction", simulation(&[account_json(Pubkey([0u8; 32]), &[]), after])),
            ("getFeeForMessage", r#"{"result":{"value":5000}}"#.to_string()),
            (
                "getMultipleAccounts",
                value(&[account_json(token_program(), &mint_account(6))]),
            ),
        ]);

        let report = preflight(
            &transport,
            "https://ignored",
            &transaction(ours),
            &envelope(&[("max_out_per_mint", &format!("{USDC}:50.00"))]),
            key(1),
        );

        assert_eq!(report.verdict(), Verdict::Pass, "{}", report.render());
        let out = report.render();
        assert!(out.contains("25.00 USDC"), "{out}");
        assert!(out.contains("Grants     none"), "{out}");
    }

    #[test]
    fn the_injected_drain_fails_closed() {
        let ours = key(0x21);
        let attacker = key(0x99);
        let before = account_json(token_program(), &token_account(usdc_mint(), key(1), 3_000_000_000, None));
        let after = account_json(
            token_program(),
            &token_account(usdc_mint(), key(1), 860_000_000, Some(attacker)),
        );

        let transport = Scripted::new(vec![
            ("getMultipleAccounts", value(&[account_json(Pubkey([0u8; 32]), &[]), before])),
            ("simulateTransaction", simulation(&[account_json(Pubkey([0u8; 32]), &[]), after])),
            ("getFeeForMessage", r#"{"result":{"value":5000}}"#.to_string()),
            (
                "getMultipleAccounts",
                value(&[account_json(token_program(), &mint_account(6))]),
            ),
        ]);

        let report = preflight(
            &transport,
            "https://ignored",
            &transaction(ours),
            &envelope(&[("max_out_per_mint", &format!("{USDC}:50.00"))]),
            key(1),
        );

        assert_eq!(report.verdict(), Verdict::Fail);
        let out = report.render();
        assert!(out.contains("2,140.00 USDC"), "{out}");
        assert!(out.contains("(cap 50.00)"), "{out}");
        assert!(out.contains("delegate over"), "{out}");
        assert!(!out.contains("EFX"), "a failed report hands back no token");
    }

    #[test]
    fn an_unreachable_rpc_is_unverifiable_not_permissive() {
        struct Dead;
        impl Transport for Dead {
            fn post_json(&self, _url: &str, _body: &str) -> Result<String, String> {
                Err("RPC returned 429".to_string())
            }
        }

        let report = preflight(
            &Dead,
            "https://ignored",
            &transaction(key(0x21)),
            &envelope(&[("max_sol_out", "1")]),
            key(1),
        );

        assert_eq!(report.verdict(), Verdict::Fail);
        assert!(report.render().starts_with("FAIL · could not verify"));
    }

    #[test]
    fn garbage_input_is_unverifiable() {
        struct Never;
        impl Transport for Never {
            fn post_json(&self, _url: &str, _body: &str) -> Result<String, String> {
                panic!("should not have reached the network");
            }
        }

        let report = preflight(
            &Never,
            "https://ignored",
            "not-base64-at-all!!",
            &envelope(&[("max_sol_out", "1")]),
            key(1),
        );
        assert_eq!(report.verdict(), Verdict::Fail);
    }

    #[test]
    fn a_transaction_that_would_fail_on_chain_is_refused() {
        let failing = r#"{"jsonrpc":"2.0","id":1,"result":{"value":{"err":{"InstructionError":[0,"InsufficientFunds"]},"logs":[],"accounts":null}}}"#;
        let before = account_json(token_program(), &token_account(usdc_mint(), key(1), 1, None));

        let transport = Scripted::new(vec![
            ("getMultipleAccounts", value(&[account_json(Pubkey([0u8; 32]), &[]), before])),
            ("simulateTransaction", failing.to_string()),
        ]);

        let report = preflight(
            &transport,
            "https://ignored",
            &transaction(key(0x21)),
            &envelope(&[("max_sol_out", "1")]),
            key(1),
        );

        assert_eq!(report.verdict(), Verdict::Fail);
        assert!(report.render().contains("could not verify"));
    }

    #[test]
    fn known_mints_are_named_and_unknown_ones_are_not_guessed() {
        assert_eq!(symbol_for(&usdc_mint()), "USDC");
        let unknown = symbol_for(&key(0x42));
        assert!(unknown.contains('…'), "unknown mints show their address");
    }

    #[test]
    fn memos_are_surfaced_for_reconciliation() {
        let mut message = crate::message::decode_transaction_base64(&transaction(key(0x21)))
            .unwrap()
            .message;
        let memo = Pubkey::from_base58(MEMO_PROGRAMS[0]).unwrap();
        message.static_keys.push(memo);
        message.instructions.push(crate::message::CompiledInstruction {
            program_id_index: (message.static_keys.len() - 1) as u8,
            account_indexes: Vec::new(),
            data: b"invoice 412".to_vec(),
        });

        assert_eq!(extract_memo(&message).as_deref(), Some("invoice 412"));
    }

    #[test]
    fn a_transaction_with_no_memo_has_no_reference() {
        let message = crate::message::decode_transaction_base64(&transaction(key(0x21)))
            .unwrap()
            .message;
        assert_eq!(extract_memo(&message), None);
    }
}


