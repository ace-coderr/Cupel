//! # cupel-core
//!
//! Solana transaction effect verification for agent runtimes.
//!
//! An agent that builds a transaction and asks a human to approve it has a
//! problem: the human approves a description the language model wrote. Poison
//! the model and the approval card reads "pay the supplier 25 USDC" while the
//! bytes underneath grant a delegate over the token account.
//!
//! `cupel-core` closes that gap. It decodes the transaction, simulates it,
//! diffs the resulting account states, and renders the **observed** effect
//! against limits the operator declared — never the effect the model claimed.
//!
//! ## Design
//!
//! - **No network.** Every RPC call goes through the [`Transport`] trait the
//!   caller supplies, so the whole crate is testable on the host with no wasm
//!   toolchain and no live endpoint.
//! - **No floats.** Money is `u128` base units with explicit decimals, all the
//!   way through.
//! - **Fail closed.** A decode failure, a simulation error, an unresolvable
//!   lookup table, or a malformed config value all produce a `FAIL` verdict.
//!   There is no permissive default anywhere in this crate.
//!
//! Built for `wasm32-wasip2` inside a WIT component, where `solana-sdk` and
//! `solana-client` will not compile. Dependencies are deliberately minimal.

#![forbid(unsafe_code)]

pub mod envelope;
pub mod message;
pub mod verdict;

pub use envelope::{Envelope, UnknownProgramPolicy};
pub use message::{
    decode_message, decode_transaction, decode_transaction_base64, AddressTableLookup,
    CompiledInstruction, Message, MessageHeader, MessageVersion, Pubkey, Transaction,
};
pub use verdict::{Amount, Counterparty, Effect, Grant, GrantKind, Report, Verdict};
pub mod token;
pub mod transport;
pub use token::{AccountState, Extension, ExtensionKind, Mint, TokenAccount};
pub use transport::{MockTransport, Transport};
pub mod solana_rpc;
pub use solana_rpc::{decode_lookup_table, AccountSnapshot, RpcClient, SimulationResult};
