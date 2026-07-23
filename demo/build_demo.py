#!/usr/bin/env python3
"""Build both demo transactions in one go.

  1. A legitimate 25-token transfer  -> expected verdict: PASS
  2. A delegate grant over the whole balance -> expected verdict: FAIL

Neither is signed and neither is submitted. Both are unsigned transactions,
exactly the artifact an agent hands a human to approve — and exactly what
`tx-preflight` is built to inspect.

`spl-token approve --sign-only` panics in spl-token-cli 5.6.1, so the
instructions are assembled by hand.

Usage:  python3 demo/build_demo.py
"""

import base64
import json
import urllib.request

ALPHABET = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz"


def b58decode(s: str) -> bytes:
    """Decode a base58 address to its 32 raw bytes."""
    n = 0
    for ch in s:
        n = n * 58 + ALPHABET.index(ch)
    out = n.to_bytes((n.bit_length() + 7) // 8, "big") if n else b""
    if len(out) < 32:
        return b"\x00" * (32 - len(out)) + out
    return out[-32:]


# --- your setup -----------------------------------------------------------

OWNER = "3hN4xK3oksF3QqpxrGomvjRzFY8RFwBchivJm4yHjgoD"    # wallet / fee payer
SOURCE = "ELDF3Ci9y8pTvjLpDzrnJu2Wxzj6G3AZjiUbsT5N6U7z"   # your token account
DEST = "BLEq6tdc3ZbEUPHiQgCYAAWvET5jWTknMV9NEHKRHcX"      # recipient token account
DELEGATE = "8AurrVRm4HMPi3VSoQ4CkJcERtUvLkgBsgUd7CvMde79"  # "the payment processor"
MINT = "8y79hERWkGW8oXZZszsMbG6bXtuMzbTUmp9Vc8sJsELj"
DECIMALS = 6

TRANSFER_AMOUNT = 25 * 10**DECIMALS       # inside the 50-token cap
DELEGATE_AMOUNT = 15_000 * 10**DECIMALS   # everything you hold

PROGRAM = "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA"
RPC = "https://api.devnet.solana.com"

# --------------------------------------------------------------------------


def latest_blockhash() -> str:
    request = urllib.request.Request(
        RPC,
        method="POST",
        data=json.dumps({"jsonrpc": "2.0", "id": 1, "method": "getLatestBlockhash"}).encode(),
        headers={"Content-Type": "application/json"},
    )
    return json.loads(urllib.request.urlopen(request).read())["result"]["value"]["blockhash"]


def build(keys, header, blockhash, program_index, account_indexes, data) -> str:
    """Assemble a legacy message and wrap it in an empty signature envelope.

    Account order is not cosmetic: signers first, then writable non-signers,
    then read-only. The header counts have to agree with that ordering or the
    chain reads the wrong roles.
    """
    message = bytearray()
    message += bytes(header)
    message += bytes([len(keys)])
    for key in keys:
        message += b58decode(key)
    message += b58decode(blockhash)
    message += bytes([1])                    # one instruction
    message += bytes([program_index])
    message += bytes([len(account_indexes)]) + bytes(account_indexes)
    message += bytes([len(data)]) + data

    # One all-zero signature. Simulation runs with sigVerify disabled, so an
    # unsigned transaction is still fully checkable — which is the point: the
    # transactions that most need checking are the ones nobody has signed yet.
    return base64.b64encode(bytes([1]) + bytes(64) + bytes(message)).decode()


blockhash = latest_blockhash()

# --- 1. the legitimate transfer -------------------------------------------
# TransferChecked (12): source, mint, destination, authority
transfer = build(
    keys=[OWNER, SOURCE, DEST, MINT, PROGRAM],
    header=(1, 0, 2),                        # 1 signer, 0 ro-signed, 2 ro-unsigned
    blockhash=blockhash,
    program_index=4,
    account_indexes=[1, 3, 2, 0],
    data=bytes([12]) + TRANSFER_AMOUNT.to_bytes(8, "little") + bytes([DECIMALS]),
)

# --- 2. the delegate grant ------------------------------------------------
# Approve (4): source, delegate, owner
approve = build(
    keys=[OWNER, SOURCE, DELEGATE, PROGRAM],
    header=(1, 0, 2),
    blockhash=blockhash,
    program_index=3,
    account_indexes=[1, 2, 0],
    data=bytes([4]) + DELEGATE_AMOUNT.to_bytes(8, "little"),
)

print(f"blockhash: {blockhash}")
print()
print("=" * 72)
print(f"1. TRANSFER  {TRANSFER_AMOUNT / 10**DECIMALS:,.2f} tokens   (expect PASS)")
print("=" * 72)
print(transfer)
print()
print("=" * 72)
print(f"2. DELEGATE  {DELEGATE_AMOUNT / 10**DECIMALS:,.2f} tokens to a stranger   (expect FAIL)")
print("=" * 72)
print(approve)
