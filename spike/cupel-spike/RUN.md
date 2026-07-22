# Cupel spike — run this first

Proves the one unverified assumption in the whole plan: **a ZeroClaw *tool*
plugin can make an outbound HTTPS call.** Every HTTP plugin in the repo is a
*channel*; the only tool plugin makes no network calls; and the `tool-plugin`
WIT world does not declare `wasi:http`. The host does wire it in
(`runtime.rs::create_plugin` → `with_granted_http()` → `tool_linker_http()`),
but nobody has ever exercised that path.

## Place it

The `path: "../../wit/v0"` in `generate!` is relative, so this must live inside
a checkout of the plugins repo:

```bash
git clone https://github.com/zeroclaw-labs/zeroclaw-plugins.git
cp -r cupel-spike zeroclaw-plugins/plugins/
cd zeroclaw-plugins/plugins/cupel-spike
```

## Run it

```bash
rustup target add wasm32-wasip2

cargo test                                              # 1. pure core, no wasm toolchain
cargo clippy --all-targets -- -D warnings               # 2. host lint gate
cargo clippy --target wasm32-wasip2 -- -D warnings      # 3. wasm lint gate
cargo build --target wasm32-wasip2 --release            # 4. the real question

ls -la target/wasm32-wasip2/release/cupel_spike.wasm
```

Then commit `Cargo.lock` — CI runs everything with `--locked` and will fail
without it.

## Reading the outcome

**Step 4 succeeds** → the assumption holds, `cupel-core` is unblocked, and you
are the first HTTP-calling tool plugin in the ecosystem. Say so in the PR.

**Step 4 fails on a missing `wasi:http` import** → the tool world genuinely
cannot reach the network and the design needs rerouting. Better to know on day
one. Take the error to `#solana-bounty` in Discord — a host-side fix is small
and upstream has every reason to want it.

**Step 4 fails on wit-bindgen or waki version friction** → toolchain problem,
not an architecture problem. Pin whatever the reference plugins resolve to.

Either way, whatever fought you here goes in the write-up. The bounty says the
wasm32-wasip2 story is worth points, and this is the story.

## Runtime check (needs your machine, not just a compiler)

Compiling is most of the risk but not all of it. To confirm the host actually
grants HTTP at runtime:

```bash
zeroclaw plugin install /path/to/cupel-spike/
```

Set `rpc_url` in the plugin's config section, then ask the agent for the latest
blockhash. A real hash back means the path is proven end to end.

## Constraints already confirmed from source

- `__config` arrives as a **flat `HashMap<String, String>`** — no nesting, no
  numbers. Caps parse from strings; allowlists are comma-separated.
- The host **strips caller-supplied `__config`** before injecting the real
  section (`runtime.rs`, with tests firing a forged `__config` at it). Config
  guardrails are unspoofable by prompt injection — by the host, not by us.
- `wasm_path` must equal the cargo output filename exactly: crate `cupel-spike`
  → `cupel_spike.wasm`. Manifest `name` must equal the plugin directory name.
- CI runs `cargo test`, both clippy gates with `-D warnings`, and the release
  build — all `--locked`. For a touched plugin, **clippy failures block the
  merge**. Zero warnings is the bar.
- CI also diffs the source tree before and after building and fails if the
  build mutated it.
- Limits are generous: 1B fuel per call, 256 MB memory.
