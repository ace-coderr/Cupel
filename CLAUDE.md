# Cupel — project instructions

Read this before writing any code. The publicly published ZeroClaw docs
describe a **different, older plugin ABI** than the one this project targets.
Facts in this file were verified by reading the actual source of
`zeroclaw-labs/zeroclaw` and `zeroclaw-labs/zeroclaw-plugins` at commit
`e112ce6b5ccdac9e1cb166bab217e730dd7e24c2`. Where this file and any web
documentation disagree, **this file wins**. If unsure, read the vendored source
under `zeroclaw/` and `zeroclaw-plugins/` rather than guessing.

---

## What we are building

A Solana payment terminal for ZeroClaw where **every outbound transaction is
simulation-verified before a human approves it**.

The thesis, stated once: *the agent proposes, the simulation testifies, the
human approves physics.*

Every competing submission will have the human approving a description that the
LLM wrote. Prompt-inject the agent and the approval card says "pay the supplier
25 USDC" while the bytes underneath grant a delegate over the token account.
Cupel closes that gap by simulating the transaction and reporting the observed
effect instead of the claimed one.

### Deliverables

| Component | Tier | Purpose |
|---|---|---|
| `cupel-core` | — | Published crates.io library: bs58, borsh, message codec, ALT resolution, JSON-RPC over `waki`, simulation decoding |
| `solana-pay-request` | T1 | "charge table 4 for 25 USDC" → Solana Pay URL + reference key. Holds no secrets. |
| `spl-transfer-build` | T1 | Unsigned versioned transfer, ATA handling, memo for reconciliation. Returns base64. |
| `tx-preflight` | T1 | The verifier. Simulates a transaction, diffs balances and authorities, renders a verdict against a config-declared envelope. |

Build order: spike → `cupel-core` → `tx-preflight` → `spl-transfer-build` →
`solana-pay-request`. If time runs short, cut features from the builders. Never
cut the verifier.

---

## Verified facts about the plugin system

### The WIT contract

`wit/v0/tool.wit`, package `zeroclaw:plugin@0.1.0`:

```wit
world tool-plugin {
    import logging;
    export plugin-info;
    export tool;
}
```

The `tool` interface exports exactly four functions — `name`, `description`,
`parameters-schema`, `execute` — plus a `tool-result` record of
`{ success: bool, output: string, error: option<string> }`. `plugin-info`
exports `plugin-name` and `plugin-version`.

Everything is gated behind `@unstable(feature = plugins-wit-v0)`, so
`generate!` must pass that feature or the interfaces are invisible.

`wit/v0/.frozen` **does not exist**. The ABI is experimental and may move.
Pin the upstream ref in the README.

### HTTP is real but unprecedented

The `tool-plugin` world does **not** declare a `wasi:http` import. HTTP is
wired separately by the host, gated on the `http_client` permission:

- `crates/zeroclaw-plugins/src/component.rs` — `with_granted_http()` attaches a
  `WasiHttpCtx` only when the scope grants `HttpClient`. There is a test named
  `grant_does_not_enable_http_without_adapter_opt_in`: the permission alone is
  not sufficient, the adapter must opt in.
- `crates/zeroclaw-plugins/src/runtime.rs::create_plugin` — the **tool** path
  calls `.with_granted_http()` and selects `tool_linker_http()` when the grant
  is present.

So a tool plugin declaring `permissions = ["http_client"]` gets `wasi:http`.

**But**: `redact-text` is the only tool plugin in the entire repository, and it
makes no network calls. All thirty other plugins are channels. No tool plugin
has ever declared `http_client`. Expect untrodden bugs. The spike exists to
prove this path before anything else is built.

Use `waki` for HTTP, exactly as `plugins/telegram` does:

```rust
waki::Client::new().post(url).json(body).send()
    .map_err(|e| e.to_string())?
    .json::<Value>().map_err(|e| e.to_string())
```

### Config injection — the security foundation

The host merges the plugin's resolved config into `execute` args under a
reserved `__config` key, **stripping any caller-supplied `__config` first so
the section cannot be spoofed** (`runtime.rs`, with tests firing
`{"prompt":"x","__config":{"api_key":"forged"}}` at it and asserting the
forgery is discarded).

This is why Cupel's guardrails actually hold: caps and allowlists read from
`__config` are unreachable by prompt injection, guaranteed by the host rather
than by our code. Cite this in the threat model.

**Constraint:** `__config` deserializes as a flat `HashMap<String, String>`.
No nesting. No numbers. Caps parse from strings; allowlists are
comma-separated. Design every config key around that shape.

### Limits

1,000,000,000 fuel per call, 256 MB memory, 100,000 table elements, 64
instances (`crates/zeroclaw-config/src/schema.rs`). Generous — decoding and
simulation parsing are not a concern.

### Logging

Use the `logging` import's `log-record`, never stdout. `plugin-action` is a
**closed enum** with no escape hatch. Relevant variants for us: `validate`,
`approve`, `reject`, `defer`, `complete`, `fail`.

---

## What the CI gate actually runs

From `tools/ci/validate_components.sh`, per plugin, all with `--locked`:

1. `cargo test --locked`
2. `cargo clippy --locked --all-targets -- -D warnings`
3. `cargo clippy --locked --target wasm32-wasip2 -- -D warnings`
4. `cargo build --locked --target wasm32-wasip2 --release`

For a plugin in the touched set, **clippy failures block the merge**. Zero
warnings is the bar, not a nicety.

It also:

- Diffs the source tree before and after building; a build that mutates its own
  source fails.
- Requires the artifact at
  `$CARGO_TARGET_DIR/wasm32-wasip2/release/<wasm_path>` to exist and be
  non-empty.
- Requires manifest `name` to equal the plugin directory name exactly.
- Validates `name` against `^[a-z0-9][a-z0-9_-]*$` and `wasm_path` against
  `^[A-Za-z0-9][A-Za-z0-9_.-]*\.wasm$`.

**`wasm_path` must equal the cargo output filename.** Crate `cupel-spike`
produces `cupel_spike.wasm` — hyphens become underscores.

**Commit `Cargo.lock`.** Every command uses `--locked` and will fail without it.

### No path dependencies

CI snapshots only `plugins/<name>` and `wit/v0` into a temp directory and
builds there. A `path = "../../crates/cupel-core"` dependency points at a
directory that will not exist in the snapshot, and the build fails. Every
existing plugin is a standalone workspace with crates.io-only dependencies.

Therefore **`cupel-core` is published to crates.io and imported by version.**
This is better anyway: the Track E prize asks for a library other tracks
actually import, and a published crate is importable by anyone.

---

## Coding conventions

Copied from `plugins/redact-text`, the canonical reference. Match it exactly.

- `edition = "2021"`, `license = "MIT OR Apache-2.0"`, `publish = false` on
  plugin crates (`cupel-core` publishes).
- `crate-type = ["cdylib", "rlib"]`.
- Empty `[workspace]` table at the bottom of every plugin `Cargo.toml` so cargo
  does not search for a parent workspace.
- `[profile.release]`: `opt-level = "s"`, `lto = true`, `strip = true`,
  `codegen-units = 1`.
- `waki` under `[target.'cfg(target_family = "wasm")'.dependencies]` so host
  tests never try to compile it.
- **Pure core, thin shim.** All logic in a plain Rust module with no wasm
  dependency. The component is a `#[cfg(target_family = "wasm")] mod component`
  that calls into it.
- `wit_bindgen::generate!({ path: "../../wit/v0", world: "tool-plugin",
  features: ["plugins-wit-v0"] })` — the path is relative, so plugins must live
  inside a checkout of `zeroclaw-plugins`.

### Behavioural rules

- **Fail closed.** Any decode failure, simulation error, unresolvable lookup
  table, or unexpected shape returns an error verdict, never a permissive one.
  Silence is not consent.
- **Shape the output.** A raw simulation response would nuke the agent's
  context and cost the operator money on every call. Target ≤160 tokens from
  `execute`. Judges will call it and count. Put the number in the README.
- **Never hold a private key.** All components are T0/T1. If a design pressures
  you toward T2, stop and reconsider.
- **Mock the RPC in tests.** No live network in `cargo test`.

---

## Traps

1. **Blockhash expiry.** A transaction sitting in an approval queue dies in
   about a minute. Our answer: approval binds to the *verified effect plus
   reference*, not the bytes. On rebuild, re-simulate; identical effect means
   the approval carries, any drift means re-ask. Document durable nonce
   accounts as the v2 alternative.
2. **`solana-sdk` and `solana-client` will not compile** for wasm32-wasip2
   inside a WIT component. Assemble with `bs58`, `borsh`, and hand-rolled
   instruction encoding. Document everything that fought you — the bounty says
   that write-up is worth points.
3. **Address lookup tables.** v0 transactions need an extra `getAccountInfo`
   round-trip to resolve. Token-2022 accounts append extensions after the
   165-byte base layout. Budget two days for the decoder; it is the part
   nobody else will get right.
4. **Do not read the published plugin-protocol docs.** They describe an Extism
   ABI with `wasm32-wasip1`, `extism-pdk`, `tool_metadata`, and
   `zc_http_request`. None of that applies here.

---

## Definition of done for any component

- [ ] Pure core with no wasm dependency, host-tested with mocked RPC
- [ ] `cargo test` passes with no wasm toolchain installed
- [ ] Both clippy gates pass with `-D warnings`
- [ ] Builds for `wasm32-wasip2 --release`, artifact name matches `wasm_path`
- [ ] `Cargo.lock` committed
- [ ] `manifest.toml`: name matches directory, minimal permissions, description
      says what it does
- [ ] `README.md`: purpose, config keys, custody tier, threat model, one worked
      example, token count
- [ ] A prompt-injection test that fails closed, transcript in the README
- [ ] Structured logging via `log-record`, never stdout
