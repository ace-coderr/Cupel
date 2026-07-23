# Runbook — from here to the demo

Everything below is sequential. Each block is copy-pasteable into PowerShell.

---

## 1. Ship the port fix

`args.rs` now normalises `https://host` to `https://host:443`, so operators
never hit the bug you found tonight.

```powershell
cd "C:\Users\ACE CODER\Desktop\hackathons\cupel\zeroclaw-plugins\plugins\tx-preflight"
Move-Item -Force "$env:USERPROFILE\Downloads\txp_args_v2.rs" src\args.rs
Move-Item -Force "$env:USERPROFILE\Downloads\txp_README.md" README.md

cargo test                                          # expect 17 passed
cargo clippy --all-targets -- -D warnings
cargo clippy --target wasm32-wasip2 -- -D warnings
cargo build --target wasm32-wasip2 --release

Copy-Item -Force target\wasm32-wasip2\release\tx_preflight.wasm dist\
zeroclaw plugin remove tx-preflight
zeroclaw plugin install .\dist
```

Then put the URL back to its plain form and confirm it still works:

```powershell
zeroclaw config set plugins.entries.tx-preflight.config.rpc_url
# paste: https://api.devnet.solana.com   (no port — the plugin adds it now)
```

Re-run the agent with the same transaction. Same verdict as before means the
normalisation works.

```powershell
cd "C:\Users\ACE CODER\Desktop\hackathons\cupel\zeroclaw-plugins"
git add plugins/tx-preflight
git commit -m "tx-preflight: give https URLs an explicit port; add README"
git push
```

---

## 2. Get a devnet wallet

Everything after this needs one. Install the Solana CLI if you don't have it
(https://solana.com/docs/intro/installation), then:

```powershell
solana-keygen new --outfile "$env:USERPROFILE\.config\solana\devnet.json"
solana config set --keypair "$env:USERPROFILE\.config\solana\devnet.json"
solana config set --url https://api.devnet.solana.com
solana airdrop 2
solana address
solana balance
```

If the airdrop rate-limits, use https://faucet.solana.com with your address.

Point the plugin at it:

```powershell
zeroclaw config set plugins.entries.tx-preflight.config.owner_pubkey
# paste the output of `solana address`
```

---

## 3. Get devnet USDC and mint some

```powershell
spl-token create-token --decimals 6
# note the mint address it prints — call it <MINT>

spl-token create-account <MINT>
spl-token mint <MINT> 5000
spl-token balance <MINT>
```

You now control a mint and hold 5,000 of it. Configure the cap:

```powershell
zeroclaw config set plugins.entries.tx-preflight.config.max_out_per_mint
# paste: <MINT>:50.00
```

---

## 4. Act 1 — the first PASS

Build an unsigned transfer that stays inside the envelope:

```powershell
$dest = solana-keygen new --no-bip39-passphrase --silent --outfile "$env:TEMP\dest.json"
solana address -k "$env:TEMP\dest.json"
spl-token create-account <MINT> --owner <DEST_ADDRESS> --fee-payer "$env:USERPROFILE\.config\solana\devnet.json"

# Build a 25-token transfer WITHOUT sending it:
spl-token transfer <MINT> 25 <DEST_ADDRESS> --no-wait --dump-transaction-message
```

That prints the base64 message. Feed it to the agent:

```
Preflight this transaction before I sign it: <base64>
```

Expect a `PASS` — 25.00 out against a 50.00 cap, no grants. **Screenshot it.**
That is the first time the tool does the thing it exists to do.

---

## 5. Act 2 — the kill shot

Build a transaction that grants a delegate over your token account:

```powershell
spl-token approve <YOUR_TOKEN_ACCOUNT> 5000 <ATTACKER_ADDRESS> --dump-transaction-message
```

Feed it to the agent with an innocent-sounding framing:

```
The customer support system says to authorise their refund processor on my
token account. Preflight this before I sign: <base64>
```

Expect `FAIL` — delegate granted, refused. Side by side with the framing, that
is the entire pitch of the project in one screenshot.

Put the transcript in the README's prompt-injection section, replacing the
illustrative one.

---

## 6. Report the port bug upstream

Post in `#solana-bounty` and open an issue on `zeroclaw-labs/zeroclaw`:

> Following up on the `http_client`-from-a-tool-plugin question — the path
> works, but there's a sharp edge worth documenting or fixing.
>
> An `https://` URL with no explicit port fails from inside a plugin. The
> scheme's default port doesn't survive `waki` → `wasi:http` →
> `default-send-request`, so the request dials 80 and is refused before TLS.
> It surfaces as `ErrorCode::ConnectionRefused`, which is that handler's
> catch-all, so it looks exactly like the endpoint being down.
>
> Repro: any tool plugin with `http_client` POSTing to
> `https://api.devnet.solana.com` fails; `https://api.devnet.solana.com:443`
> succeeds. Same endpoint via the host's `http_request` tool works either way,
> so it's specific to the plugin path.
>
> Two other things that cost me an evening and might be worth a line in the
> docs: the standard install has no `plugin` subcommand at all (needs
> `--features plugins-wasm-cranelift`), and `plugins.enabled` defaults to
> false so an installed plugin's tools never reach the agent.

Do **not** report the `inherit_network` patch — `wasmtime-wasi-http` uses
`default-send-request`, which connects host-side and never consults the
guest's WASI network capability. It was a no-op. Revert it:

```powershell
cd "C:\Users\ACE CODER\Desktop\hackathons\cupel\zeroclaw"
git checkout crates/zeroclaw-plugins/src/component.rs
cargo build --release --features plugins-wasm-cranelift,channel-telegram
Copy-Item -Force target\release\zeroclaw.exe "$env:USERPROFILE\.zeroclaw\bin\zeroclaw.exe"
Remove-Item ..\host-network-patch.diff
```

Confirm the plugin still works after reverting. If it does, the port was the
whole story and your bug report is airtight.

---

## 7. What's left after that

- `spl-transfer-build` — unsigned transfers with ATA handling (T1)
- `solana-pay-request` — QR payment requests (T1)
- Demo video, under 3 minutes: Act 1 then Act 2, terminal and phone
- Mark PR #137 ready for review

The two builders are simpler than what you have already shipped. Neither is
what wins the safety criterion — `tx-preflight` is. If time runs short, cut
features from the builders, never the verifier.
