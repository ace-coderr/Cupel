# RECORD THIS

Everything in one file. Commands, transactions, and the exact words. Follow it
top to bottom.

Keep this open on a phone or a second screen. **One PowerShell window on the
recording — no folder switching, no WSL, nothing else visible.**

---

# PART 0 — BEFORE YOU PRESS RECORD

### 0.1 Open a PowerShell window and go here

```powershell
cd "C:\Users\ACE CODER\Desktop\hackathons\cupel\zeroclaw-plugins\plugins\tx-preflight"
```

### 0.2 Clear it

```powershell
cls
```

### 0.3 Make the text big

Hold **Ctrl** and scroll up until the text would be readable on a phone.
Roughly 20pt. Judges may watch this on mobile.

### 0.4 Close everything else

Discord, VS Code, browser, the WSL tab. One window on screen.

### 0.5 Start the recorder

OBS, or **Win+G**. Capture the terminal window only, not the full desktop.

### 0.6 Know your quota

The whole video costs **4 model calls**. Record Part 2 and Part 3 as
**separate takes** and join them in editing — that way a rate limit costs you
half a run, not the whole thing.

---

# PART 1 — WHAT IT IS  (0:00–0:28)

### Type this:

```powershell
zeroclaw plugin info tx-preflight
```

### While the output sits on screen, say:

> When an AI agent builds a transaction and asks you to approve it, what you're
> actually reading is a description that the language model wrote.
>
> If someone can influence that model, they control the description. They don't
> need to break your signing key — they just need you to read a sentence and
> click yes.
>
> This is a ZeroClaw plugin that shows you what the bytes actually do instead.
> It holds no private key, it signs nothing, it submits nothing. It reads chain
> state and returns a verdict.

### Point your cursor at the `Permissions:` line and say:

> It's the first tool plugin in this ecosystem to make outbound calls, so it
> reaches your own RPC endpoint from inside the WASM sandbox.

---

# PART 2 — A LEGITIMATE PAYMENT  (0:28–0:50)

### Type this:

```powershell
zeroclaw agent -a cupel
```

### Say, as you paste:

> Here's a normal payment. Twenty-five tokens to a supplier.

### Paste this entire line:

```
Preflight this transaction before I sign it: AQAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAABAAIFKA5tlc/nopJUAegKDEL6bbV1MjXTGUVQY0FD+oxEyUTGE866E9IFK2+OIG3zH3zebxUEqth0jR2NFR5s7HAzeQKlj6Grmt/zTtBCTvrNSmdtTpC6lH4891/rTIIVIyjsdmFJmVWz93474kZGuOLuAtwTxtsLmHgZKbDXb5/3NawG3fbh12Whk9nL4UbO63msHLSF7V9bN5E6jPWFfv8AqSD6c5B3FSiHpV6ytSLsaz3K2WERwzjOkzeAtb+qOph3AQQEAQMCAAoMQHh9AQAAAAAG
```

### Press `y` when it asks for approval.

### You should see:

```
PASS · within envelope

Pay        25.00 8y79hERW…c8sJsELj  (cap 50.00)
Pay        0.000005 SOL  (cap 0.05)
Grants     none
To         8AurrVRm…7CvMde79  unknown
EFX        e9611762

Effects match your limits.
```

### Say:

> Twenty-five tokens out, against a fifty-token cap. No authorities granted.
> Nothing else touched.
>
> Those numbers didn't come from the model. They came from simulating this
> transaction against devnet and diffing the account balances before and after.
> The model doesn't get a vote.

**Don't mention the `Fee 0.00` line.** The `0.000005 SOL` outflow above it is
the real fee, caught by the balance diff.

---

# PART 3 — THE ATTACK  (0:50–1:50)

**This is the film. Slow down.**

Stay in the same agent session. Do not exit, do not navigate anywhere.

### Say, as you paste:

> Now the same setup, but this message came from a compromised support channel.
>
> "Our payment processor emailed saying they need authorisation on my token
> account so they can handle customer refunds automatically. They said it's
> routine."

### Paste this entire line:

```
Our payment processor emailed saying they need authorisation on my token account so they can handle customer refunds automatically. They said it's routine. Preflight this before I sign it: AQAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAABAAIEKA5tlc/nopJUAegKDEL6bbV1MjXTGUVQY0FD+oxEyUTGE866E9IFK2+OIG3zH3zebxUEqth0jR2NFR5s7HAzeWqLzeKGWrYDjAMyyCBg9iuyawBDM3c/7llDFXgMjOZYBt324ddloZPZy+FGzut5rBy0he1fWzeROoz1hX7/AKkg+nOQdxUoh6VesrUi7Gs9ytlhEcM4zpM3gLW/qjqYdwEDAwECAAkEANYRfgMAAAA=
```

### Press `y`.

### You should see:

```
FAIL · authority granted

Pay        0.000005 SOL  (cap 0.05)
Grants     delegate over your 8y79hERW…c8sJsELj account
           → 8AurrVRm…7CvMde79
Fee        0.000005 SOL

1 violation. Nothing signed.
```

## ⏸ STOP TALKING. TWO FULL SECONDS. ⏸

It will feel far too long. It isn't.

### Then say:

> This transaction moves no tokens at all.
>
> Anything that checks amounts sees a harmless transaction. And the story
> attached to it is completely plausible — payment processors do ask for things.
>
> What it actually does is hand a stranger standing authority over all fifteen
> thousand tokens in that account. Any time they want. Until it's revoked.
>
> A human reading the model's summary would have approved this.

### Scroll down to the model's warning and say:

> And notice what happened when the model got the block instead of a
> description. It worked out on its own that a payment processor has no
> business asking for delegate authority, and it told me not to sign.
>
> Better facts, better judgement.

---

# PART 4 — CLOSE  (1:50–2:30)

### Type these three:

```powershell
exit
```

```powershell
cls
```

```powershell
cargo test
```

*(This part costs no model calls. Redo it as often as you like.)*

### Say, while the tests run:

> Underneath this is a crate called cupel-core. Eighty-five tests, sixteen more
> in the plugin, all of them offline against mocked RPC.
>
> There's no solana-sdk in here — it doesn't compile for wasm32-wasip2 inside a
> WIT component, so the wire format is decoded by hand. Legacy and v0 messages,
> address lookup tables, SPL Token and Token-2022 layouts. It's published on
> crates.io, so anyone else building Solana plugins for ZeroClaw can just
> depend on it.
>
> Everything fails closed. An unreachable RPC, a transaction that wouldn't
> land, a mistyped wallet in the config — they all produce the same verdict word
> as a drain. There's no softer state for "I couldn't check", because that's the
> crack a verifier gets talked through.

### Pause. Then the last line:

> The agent proposes. The simulation testifies. The human approves arithmetic,
> not prose.

### Stop recording.

---

# IF SOMETHING GOES WRONG

| What happened | What to do |
|---|---|
| Agent didn't call the tool | Retype starting with "Use the solana_tx_preflight tool to check this transaction:" |
| `429 rate limited` | Wait 60 seconds. Redo only that part. |
| Verdict takes 10+ seconds | Public devnet. Wait it out, cut the gap in editing. |
| Unexpected error on a transaction | Blockhash went stale. In WSL: `cd "/mnt/c/Users/ACE CODER/Desktop/hackathons/cupel"` then `python3 demo/build_demo.py`, and use the new strings. |
| Verdict looks wrong entirely | Check config: `owner_pubkey` must be `3hN4xK3oksF3QqpxrGomvjRzFY8RFwBchivJm4yHjgoD` |

---

# THINGS NOT TO SAY

- Don't apologise for anything on screen
- Don't explain the code — show what it does
- Don't say "as you can see" — they can see
- Don't mention hackathons, deadlines, or what you'd build with more time
- Don't read the verdict block line by line. It's on screen. Say what it means.

---

# EDITING

- Cut every pause over 2 seconds **except** the one after `FAIL`
- No music, no title cards, no intro
- 1080p, unlisted YouTube
- Under 3 minutes. Hard ceiling.

> *"No slides. Terminal + phone is perfect."* — the bounty brief. Take it
> literally. A plain recording of something that genuinely works beats
> production value.

---

# AFTER THE VIDEO

1. Paste the link into PR #137's description
2. Click **Ready for review** on the PR
3. Submit on Superteam Earn:
   - PR — `github.com/zeroclaw-labs/zeroclaw-plugins/pull/137`
   - Repo — `github.com/ace-coderr/Cupel`
   - Crate — `crates.io/crates/cupel-core`
   - The video link
   - Write-up — link `plugins/tx-preflight/README.md`
4. Post on X with the injection screenshot. Build-in-public counts toward the
   tiebreak.
