#!/usr/bin/env python3
"""Build an unsigned SPL Token Approve transaction — the delegate grant.

`spl-token approve --sign-only` panics in spl-token-cli 5.6.1, so the
instruction is assembled by hand. Nothing here signs or submits anything: the
output is an unsigned transaction, exactly the artifact an agent would hand a
human to approve.

Usage:  python3 build_approve.py
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


# --- edit these to match your setup ---------------------------------------

OWNER = "3hN4xK3oksF3QqpxrGomvjRzFY8RFwBchivJm4yHjgoD"     # your wallet, the authority
SOURCE = "ELDF3Ci9y8pTvjLpDzrnJu2Wxzj6G3AZjiUbsT5N6U7z"    # your token account
DELEGATE = "8AurrVRm4HMPi3VSoQ4CkJcERtUvLkgBsgUd7CvMde79"  # who gets standing access
AMOUNT = 15_000 * 10**6                                     # every token you hold

PROGRAM = "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA"
RPC = "https://api.devnet.solana.com"

# --------------------------------------------------------------------------

request = urllib.request.Request(
    RPC,
    method="POST",
    data=json.dumps({"jsonrpc": "2.0", "id": 1, "method": "getLatestBlockhash"}).encode(),
    headers={"Content-Type": "application/json"},
)
blockhash = json.loads(urllib.request.urlopen(request).read())["result"]["value"]["blockhash"]

# Account order is not cosmetic: signers first, then writable non-signers,
# then read-only. The header counts have to agree with that ordering.
keys = [OWNER, SOURCE, DELEGATE, PROGRAM]

message = bytearray()
message += bytes([1, 0, 2])          # 1 signer, 0 readonly-signed, 2 readonly-unsigned
message += bytes([len(keys)])
for key in keys:
    message += b58decode(key)
message += b58decode(blockhash)

# Approve, opcode 4: [source, delegate, owner]
message += bytes([1])                # one instruction
message += bytes([3])                # program index -> SPL Token
message += bytes([3]) + bytes([1, 2, 0])
data = bytes([4]) + AMOUNT.to_bytes(8, "little")
message += bytes([len(data)]) + data

# One all-zero signature. Simulation runs with sigVerify disabled, so an
# unsigned transaction is still fully checkable — which is the point: the
# transactions that most need checking are the ones nobody has signed yet.
transaction = bytes([1]) + bytes(64) + bytes(message)

print(f"blockhash: {blockhash}")
print(f"delegate:  {DELEGATE}")
print(f"amount:    {AMOUNT} base units = {AMOUNT / 10**6:,.2f} tokens")
print()
print(base64.b64encode(transaction).decode())
