//! Transaction decoding: legacy and v0, from scratch.
//!
//! `solana-sdk` does not compile for `wasm32-wasip2` inside a WIT component,
//! so the wire format is parsed by hand here. It is less code than it looks —
//! the whole format is compact-u16 length prefixes around fixed-size fields.
//!
//! ## Wire format
//!
//! ```text
//! transaction := shortvec<signature:64>  message
//!
//! message     := [0x80 | version]?          ← present only for versioned
//!                header:3
//!                shortvec<pubkey:32>        ← static account keys
//!                blockhash:32
//!                shortvec<instruction>
//!                shortvec<lookup>           ← v0 only
//!
//! instruction := program_index:1  shortvec<u8>  shortvec<u8>
//! lookup      := table:32  shortvec<u8>  shortvec<u8>
//! ```
//!
//! A legacy message is distinguished from a versioned one by its first byte:
//! the high bit is set on a version prefix, and cannot be set on a legacy
//! header because `num_required_signatures` is bounded well below 128.
//!
//! Every parse failure is an error. A partially-decoded transaction is never
//! returned, because a caller that sees `Ok` must be able to trust that every
//! account and instruction in the message is accounted for.

use std::fmt;

/// A 32-byte account address.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Pubkey(pub [u8; 32]);

impl Pubkey {
    pub fn to_base58(self) -> String {
        bs58::encode(self.0).into_string()
    }

    pub fn from_base58(s: &str) -> Result<Self, String> {
        let bytes = bs58::decode(s)
            .into_vec()
            .map_err(|e| format!("'{s}' is not valid base58: {e}"))?;
        let arr: [u8; 32] = bytes
            .try_into()
            .map_err(|_| format!("'{s}' is not 32 bytes"))?;
        Ok(Self(arr))
    }
}

impl fmt::Display for Pubkey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_base58())
    }
}

/// Which accounts must sign, and which are read-only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MessageHeader {
    pub num_required_signatures: u8,
    pub num_readonly_signed: u8,
    pub num_readonly_unsigned: u8,
}

/// An instruction as it appears on the wire: indices, not addresses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompiledInstruction {
    pub program_id_index: u8,
    pub account_indexes: Vec<u8>,
    pub data: Vec<u8>,
}

/// A reference to accounts stored in an on-chain address lookup table.
///
/// The addresses themselves are not in the transaction — resolving them needs
/// a `getAccountInfo` round-trip against the table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddressTableLookup {
    pub table: Pubkey,
    pub writable_indexes: Vec<u8>,
    pub readonly_indexes: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageVersion {
    Legacy,
    V0,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub version: MessageVersion,
    pub header: MessageHeader,
    pub static_keys: Vec<Pubkey>,
    pub recent_blockhash: Pubkey,
    pub instructions: Vec<CompiledInstruction>,
    pub lookups: Vec<AddressTableLookup>,
}

impl Message {
    /// Total accounts referenced, including those still inside lookup tables.
    pub fn total_accounts(&self) -> usize {
        self.static_keys.len()
            + self
                .lookups
                .iter()
                .map(|l| l.writable_indexes.len() + l.readonly_indexes.len())
                .sum::<usize>()
    }

    /// Whether the account at `index` can be mutated by this transaction.
    ///
    /// Ordering after the static keys is: every lookup's writable indexes in
    /// table order, then every lookup's readonly indexes in table order.
    pub fn is_writable(&self, index: usize) -> bool {
        let signers = self.header.num_required_signatures as usize;
        let statics = self.static_keys.len();

        if index < signers {
            return index < signers - self.header.num_readonly_signed as usize;
        }
        if index < statics {
            return index < statics - self.header.num_readonly_unsigned as usize;
        }
        let writable_loaded: usize = self.lookups.iter().map(|l| l.writable_indexes.len()).sum();
        index < statics + writable_loaded
    }

    /// Whether the account at `index` must sign.
    pub fn is_signer(&self, index: usize) -> bool {
        index < self.header.num_required_signatures as usize
    }

    /// The fee payer: always the first account.
    pub fn fee_payer(&self) -> Option<Pubkey> {
        self.static_keys.first().copied()
    }

    /// Distinct programs invoked at the top level.
    ///
    /// Inner instructions are not visible here — they come from simulation.
    pub fn program_ids(&self) -> Vec<Pubkey> {
        let mut ids: Vec<Pubkey> = self
            .instructions
            .iter()
            .filter_map(|ix| self.static_keys.get(ix.program_id_index as usize).copied())
            .collect();
        ids.sort_unstable();
        ids.dedup();
        ids
    }

    /// True when accounts must be resolved from lookup tables before the
    /// transaction can be fully understood.
    pub fn needs_lookup_resolution(&self) -> bool {
        !self.lookups.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transaction {
    pub signatures: Vec<Vec<u8>>,
    pub message: Message,
}

/// A byte reader that refuses to read past the end.
struct Cursor<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }

    fn remaining(&self) -> usize {
        self.bytes.len().saturating_sub(self.pos)
    }

    fn peek(&self) -> Result<u8, String> {
        self.bytes
            .get(self.pos)
            .copied()
            .ok_or_else(|| "unexpected end of transaction".to_string())
    }

    fn u8(&mut self) -> Result<u8, String> {
        let b = self.peek()?;
        self.pos += 1;
        Ok(b)
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        if self.remaining() < n {
            return Err(format!(
                "unexpected end of transaction: wanted {n} bytes, {} left",
                self.remaining()
            ));
        }
        let slice = &self.bytes[self.pos..self.pos + n];
        self.pos += n;
        Ok(slice)
    }

    fn pubkey(&mut self) -> Result<Pubkey, String> {
        let bytes = self.take(32)?;
        let mut key = [0u8; 32];
        key.copy_from_slice(bytes);
        Ok(Pubkey(key))
    }

    /// compact-u16: up to three bytes, seven bits each, high bit continues.
    fn shortvec_len(&mut self) -> Result<usize, String> {
        let mut value: usize = 0;
        for group in 0..3 {
            let byte = self.u8()?;
            value |= ((byte & 0x7f) as usize) << (group * 7);
            if byte & 0x80 == 0 {
                // Reject non-canonical encodings: a continuation byte that
                // added nothing means the length was padded.
                if group > 0 && byte == 0 {
                    return Err("non-canonical shortvec length".to_string());
                }
                return Ok(value);
            }
        }
        Err("shortvec length exceeds three bytes".to_string())
    }
}

/// Decode a base64-encoded transaction.
pub fn decode_transaction_base64(encoded: &str) -> Result<Transaction, String> {
    let bytes = base64_decode(encoded.trim())?;
    decode_transaction(&bytes)
}

/// Decode a wire-format transaction.
pub fn decode_transaction(bytes: &[u8]) -> Result<Transaction, String> {
    let mut cursor = Cursor::new(bytes);

    let sig_count = cursor.shortvec_len()?;
    if sig_count > 64 {
        return Err(format!("implausible signature count: {sig_count}"));
    }
    let mut signatures = Vec::with_capacity(sig_count);
    for _ in 0..sig_count {
        signatures.push(cursor.take(64)?.to_vec());
    }

    let message = decode_message_at(&mut cursor)?;

    if cursor.remaining() != 0 {
        return Err(format!(
            "{} trailing bytes after message",
            cursor.remaining()
        ));
    }

    Ok(Transaction {
        signatures,
        message,
    })
}

/// Decode a bare message, with no signature envelope.
pub fn decode_message(bytes: &[u8]) -> Result<Message, String> {
    let mut cursor = Cursor::new(bytes);
    let message = decode_message_at(&mut cursor)?;
    if cursor.remaining() != 0 {
        return Err(format!(
            "{} trailing bytes after message",
            cursor.remaining()
        ));
    }
    Ok(message)
}

fn decode_message_at(cursor: &mut Cursor<'_>) -> Result<Message, String> {
    let first = cursor.peek()?;
    let version = if first & 0x80 == 0 {
        MessageVersion::Legacy
    } else {
        let prefix = cursor.u8()?;
        match prefix & 0x7f {
            0 => MessageVersion::V0,
            other => return Err(format!("unsupported message version: {other}")),
        }
    };

    let header = MessageHeader {
        num_required_signatures: cursor.u8()?,
        num_readonly_signed: cursor.u8()?,
        num_readonly_unsigned: cursor.u8()?,
    };

    let key_count = cursor.shortvec_len()?;
    let mut static_keys = Vec::with_capacity(key_count.min(256));
    for _ in 0..key_count {
        static_keys.push(cursor.pubkey()?);
    }

    if (header.num_required_signatures as usize) > static_keys.len() {
        return Err("header requires more signatures than there are accounts".to_string());
    }
    let readonly = header.num_readonly_signed as usize + header.num_readonly_unsigned as usize;
    if readonly > static_keys.len() {
        return Err("header marks more accounts read-only than exist".to_string());
    }

    let recent_blockhash = cursor.pubkey()?;

    let ix_count = cursor.shortvec_len()?;
    let mut instructions = Vec::with_capacity(ix_count.min(256));
    for _ in 0..ix_count {
        let program_id_index = cursor.u8()?;
        let account_len = cursor.shortvec_len()?;
        let account_indexes = cursor.take(account_len)?.to_vec();
        let data_len = cursor.shortvec_len()?;
        let data = cursor.take(data_len)?.to_vec();
        instructions.push(CompiledInstruction {
            program_id_index,
            account_indexes,
            data,
        });
    }

    let lookups = if version == MessageVersion::V0 {
        let lookup_count = cursor.shortvec_len()?;
        let mut lookups = Vec::with_capacity(lookup_count.min(64));
        for _ in 0..lookup_count {
            let table = cursor.pubkey()?;
            let writable_len = cursor.shortvec_len()?;
            let writable_indexes = cursor.take(writable_len)?.to_vec();
            let readonly_len = cursor.shortvec_len()?;
            let readonly_indexes = cursor.take(readonly_len)?.to_vec();
            lookups.push(AddressTableLookup {
                table,
                writable_indexes,
                readonly_indexes,
            });
        }
        lookups
    } else {
        Vec::new()
    };

    // A program index pointing outside the static keys means the transaction
    // cannot be understood without resolving lookups, and a program that lives
    // in a lookup table is itself worth refusing.
    for ix in &instructions {
        if ix.program_id_index as usize >= static_keys.len() {
            return Err(format!(
                "instruction program index {} is outside the static key list",
                ix.program_id_index
            ));
        }
    }

    Ok(Message {
        version,
        header,
        static_keys,
        recent_blockhash,
        instructions,
        lookups,
    })
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Minimal base64 decoder, so the crate does not take a dependency for it.
pub fn base64_decode(input: &str) -> Result<Vec<u8>, String> {
    let mut lookup = [0xffu8; 256];
    for (i, c) in B64.iter().enumerate() {
        lookup[*c as usize] = i as u8;
    }

    let bytes: Vec<u8> = input.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
    let bytes: &[u8] = match bytes.iter().position(|b| *b == b'=') {
        Some(p) => &bytes[..p],
        None => &bytes,
    };

    let mut out = Vec::with_capacity(bytes.len() * 3 / 4);
    let mut accumulator: u32 = 0;
    let mut bits: u32 = 0;

    for byte in bytes {
        let value = lookup[*byte as usize];
        if value == 0xff {
            return Err(format!("invalid base64 character: {}", *byte as char));
        }
        accumulator = (accumulator << 6) | u32::from(value);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((accumulator >> bits) as u8);
        }
    }

    Ok(out)
}

/// Minimal base64 encoder, used for building test fixtures and request bodies.
pub fn base64_encode(input: &[u8]) -> String {
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(B64[(n >> 18) as usize & 63] as char);
        out.push(B64[(n >> 12) as usize & 63] as char);
        if chunk.len() > 1 {
            out.push(B64[(n >> 6) as usize & 63] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(B64[n as usize & 63] as char);
        } else {
            out.push('=');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Encode a compact-u16 length, for building fixtures.
    fn shortvec(mut n: usize) -> Vec<u8> {
        let mut out = Vec::new();
        loop {
            let mut byte = (n & 0x7f) as u8;
            n >>= 7;
            if n != 0 {
                byte |= 0x80;
            }
            out.push(byte);
            if n == 0 {
                return out;
            }
        }
    }

    fn key(seed: u8) -> Pubkey {
        Pubkey([seed; 32])
    }

    /// A legacy transfer: payer, recipient, system program.
    fn legacy_fixture() -> Vec<u8> {
        let mut msg = Vec::new();
        msg.extend([1, 0, 1]); // 1 signer, 0 readonly signed, 1 readonly unsigned
        msg.extend(shortvec(3));
        msg.extend(key(1).0);
        msg.extend(key(2).0);
        msg.extend(key(9).0); // system program
        msg.extend(key(0xbb).0); // blockhash
        msg.extend(shortvec(1));
        msg.push(2); // program index -> system program
        msg.extend(shortvec(2));
        msg.extend([0u8, 1u8]);
        msg.extend(shortvec(4));
        msg.extend([2, 0, 0, 0]);

        let mut tx = Vec::new();
        tx.extend(shortvec(1));
        tx.extend([7u8; 64]);
        tx.extend(msg);
        tx
    }

    fn v0_fixture() -> Vec<u8> {
        let mut msg = Vec::new();
        msg.push(0x80); // version 0
        msg.extend([1, 0, 1]);
        msg.extend(shortvec(2));
        msg.extend(key(1).0);
        msg.extend(key(9).0);
        msg.extend(key(0xbb).0);
        msg.extend(shortvec(1));
        msg.push(1);
        msg.extend(shortvec(1));
        msg.push(0);
        msg.extend(shortvec(0));
        msg.extend(shortvec(1)); // one lookup
        msg.extend(key(0x55).0);
        msg.extend(shortvec(2));
        msg.extend([3u8, 4u8]);
        msg.extend(shortvec(1));
        msg.extend([5u8]);

        let mut tx = Vec::new();
        tx.extend(shortvec(1));
        tx.extend([7u8; 64]);
        tx.extend(msg);
        tx
    }

    #[test]
    fn decodes_a_legacy_transaction() {
        let tx = decode_transaction(&legacy_fixture()).expect("legacy fixture decodes");
        assert_eq!(tx.message.version, MessageVersion::Legacy);
        assert_eq!(tx.signatures.len(), 1);
        assert_eq!(tx.message.static_keys.len(), 3);
        assert_eq!(tx.message.instructions.len(), 1);
        assert_eq!(tx.message.instructions[0].data, vec![2, 0, 0, 0]);
        assert_eq!(tx.message.fee_payer(), Some(key(1)));
        assert!(!tx.message.needs_lookup_resolution());
    }

    #[test]
    fn decodes_a_v0_transaction_with_lookups() {
        let tx = decode_transaction(&v0_fixture()).expect("v0 fixture decodes");
        assert_eq!(tx.message.version, MessageVersion::V0);
        assert_eq!(tx.message.lookups.len(), 1);
        assert_eq!(tx.message.lookups[0].writable_indexes, vec![3, 4]);
        assert_eq!(tx.message.lookups[0].readonly_indexes, vec![5]);
        assert!(tx.message.needs_lookup_resolution());
        // Two static plus three loaded from the table.
        assert_eq!(tx.message.total_accounts(), 5);
    }

    #[test]
    fn writability_follows_the_header() {
        let tx = decode_transaction(&legacy_fixture()).unwrap();
        assert!(tx.message.is_writable(0), "fee payer is writable");
        assert!(tx.message.is_writable(1), "recipient is writable");
        assert!(!tx.message.is_writable(2), "program is read-only");
        assert!(tx.message.is_signer(0));
        assert!(!tx.message.is_signer(1));
    }

    #[test]
    fn loaded_writable_accounts_come_before_loaded_readonly() {
        let tx = decode_transaction(&v0_fixture()).unwrap();
        // statics 0..2, then two writable loaded, then one readonly loaded.
        assert!(tx.message.is_writable(2));
        assert!(tx.message.is_writable(3));
        assert!(!tx.message.is_writable(4));
    }

    #[test]
    fn truncated_input_is_an_error_not_a_partial_message() {
        let full = legacy_fixture();
        for cut in [1, 10, 40, full.len() - 1] {
            assert!(
                decode_transaction(&full[..cut]).is_err(),
                "truncation at {cut} must fail"
            );
        }
    }

    #[test]
    fn trailing_bytes_are_rejected() {
        let mut tx = legacy_fixture();
        tx.push(0);
        assert!(decode_transaction(&tx).is_err());
    }

    #[test]
    fn program_index_outside_the_key_list_is_rejected() {
        let mut msg = Vec::new();
        msg.extend([1, 0, 0]);
        msg.extend(shortvec(1));
        msg.extend(key(1).0);
        msg.extend(key(0xbb).0);
        msg.extend(shortvec(1));
        msg.push(9); // no such account
        msg.extend(shortvec(0));
        msg.extend(shortvec(0));

        let mut tx = Vec::new();
        tx.extend(shortvec(0));
        tx.extend(msg);

        let err = decode_transaction(&tx).expect_err("dangling program index must fail");
        assert!(err.contains("outside the static key list"));
    }

    #[test]
    fn an_inconsistent_header_is_rejected() {
        let mut msg = Vec::new();
        msg.extend([5, 0, 0]); // five signers, one account
        msg.extend(shortvec(1));
        msg.extend(key(1).0);
        msg.extend(key(0xbb).0);
        msg.extend(shortvec(0));

        let mut tx = Vec::new();
        tx.extend(shortvec(0));
        tx.extend(msg);

        assert!(decode_transaction(&tx).is_err());
    }

    #[test]
    fn unsupported_versions_are_rejected() {
        let mut tx = v0_fixture();
        let prefix = 1 + 64; // past the signature shortvec and the signature
        tx[prefix] = 0x81; // version 1
        let err = decode_transaction(&tx).expect_err("v1 must not silently decode");
        assert!(err.contains("unsupported message version"));
    }

    #[test]
    fn base64_round_trips() {
        for case in [
            &b""[..],
            &b"f"[..],
            &b"fo"[..],
            &b"foo"[..],
            &b"foob"[..],
            &b"fooba"[..],
            &b"foobar"[..],
        ] {
            let encoded = base64_encode(case);
            let decoded = base64_decode(&encoded).expect("round trip");
            assert_eq!(decoded, case, "failed for {case:?}");
        }
    }

    #[test]
    fn base64_rejects_junk() {
        assert!(base64_decode("not valid!!").is_err());
    }

    #[test]
    fn a_transaction_survives_base64() {
        let encoded = base64_encode(&legacy_fixture());
        let tx = decode_transaction_base64(&encoded).expect("base64 transaction decodes");
        assert_eq!(tx.message.static_keys.len(), 3);
    }

    #[test]
    fn pubkeys_round_trip_through_base58() {
        let k = key(3);
        let s = k.to_base58();
        assert_eq!(Pubkey::from_base58(&s).unwrap(), k);
        assert!(Pubkey::from_base58("0OIl").is_err());
    }

    #[test]
    fn program_ids_are_deduplicated() {
        let tx = decode_transaction(&legacy_fixture()).unwrap();
        assert_eq!(tx.message.program_ids(), vec![key(9)]);
    }
}
