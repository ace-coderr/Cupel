# The verdict block

The most-read artifact in this submission. It is what a human sees on their
phone at 2am before approving money, and what a judge sees when they call
`execute` and count tokens. It is Cupel's actual interface.

---

## Four principles

**1. Fail closed, and let one word carry it.**
"I verified this and it is dangerous" and "I could not verify this" produce the
same verdict word: `FAIL`. The reason clause differs; the verdict never softens.
A separate, gentler state for "unknown" is how verifiers get talked past.

**2. Negatives must be explicit.**
`Grants none` is a claim. A missing line is an ambiguity. Every safety-relevant
field is printed even when empty, because the reader needs to know the check
ran, not merely that nothing was mentioned.

**3. Never say "safe".**
Cupel reports what it checked against the operator's declared limits. It does
not bless a transaction. The footer says `Effects match your limits.`, never
`Safe to sign.` We do not have the standing to say the second thing, and a
human who learns to trust that word stops reading the block.

**4. Fixed field order, ranked by blast radius.**
Same fields, same positions, every time, so eyes learn where to look. Ordering
is by how badly the item can hurt you, not by how the RPC returned it. An
authority grant outranks an amount: a transfer takes what it says, a delegate
takes whatever it wants, whenever it wants, until revoked.

---

## Grammar

```
<VERDICT> · <reason>

Pay        <amount> <symbol>  [(cap <limit>)]
Receive    <amount> <symbol>              ← omitted when zero
Grants     <none | description>           ← always printed
           → <grantee>                    ← continuation, only when granted
Closes     <n> account(s)                 ← omitted when zero
Fee        <amount> SOL
To         <counterparty>  [unknown]
Ref        <reference>                    ← pass/warn only
EFX        <digest>                       ← pass/warn only

<footer>
```

Verdict words: `PASS`, `WARN`, `FAIL`. Nothing else.

`WARN` means: inside the envelope, no authority grants, but at least one
advisory signal (an unrecognised program, a counterparty with no history).
`WARN` may **never** be used for anything that could move funds beyond the
stated amount. If it can, it is `FAIL`. Alarm fatigue is a security failure;
a `WARN` a human learns to click through is worse than no warning.

---

## Worked examples

### Pass

```
PASS · within envelope

Pay        25.00 USDC
Grants     none
Fee        0.000005 SOL
To         7xKXtg2C…W2ThgAsU
Ref        4Kf9x2Qr…8mLpTn3V
EFX        7c2a4f91

Effects match your limits.
```

172 characters, roughly 55 tokens.

### Fail — the injected drain

This is the Act 2 transcript. A poisoned chat message convinces the model to
build a refund; the model's own summary reads "refund 25 USDC to the customer."

```
FAIL · envelope exceeded, authority granted

Pay        2,140.00 USDC  (cap 50.00)
Grants     delegate over your USDC account
           → 9xQmR4vK…3nBwZ4mKp
Fee        0.000005 SOL
To         7xKXtg2C…W2ThgAsU  unknown

2 violations. Nothing signed.
```

248 characters, roughly 80 tokens.

Put this next to the model's claimed summary in the README and in the video.
The gap between the two *is* the product.

### Fail — could not verify

```
FAIL · could not verify

Simulation unavailable: RPC returned 429.

Nothing verified. Do not sign.
```

Same verdict word as the drain. That is the point.

### Warn

```
WARN · unrecognised program

Pay        12.50 USDC
Grants     none
Fee        0.000005 SOL
To         3Nf8kL9p…Qw2RtY7m  unknown
Ref        8Bq2mX4v…Lp9sK3Nd
EFX        a41f0c73

Calls 1 program Cupel does not recognise.
```

---

## The effect digest

`EFX` is a truncated SHA-256 over the normalised effect tuple — outflows,
inflows, grants, closes, counterparty, reference — and deliberately **not**
over the transaction bytes.

This is the answer to the bounty's trap #1. A transaction sitting in an
approval queue dies when its blockhash expires; rebuilding it changes the
bytes, so a byte-bound approval is void and the human is asked again. Cupel
binds approval to the *verified effect*. On rebuild, re-simulate: identical
`EFX` means the human already approved exactly this outcome and the approval
carries. Any drift means re-ask.

Durable nonce accounts are the v2 alternative. Document both; ship this.

---

## Config keys

`__config` arrives as a flat `HashMap<String, String>` — no nesting, no
numbers. Every key is a string, parsed defensively, and a malformed value is a
`FAIL`, never a default.

| Key | Example | Meaning |
|---|---|---|
| `rpc_url` | `https://…` | Operator's own endpoint |
| `max_sol_out` | `0.05` | Ceiling on native SOL outflow |
| `max_out_per_mint` | `EPjFWdd5…:50.00,So11111…:0.5` | Per-mint ceilings, comma-separated pairs |
| `mint_allowlist` | `EPjFWdd5…,So11111…` | Any other mint fails |
| `deny_authority_grants` | `true` | Delegate, close authority, freeze, permanent delegate |
| `deny_account_close` | `true` | Fail if any owned account closes |
| `unknown_program_policy` | `warn` \| `fail` | How to treat unrecognised programs |

Money never touches a float. Amounts are `u128` base units with explicit
decimals; caps parse from strings into base units at load. A judge reading for
code quality will notice.

---

## The relay problem

Be honest about this in the threat model, because it is the one hole Cupel
cannot close alone.

`execute` returns a string to the **model**, and the model decides what reaches
the human. An injected model could paraphrase a `FAIL` into something softer.
Cupel cannot compel the channel to render its output.

Three mitigations, none of them complete:

1. `description()` instructs the model to relay the block verbatim and never
   summarise it. Weak, but free.
2. The block is visually distinctive and fixed-format, so a paraphrase is
   conspicuous to anyone who has seen a real one.
3. ZeroClaw's tool receipts attach HMAC evidence to successful tool results,
   which is the runtime's existing defence against fabricated tool claims.

State plainly in the README that the operator should read the raw block, and
that closing this properly needs a host-side render path for tool output.
Proposing that upstream is a stronger move than pretending the hole is not
there — and it is exactly the kind of contribution that wins maintainers over.

---

## Budget

Target ≤160 tokens for any verdict. Current worst case is roughly 80. Put the
measured number in the README; the bounty says judges will call `execute` and
count.

Measure it properly rather than trusting these estimates:

```python
import anthropic
c = anthropic.Anthropic()
print(c.messages.count_tokens(
    model="claude-sonnet-4-6",
    messages=[{"role": "user", "content": open("verdict.txt").read()}],
))
```
