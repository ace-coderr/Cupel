# cupel-core

Solana transaction effect verification for `wasm32-wasip2` agent plugins.

An agent that builds a transaction and asks a human to approve it has a
problem: the human approves a description the language model wrote. Poison the
model and the approval card reads "pay the supplier 25 USDC" while the bytes
underneath grant a delegate over the token account.

`cupel-core` decodes the transaction, simulates it against the operator's own
RPC, diffs the resulting account states, and reports the **observed** effect
against limits the operator declared.

## Design

- **No network.** Every RPC call goes through a `Transport` the caller
  supplies, so the crate is testable on the host with no wasm toolchain and no
  live endpoint.
- **No floats.** Money is `u128` base units with explicit decimals throughout.
- **No `solana-sdk`.** It does not compile for `wasm32-wasip2` inside a WIT
  component, so the wire format is parsed by hand: legacy and v0 messages,
  address lookup tables, SPL Token and Token-2022 account layouts.
- **Fail closed.** A decode failure, a simulation error, an unresolvable lookup
  table, or a malformed config value all produce a `FAIL` verdict. There is no
  permissive default anywhere in this crate.

## Example

```rust
use cupel_core::{preflight, Envelope, Pubkey};

let report = preflight(&transport, rpc_url, &tx_base64, &envelope, owner);
if !report.is_signable() {
    println!("{}", report.render());
}
```

```text
FAIL - envelope exceeded, authority granted

Pay        2,140.00 USDC  (cap 50.00)
Grants     delegate over your USDC account
           -> 9xQmR4vK...3nBwZ4mKp
Fee        0.000005 SOL
To         7xKXtg2C...W2ThgAsU  unknown

2 violations. Nothing signed.
```

Built for [ZeroClaw](https://github.com/zeroclaw-labs/zeroclaw) plugins.

## License

MIT OR Apache-2.0
