//! SPL Token and Token-2022 account layouts.
//!
//! This is where a drain gets caught. A transfer moves what it says it moves;
//! a **delegate** moves whatever it likes, whenever it likes, until revoked.
//! Same for a close authority, a freeze authority, and Token-2022's permanent
//! delegate — which cannot be revoked at all. Those fields live here, and
//! diffing them across a simulation is what turns "the model says this is a
//! refund" into "this grants a stranger standing access to your balance".
//!
//! ## Layouts
//!
//! ```text
//! account (165 bytes)          mint (82 bytes)
//!   0..32   mint                 0..4    mint_authority tag
//!  32..64   owner                4..36   mint_authority
//!  64..72   amount  u64le       36..44   supply u64le
//!  72..76   delegate tag        44..45   decimals
//!  76..108  delegate            45..46   is_initialized
//! 108..109  state               46..50   freeze_authority tag
//! 109..113  is_native tag       50..82   freeze_authority
//! 113..121  is_native
//! 121..129  delegated_amount
//! 129..133  close_authority tag
//! 133..165  close_authority
//! ```
//!
//! A Token-2022 account is the same 165 bytes, padded if it is a mint, then a
//! one-byte account type at offset 165, then TLV extensions: `type:u16le`,
//! `length:u16le`, `data`.
//!
//! `COption<Pubkey>` on the wire is a four-byte little-endian tag followed by
//! the 32-byte key, present regardless. Tag values other than 0 or 1 are
//! rejected rather than guessed at.

use crate::message::Pubkey;

/// `TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA`
pub const TOKEN_PROGRAM_ID: &str = "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA";
/// `TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb`
pub const TOKEN_2022_PROGRAM_ID: &str = "TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb";

/// Size of the base account and mint layouts.
pub const ACCOUNT_LEN: usize = 165;
pub const MINT_LEN: usize = 82;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccountState {
    Uninitialized,
    Initialized,
    Frozen,
}

/// A token account as it exists on chain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenAccount {
    pub mint: Pubkey,
    pub owner: Pubkey,
    pub amount: u64,
    /// Someone other than the owner who may move this balance.
    pub delegate: Option<Pubkey>,
    pub delegated_amount: u64,
    pub state: AccountState,
    pub close_authority: Option<Pubkey>,
    /// Extensions present when this is a Token-2022 account.
    pub extensions: Vec<Extension>,
}

impl TokenAccount {
    /// Every power this account hands to someone other than its owner.
    ///
    /// The check a human actually needs: who, besides me, can touch this.
    pub fn outstanding_authorities(&self) -> Vec<(&'static str, Pubkey)> {
        let mut out = Vec::new();
        if let Some(d) = self.delegate {
            out.push(("delegate", d));
        }
        if let Some(c) = self.close_authority {
            out.push(("close authority", c));
        }
        out
    }

    pub fn is_frozen(&self) -> bool {
        self.state == AccountState::Frozen
    }

    pub fn decode(data: &[u8]) -> Result<Self, String> {
        if data.len() < ACCOUNT_LEN {
            return Err(format!(
                "token account is {} bytes, expected at least {ACCOUNT_LEN}",
                data.len()
            ));
        }

        let mint = pubkey_at(data, 0)?;
        let owner = pubkey_at(data, 32)?;
        let amount = u64_at(data, 64)?;
        let delegate = coption_pubkey(data, 72)?;
        let state = match data[108] {
            0 => AccountState::Uninitialized,
            1 => AccountState::Initialized,
            2 => AccountState::Frozen,
            other => return Err(format!("unknown token account state: {other}")),
        };
        let delegated_amount = u64_at(data, 121)?;
        let close_authority = coption_pubkey(data, 129)?;

        // A delegate with no delegated amount is inert but still present, and a
        // delegated amount with no delegate is incoherent. Refuse the latter.
        if delegate.is_none() && delegated_amount > 0 {
            return Err("delegated amount set with no delegate".to_string());
        }

        let extensions = if data.len() > ACCOUNT_LEN {
            decode_extensions(data)?
        } else {
            Vec::new()
        };

        Ok(Self {
            mint,
            owner,
            amount,
            delegate,
            delegated_amount,
            state,
            close_authority,
            extensions,
        })
    }
}

/// A mint as it exists on chain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mint {
    pub mint_authority: Option<Pubkey>,
    pub supply: u64,
    pub decimals: u8,
    pub is_initialized: bool,
    pub freeze_authority: Option<Pubkey>,
    pub extensions: Vec<Extension>,
}

impl Mint {
    pub fn decode(data: &[u8]) -> Result<Self, String> {
        if data.len() < MINT_LEN {
            return Err(format!(
                "mint is {} bytes, expected at least {MINT_LEN}",
                data.len()
            ));
        }

        let mint_authority = coption_pubkey(data, 0)?;
        let supply = u64_at(data, 36)?;
        let decimals = data[44];
        let is_initialized = match data[45] {
            0 => false,
            1 => true,
            other => return Err(format!("mint is_initialized is {other}, expected 0 or 1")),
        };
        let freeze_authority = coption_pubkey(data, 46)?;

        if decimals > 18 {
            return Err(format!("implausible mint decimals: {decimals}"));
        }

        let extensions = if data.len() > ACCOUNT_LEN {
            decode_extensions(data)?
        } else {
            Vec::new()
        };

        Ok(Self {
            mint_authority,
            supply,
            decimals,
            is_initialized,
            freeze_authority,
            extensions,
        })
    }

    /// Extensions that change what a transfer of this mint actually does.
    ///
    /// A human reading "send 100 tokens" assumes 100 arrive and that nobody
    /// else can claw them back. Each of these breaks that assumption.
    pub fn hazards(&self) -> Vec<&'static str> {
        self.extensions
            .iter()
            .filter_map(|e| match e.kind {
                ExtensionKind::PermanentDelegate => Some("permanent delegate (cannot be revoked)"),
                ExtensionKind::TransferHook => Some("transfer hook runs third-party code"),
                ExtensionKind::TransferFeeConfig => Some("transfer fee is deducted in flight"),
                ExtensionKind::MintCloseAuthority => Some("mint can be closed"),
                ExtensionKind::DefaultAccountState => Some("new accounts may start frozen"),
                ExtensionKind::NonTransferable => Some("tokens cannot be transferred"),
                ExtensionKind::ConfidentialTransferMint => Some("balances may be confidential"),
                _ => None,
            })
            .collect()
    }
}

/// A Token-2022 TLV extension.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Extension {
    pub kind: ExtensionKind,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtensionKind {
    Uninitialized,
    TransferFeeConfig,
    TransferFeeAmount,
    MintCloseAuthority,
    ConfidentialTransferMint,
    ConfidentialTransferAccount,
    DefaultAccountState,
    ImmutableOwner,
    MemoTransfer,
    NonTransferable,
    InterestBearingConfig,
    CpiGuard,
    PermanentDelegate,
    NonTransferableAccount,
    TransferHook,
    TransferHookAccount,
    ConfidentialTransferFeeConfig,
    ConfidentialTransferFeeAmount,
    MetadataPointer,
    TokenMetadata,
    GroupPointer,
    TokenGroup,
    GroupMemberPointer,
    TokenGroupMember,
    /// A type this build does not know about. Never treated as benign.
    Unknown(u16),
}

impl ExtensionKind {
    fn from_code(code: u16) -> Self {
        match code {
            0 => Self::Uninitialized,
            1 => Self::TransferFeeConfig,
            2 => Self::TransferFeeAmount,
            3 => Self::MintCloseAuthority,
            4 => Self::ConfidentialTransferMint,
            5 => Self::ConfidentialTransferAccount,
            6 => Self::DefaultAccountState,
            7 => Self::ImmutableOwner,
            8 => Self::MemoTransfer,
            9 => Self::NonTransferable,
            10 => Self::InterestBearingConfig,
            11 => Self::CpiGuard,
            12 => Self::PermanentDelegate,
            13 => Self::NonTransferableAccount,
            14 => Self::TransferHook,
            15 => Self::TransferHookAccount,
            16 => Self::ConfidentialTransferFeeConfig,
            17 => Self::ConfidentialTransferFeeAmount,
            18 => Self::MetadataPointer,
            19 => Self::TokenMetadata,
            20 => Self::GroupPointer,
            21 => Self::TokenGroup,
            22 => Self::GroupMemberPointer,
            23 => Self::TokenGroupMember,
            other => Self::Unknown(other),
        }
    }

    /// Whether this build understands the extension well enough to judge it.
    ///
    /// An unrecognised extension is reported, not ignored. Token-2022 is
    /// extensible by design, and "I do not know what this does" is a finding.
    pub fn is_recognised(self) -> bool {
        !matches!(self, Self::Unknown(_))
    }
}

/// Read the TLV extension list that follows the base layout.
fn decode_extensions(data: &[u8]) -> Result<Vec<Extension>, String> {
    // Byte 165 is the account type discriminator; extensions start after it.
    let mut pos = ACCOUNT_LEN + 1;
    if data.len() <= pos {
        return Ok(Vec::new());
    }

    let mut extensions = Vec::new();
    while pos + 4 <= data.len() {
        let kind_code = u16::from_le_bytes([data[pos], data[pos + 1]]);
        let length = u16::from_le_bytes([data[pos + 2], data[pos + 3]]) as usize;
        pos += 4;

        // A zero type with zero length is the padding that ends the list.
        if kind_code == 0 && length == 0 {
            break;
        }
        if pos + length > data.len() {
            return Err(format!(
                "extension {kind_code} claims {length} bytes but only {} remain",
                data.len() - pos
            ));
        }

        extensions.push(Extension {
            kind: ExtensionKind::from_code(kind_code),
            data: data[pos..pos + length].to_vec(),
        });
        pos += length;
    }

    Ok(extensions)
}

fn pubkey_at(data: &[u8], offset: usize) -> Result<Pubkey, String> {
    let slice = data
        .get(offset..offset + 32)
        .ok_or_else(|| format!("truncated at offset {offset}"))?;
    let mut key = [0u8; 32];
    key.copy_from_slice(slice);
    Ok(Pubkey(key))
}

fn u64_at(data: &[u8], offset: usize) -> Result<u64, String> {
    let slice = data
        .get(offset..offset + 8)
        .ok_or_else(|| format!("truncated at offset {offset}"))?;
    let mut buf = [0u8; 8];
    buf.copy_from_slice(slice);
    Ok(u64::from_le_bytes(buf))
}

/// A four-byte tag followed by a key that is present either way.
fn coption_pubkey(data: &[u8], offset: usize) -> Result<Option<Pubkey>, String> {
    let tag = data
        .get(offset..offset + 4)
        .ok_or_else(|| format!("truncated COption tag at {offset}"))?;
    let tag = u32::from_le_bytes([tag[0], tag[1], tag[2], tag[3]]);
    match tag {
        0 => Ok(None),
        1 => Ok(Some(pubkey_at(data, offset + 4)?)),
        other => Err(format!("COption tag at {offset} is {other}, expected 0 or 1")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(seed: u8) -> Pubkey {
        Pubkey([seed; 32])
    }

    struct AccountBuilder {
        data: Vec<u8>,
    }

    impl AccountBuilder {
        fn new() -> Self {
            let mut data = vec![0u8; ACCOUNT_LEN];
            data[0..32].copy_from_slice(&key(1).0); // mint
            data[32..64].copy_from_slice(&key(2).0); // owner
            data[64..72].copy_from_slice(&25_000_000u64.to_le_bytes());
            data[108] = 1; // initialized
            Self { data }
        }

        fn delegate(mut self, who: Pubkey, amount: u64) -> Self {
            self.data[72..76].copy_from_slice(&1u32.to_le_bytes());
            self.data[76..108].copy_from_slice(&who.0);
            self.data[121..129].copy_from_slice(&amount.to_le_bytes());
            self
        }

        fn close_authority(mut self, who: Pubkey) -> Self {
            self.data[129..133].copy_from_slice(&1u32.to_le_bytes());
            self.data[133..165].copy_from_slice(&who.0);
            self
        }

        fn frozen(mut self) -> Self {
            self.data[108] = 2;
            self
        }

        fn with_extension(mut self, code: u16, payload: &[u8]) -> Self {
            if self.data.len() == ACCOUNT_LEN {
                self.data.push(2); // account type: Account
            }
            self.data.extend(code.to_le_bytes());
            self.data.extend((payload.len() as u16).to_le_bytes());
            self.data.extend(payload);
            self
        }

        fn build(self) -> Vec<u8> {
            self.data
        }
    }

    fn mint_bytes(decimals: u8) -> Vec<u8> {
        let mut data = vec![0u8; MINT_LEN];
        data[0..4].copy_from_slice(&1u32.to_le_bytes());
        data[4..36].copy_from_slice(&key(7).0);
        data[36..44].copy_from_slice(&1_000_000u64.to_le_bytes());
        data[44] = decimals;
        data[45] = 1;
        data
    }

    #[test]
    fn decodes_a_plain_token_account() {
        let acct = TokenAccount::decode(&AccountBuilder::new().build()).expect("decodes");
        assert_eq!(acct.mint, key(1));
        assert_eq!(acct.owner, key(2));
        assert_eq!(acct.amount, 25_000_000);
        assert_eq!(acct.delegate, None);
        assert_eq!(acct.state, AccountState::Initialized);
        assert!(acct.outstanding_authorities().is_empty());
    }

    #[test]
    fn catches_a_delegate() {
        let data = AccountBuilder::new().delegate(key(0x99), u64::MAX).build();
        let acct = TokenAccount::decode(&data).expect("decodes");

        assert_eq!(acct.delegate, Some(key(0x99)));
        assert_eq!(acct.delegated_amount, u64::MAX);

        let authorities = acct.outstanding_authorities();
        assert_eq!(authorities.len(), 1);
        assert_eq!(authorities[0].0, "delegate");
        assert_eq!(authorities[0].1, key(0x99));
    }

    #[test]
    fn catches_a_close_authority() {
        let data = AccountBuilder::new().close_authority(key(0x88)).build();
        let acct = TokenAccount::decode(&data).unwrap();
        assert_eq!(acct.close_authority, Some(key(0x88)));
        assert_eq!(acct.outstanding_authorities()[0].0, "close authority");
    }

    #[test]
    fn reports_both_authorities_at_once() {
        let data = AccountBuilder::new()
            .delegate(key(0x99), 1)
            .close_authority(key(0x88))
            .build();
        let acct = TokenAccount::decode(&data).unwrap();
        assert_eq!(acct.outstanding_authorities().len(), 2);
    }

    #[test]
    fn reads_frozen_state() {
        let acct = TokenAccount::decode(&AccountBuilder::new().frozen().build()).unwrap();
        assert!(acct.is_frozen());
    }

    #[test]
    fn rejects_an_incoherent_delegation() {
        let mut data = AccountBuilder::new().build();
        data[121..129].copy_from_slice(&500u64.to_le_bytes()); // amount, no delegate
        assert!(TokenAccount::decode(&data).is_err());
    }

    #[test]
    fn rejects_a_bad_coption_tag() {
        let mut data = AccountBuilder::new().build();
        data[72..76].copy_from_slice(&7u32.to_le_bytes());
        let err = TokenAccount::decode(&data).expect_err("tag 7 is not 0 or 1");
        assert!(err.contains("expected 0 or 1"));
    }

    #[test]
    fn rejects_an_unknown_state() {
        let mut data = AccountBuilder::new().build();
        data[108] = 9;
        assert!(TokenAccount::decode(&data).is_err());
    }

    #[test]
    fn rejects_a_truncated_account() {
        assert!(TokenAccount::decode(&[0u8; 100]).is_err());
    }

    #[test]
    fn decodes_token_2022_extensions() {
        let data = AccountBuilder::new()
            .with_extension(7, &[]) // ImmutableOwner
            .with_extension(2, &[0u8; 8]) // TransferFeeAmount
            .build();
        let acct = TokenAccount::decode(&data).unwrap();

        assert_eq!(acct.extensions.len(), 2);
        assert_eq!(acct.extensions[0].kind, ExtensionKind::ImmutableOwner);
        assert_eq!(acct.extensions[1].kind, ExtensionKind::TransferFeeAmount);
    }

    #[test]
    fn an_unknown_extension_is_reported_not_ignored() {
        let data = AccountBuilder::new().with_extension(999, &[1, 2, 3]).build();
        let acct = TokenAccount::decode(&data).unwrap();
        assert_eq!(acct.extensions[0].kind, ExtensionKind::Unknown(999));
        assert!(!acct.extensions[0].kind.is_recognised());
    }

    #[test]
    fn a_lying_extension_length_is_rejected() {
        let mut data = AccountBuilder::new().build();
        data.push(2);
        data.extend(12u16.to_le_bytes()); // PermanentDelegate
        data.extend(200u16.to_le_bytes()); // claims 200 bytes
        data.extend([0u8; 4]); // provides 4
        let err = TokenAccount::decode(&data).expect_err("must not read past the buffer");
        assert!(err.contains("claims 200 bytes"));
    }

    #[test]
    fn decodes_a_mint() {
        let mint = Mint::decode(&mint_bytes(6)).expect("decodes");
        assert_eq!(mint.decimals, 6);
        assert_eq!(mint.supply, 1_000_000);
        assert_eq!(mint.mint_authority, Some(key(7)));
        assert_eq!(mint.freeze_authority, None);
        assert!(mint.is_initialized);
    }

    #[test]
    fn rejects_implausible_decimals() {
        let mut data = mint_bytes(6);
        data[44] = 60;
        assert!(Mint::decode(&data).is_err());
    }

    #[test]
    fn surfaces_permanent_delegate_as_a_hazard() {
        let mut data = mint_bytes(6);
        data.resize(ACCOUNT_LEN, 0);
        data.push(1); // account type: Mint
        data.extend(12u16.to_le_bytes()); // PermanentDelegate
        data.extend(32u16.to_le_bytes());
        data.extend(key(0x77).0);

        let mint = Mint::decode(&data).unwrap();
        let hazards = mint.hazards();
        assert!(hazards.iter().any(|h| h.contains("permanent delegate")));
    }

    #[test]
    fn surfaces_transfer_hooks_and_fees() {
        let mut data = mint_bytes(6);
        data.resize(ACCOUNT_LEN, 0);
        data.push(1);
        data.extend(14u16.to_le_bytes()); // TransferHook
        data.extend(0u16.to_le_bytes());
        data.extend(1u16.to_le_bytes()); // TransferFeeConfig
        data.extend(0u16.to_le_bytes());

        let hazards = Mint::decode(&data).unwrap().hazards();
        assert_eq!(hazards.len(), 2);
    }

    #[test]
    fn a_clean_mint_has_no_hazards() {
        let mint = Mint::decode(&mint_bytes(9)).unwrap();
        assert!(mint.hazards().is_empty());
    }
}
