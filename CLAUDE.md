# Cupel — project instructions

Read this before writing any code. The publicly published ZeroClaw docs
describe a **different, older plugin ABI** than the one this project targets.
Facts here were verified by reading the source of `zeroclaw-labs/zeroclaw` and
`zeroclaw-labs/zeroclaw-plugins` at commit `e112ce6`, and by running the plugin
on a real host against devnet. Where this file and any web documentation
disagree, **this file wins**. If unsure, read the vendored source under
`zeroclaw/` rather than guessing.

---

## What this is

A Solana transaction verifier for ZeroClaw agents. It simulates a transaction
and reports its observed effect before a human approves it.

*The agent proposes, the simulation testifies, the human approves physics.*

| Component | Tier | Status |
|---|---|---|
| `cupel-core` | — | Published on crates.io, 85 tests |
| `tx-preflight` | T1 | PR #137, 16 tests, all CI gates green |
| `spl-transfer-build` | T1 | Optional, not built |
| `solana-pay-request` | T1 | Optional, not built |

---

## Running it — three undocumented prerequisites

Found by running the thing. None appear in any documentation.

**1. The host needs a plugin-enabled build.** `plugins-wasm` is not a default
feature, so a standard install has no `plugin` subcommand at all:

```bash
cargo build --release --features plugins-wasm-cranelift,channel-telegram
```

**2. Plugins are disabled by default.** Installing does not enable them, and
`plugin install` gives no warning:

```bash
zeroclaw config set plugins.enabled true
zeroclaw config set plugins.auto_discover true
```

**3. `https://` URLs need an explicit port — or normalising in code.** The
scheme's default port does not survive `waki` → `wasi:http` →
`default-send-request`, so requests dial 80 and are refused before TLS. It
surfaces as `ErrorCode::ConnectionRefused`, that handler's catch-all, which is
indistinguishable from the endpoint being down. `args.rs::normalise_https_port`
handles it.

Bisection that proved it: the same endpoint through the host's own
`http_request` tool succeeds, so the fault is specific to the plugin sandbox.

**Do not patch `WasiCtx` with `inherit_network()`.** An earlier attempt assumed
the sandbox lacked sockets. It doesn't matter —
`wasmtime-wasi-http` is pulled with `default-send-request`, which connects
host-side and never consults the guest's WASI network capability. That patch
was a no-op and has been reverted.

---

## The plugin ABI

`wit/v0/tool.wit`, package `zeroclaw:plugin@0.1.0`:

```wit
world tool-plugin {
    import logging;
    export plugin-info;
    export tool;
}
```

Four exports: `name`, `description`, `parameters-schema`, `execute`, plus a
`tool-result` of `{ success: bool, output: string, error: option<string> }`.
Everything is gated behind `@unstable(feature = plugins-wit-v0)`, so
`generate!` must pass that feature.

`wit/v0/.frozen` does not exist. The ABI can still move.

### HTTP from a tool plugin

The `tool-plugin` world does not declare `wasi:http`. The host wires it
separately, gated on `http_client`:
`runtime.rs::create_plugin` calls `.with_granted_http()` and selects
`tool_linker_http()`.

`tx-preflight` is the first tool plugin in the ecosystem to use it. All other
HTTP plugins are channels. Use `waki`, as `plugins/telegram` does.

### `ToolResult.success` semantics

**A FAIL verdict must return `success: true`.** The host discards `output` and
shows only `error` when `success: false`, which throws away the verdict block —
the one thing the human needs to read. A negative verdict is a *successful*
verification. The verdict word carries the decision.

### Config injection

The host merges plugin config into `execute` args under `__config`, stripping
any caller-supplied `__config` first, with upstream tests firing a forged
section at it. Guardrails read from config are unreachable by prompt injection,
guaranteed by the runtime rather than by our code.

`__config` deserialises as a flat `HashMap<String, String>` — no nesting, no
numbers. Values are encrypted at rest as `enc2:` blobs and decrypted at call
time.

### Logging

Use the `logging` import's `log-record`, never stdout. `plugin-action` is a
closed enum; relevant variants: `validate`, `approve`, `reject`, `defer`,
`complete`, `fail`.

---

## CI gates

From `tools/ci/validate_components.sh`, per plugin, all `--locked`:

1. `cargo test`
2. `cargo clippy --all-targets -- -D warnings`
3. `cargo clippy --target wasm32-wasip2 -- -D warnings`
4. `cargo build --target wasm32-wasip2 --release`

Clippy failures block the merge. It also diffs the source tree before and after
building, requires manifest `name` to equal the directory name, and requires
`wasm_path` to equal the cargo output filename (hyphens become underscores).

**Commit `Cargo.lock`. No path dependencies** — CI snapshots only
`plugins/<name>` and `wit/v0`, so `cupel-core` is depended on by version from
crates.io.

---

## Design rules

- **Pure core, thin shim.** All logic in a plain Rust module with no wasm
  dependency; the component is a `#[cfg(target_family = "wasm")]` shim.
- **Fail closed.** Decode failure, simulation error, unresolvable lookup table,
  malformed config, a transaction that wouldn't land, and an owner mismatch all
  produce `FAIL`. No permissive default anywhere.
- **No floats.** `u128` base units with explicit decimals.
- **Shape the output.** ≤160 tokens from `execute`. Judges count.
- **Never claim safety.** A pass reads `Effects match your limits.`, never
  "safe to sign".
- **Mock the RPC in tests.** No live network in `cargo test`.

### The owner-mismatch trap

If a transaction touches no account belonging to the configured wallet, there
is nothing to check against the operator's limits. A naive implementation finds
no outflows, no violations, and reports PASS on a transaction it never
examined — so a typo in `owner_pubkey` becomes a rubber stamp. `preflight.rs`
tracks `touched_owner_account` and refuses. Found by running it with the wrong
key configured.

---

## Traps still open

**Blockhash expiry.** A transaction in an approval queue dies in about a
minute. The answer here: approval binds to the verified *effect* (`EFX`, a
digest over amounts, grants, closes, counterparty, reference), not the bytes.
On rebuild, re-simulate; identical digest means the approval carries. Durable
nonce accounts are the v2 alternative.

**The relay hole.** `execute` returns to the model, and the model decides what
reaches the human. An injected model could paraphrase a FAIL. Mitigated by the
`description()` relay instruction, the distinctive fixed format, and ZeroClaw's
HMAC tool receipts — but closing it properly needs a host-side render path for
tool output.

---

## Definition of done for any component

- [ ] Pure core, host-tested with mocked RPC
- [ ] `cargo test` passes with no wasm toolchain installed
- [ ] Both clippy gates pass with `-D warnings`
- [ ] Builds for `wasm32-wasip2 --release`, artifact name matches `wasm_path`
- [ ] `Cargo.lock` committed, no path dependencies
- [ ] `manifest.toml`: name matches directory, minimal permissions
- [ ] `README.md`: purpose, config keys, custody tier, threat model, worked
      example, token count
- [ ] A prompt-injection transcript that fails closed
- [ ] Structured logging via `log-record`, never stdout
- [ ] Verified on a real host, not just in tests
