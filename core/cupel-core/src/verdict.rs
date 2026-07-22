//! Verdict rendering for `tx-preflight`.
//!
//! Pure: no wasm dependency, no network, no clock. Everything here is decided
//! by the caller and rendered deterministically, so the whole module is
//! host-testable with a plain `cargo test`.
//!
//! See `design/verdict-spec.md` for the reasoning behind the format. The four
//! rules that matter: fail closed under one word, print negatives explicitly,
//! never claim safety, fixed field order ranked by blast radius.

use std::fmt::Write as _;

use sha2::{Digest, Sha256};

/// A token amount in base units. Money never touches a float.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Amount {
    pub symbol: String,
    pub base_units: u128,
    pub decimals: u8,
}

impl Amount {
    pub fn new(symbol: impl Into<String>, base_units: u128, decimals: u8) -> Self {
        Self {
            symbol: symbol.into(),
            base_units,
            decimals,
        }
    }

    /// Render as a grouped decimal string: `2,140.00`, `0.000005`.
    ///
    /// Trailing zeros are trimmed but at least two decimal places are kept, so
    /// a fee reads `0.000005` while a payment reads `25.00`.
    pub fn display(&self) -> String {
        let divisor = 10u128.pow(u32::from(self.decimals));
        let whole = self.base_units / divisor;
        let frac = self.base_units % divisor;

        let mut fraction = format!("{frac:0width$}", width = self.decimals as usize);
        while fraction.len() > 2 && fraction.ends_with('0') {
            fraction.pop();
        }
        if fraction.is_empty() {
            fraction.push_str("00");
        }

        format!("{}.{}", group_thousands(whole), fraction)
    }
}

fn group_thousands(value: u128) -> String {
    let digits = value.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

/// Shorten an address for display: first eight, ellipsis, last eight.
pub fn short_addr(addr: &str) -> String {
    if addr.chars().count() <= 20 {
        return addr.to_string();
    }
    let chars: Vec<char> = addr.chars().collect();
    let head: String = chars[..8].iter().collect();
    let tail: String = chars[chars.len() - 8..].iter().collect();
    format!("{head}…{tail}")
}

/// A power handed to someone else. Persistent, unbounded, and worse than any
/// single transfer, which is why it is ranked above amounts in the output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GrantKind {
    Delegate,
    CloseAuthority,
    FreezeAuthority,
    PermanentDelegate,
    OwnerChange,
}

impl GrantKind {
    fn describe(self) -> &'static str {
        match self {
            GrantKind::Delegate => "delegate over",
            GrantKind::CloseAuthority => "close authority over",
            GrantKind::FreezeAuthority => "freeze authority over",
            GrantKind::PermanentDelegate => "permanent delegate over",
            GrantKind::OwnerChange => "new owner of",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Grant {
    pub kind: GrantKind,
    /// Human label for the affected account, e.g. `"your USDC account"`.
    pub account_label: String,
    pub grantee: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Counterparty {
    pub address: String,
    /// True when the address resolved to something the operator configured or
    /// has transacted with before.
    pub known: bool,
}

/// What the simulation says will actually happen.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Effect {
    pub outflows: Vec<Amount>,
    pub inflows: Vec<Amount>,
    pub grants: Vec<Grant>,
    pub accounts_closed: usize,
    pub fee_lamports: u64,
    pub counterparty: Option<Counterparty>,
    pub reference: Option<String>,
    pub unknown_programs: Vec<String>,
}

impl Effect {
    /// Truncated SHA-256 over the normalised effect.
    ///
    /// Deliberately **not** over the transaction bytes: a rebuild after
    /// blockhash expiry changes the bytes but not the outcome, and approval
    /// binds to the outcome. Identical digest means the human already approved
    /// exactly this.
    pub fn digest(&self) -> String {
        let mut hasher = Sha256::new();
        for a in &self.outflows {
            hasher.update(format!("out:{}:{}:{};", a.symbol, a.base_units, a.decimals));
        }
        for a in &self.inflows {
            hasher.update(format!("in:{}:{}:{};", a.symbol, a.base_units, a.decimals));
        }
        for g in &self.grants {
            hasher.update(format!("grant:{:?}:{};", g.kind, g.grantee));
        }
        hasher.update(format!("closed:{};", self.accounts_closed));
        if let Some(cp) = &self.counterparty {
            hasher.update(format!("to:{};", cp.address));
        }
        if let Some(r) = &self.reference {
            hasher.update(format!("ref:{r};"));
        }
        let out = hasher.finalize();
        out.iter().take(4).map(|b| format!("{b:02x}")).collect()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Pass,
    Warn,
    Fail,
}

impl Verdict {
    fn word(self) -> &'static str {
        match self {
            Verdict::Pass => "PASS",
            Verdict::Warn => "WARN",
            Verdict::Fail => "FAIL",
        }
    }
}

/// A rendered decision. Build it through the constructors so the verdict can
/// never disagree with the violations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    verdict: Verdict,
    reason: String,
    effect: Option<Effect>,
    violations: Vec<String>,
    advisories: Vec<String>,
    /// Per-symbol caps, shown inline beside the amount they constrain.
    caps: Vec<(String, String)>,
}

impl Report {
    /// A transaction that was successfully simulated and checked.
    pub fn verified(
        effect: Effect,
        violations: Vec<String>,
        advisories: Vec<String>,
        caps: Vec<(String, String)>,
    ) -> Self {
        let verdict = if violations.is_empty() {
            if advisories.is_empty() {
                Verdict::Pass
            } else {
                Verdict::Warn
            }
        } else {
            Verdict::Fail
        };

        let reason = match verdict {
            Verdict::Pass => "within envelope".to_string(),
            Verdict::Warn => advisories
                .first()
                .cloned()
                .unwrap_or_else(|| "advisory".to_string()),
            Verdict::Fail => summarise_violations(&violations),
        };

        Self {
            verdict,
            reason,
            effect: Some(effect),
            violations,
            advisories,
            caps,
        }
    }

    /// A transaction that could not be checked at all.
    ///
    /// Same verdict word as a verified drain. An unverifiable transaction and a
    /// malicious one are equally unsignable, and a softer word here is exactly
    /// the crack a verifier gets talked through.
    pub fn unverifiable(reason: impl Into<String>) -> Self {
        Self {
            verdict: Verdict::Fail,
            reason: "could not verify".to_string(),
            effect: None,
            violations: vec![reason.into()],
            advisories: Vec::new(),
            caps: Vec::new(),
        }
    }

    pub fn verdict(&self) -> Verdict {
        self.verdict
    }

    pub fn is_signable(&self) -> bool {
        !matches!(self.verdict, Verdict::Fail)
    }

    pub fn render(&self) -> String {
        let mut out = String::with_capacity(256);
        let _ = writeln!(out, "{} · {}", self.verdict.word(), self.reason);
        out.push('\n');

        let Some(effect) = &self.effect else {
            for v in &self.violations {
                let _ = writeln!(out, "{v}");
            }
            out.push('\n');
            out.push_str("Nothing verified. Do not sign.");
            return out;
        };

        for amount in &effect.outflows {
            let cap = self
                .caps
                .iter()
                .find(|(sym, _)| *sym == amount.symbol)
                .map(|(_, limit)| format!("  (cap {limit})"))
                .unwrap_or_default();
            let _ = writeln!(
                out,
                "Pay        {} {}{}",
                amount.display(),
                amount.symbol,
                cap
            );
        }

        for amount in &effect.inflows {
            let _ = writeln!(out, "Receive    {} {}", amount.display(), amount.symbol);
        }

        // Always printed. "none" is a claim that the check ran; a missing line
        // is only an ambiguity.
        if effect.grants.is_empty() {
            out.push_str("Grants     none\n");
        } else {
            for grant in &effect.grants {
                let _ = writeln!(
                    out,
                    "Grants     {} {}",
                    grant.kind.describe(),
                    grant.account_label
                );
                let _ = writeln!(out, "           → {}", short_addr(&grant.grantee));
            }
        }

        if effect.accounts_closed > 0 {
            let plural = if effect.accounts_closed == 1 { "" } else { "s" };
            let _ = writeln!(
                out,
                "Closes     {} account{}",
                effect.accounts_closed, plural
            );
        }

        let fee = Amount::new("SOL", u128::from(effect.fee_lamports), 9);
        let _ = writeln!(out, "Fee        {} SOL", fee.display());

        if let Some(cp) = &effect.counterparty {
            let tag = if cp.known { "" } else { "  unknown" };
            let _ = writeln!(out, "To         {}{}", short_addr(&cp.address), tag);
        }

        if self.is_signable() {
            if let Some(reference) = &effect.reference {
                let _ = writeln!(out, "Ref        {}", short_addr(reference));
            }
            let _ = writeln!(out, "EFX        {}", effect.digest());
        }

        out.push('\n');
        out.push_str(&self.footer());
        out
    }

    fn footer(&self) -> String {
        match self.verdict {
            // Never "safe to sign". Cupel reports what it checked against the
            // operator's limits; it does not bless a transaction.
            Verdict::Pass => "Effects match your limits.".to_string(),
            Verdict::Warn => self
                .advisories
                .first()
                .cloned()
                .unwrap_or_else(|| "Advisory only.".to_string()),
            Verdict::Fail => {
                let n = self.violations.len();
                let plural = if n == 1 { "" } else { "s" };
                format!("{n} violation{plural}. Nothing signed.")
            }
        }
    }
}

fn summarise_violations(violations: &[String]) -> String {
    let mut parts: Vec<&str> = Vec::new();
    if violations.iter().any(|v| v.contains("cap")) {
        parts.push("envelope exceeded");
    }
    if violations.iter().any(|v| v.contains("authority") || v.contains("delegate")) {
        parts.push("authority granted");
    }
    if parts.is_empty() {
        return "policy violation".to_string();
    }
    parts.join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usdc(units: u128) -> Amount {
        Amount::new("USDC", units, 6)
    }

    fn clean_effect() -> Effect {
        Effect {
            outflows: vec![usdc(25_000_000)],
            fee_lamports: 5_000,
            counterparty: Some(Counterparty {
                address: "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU".to_string(),
                known: true,
            }),
            reference: Some("4Kf9x2Qr8mLpTn3VbW5yH6cJd1eR7gS2aZ8mLpTn3VbW".to_string()),
            ..Default::default()
        }
    }

    #[test]
    fn amounts_group_and_trim() {
        assert_eq!(usdc(25_000_000).display(), "25.00");
        assert_eq!(usdc(2_140_000_000).display(), "2,140.00");
        assert_eq!(Amount::new("SOL", 5_000, 9).display(), "0.000005");
        assert_eq!(usdc(0).display(), "0.00");
    }

    #[test]
    fn short_addresses_keep_both_ends() {
        let s = short_addr("7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU");
        assert!(s.starts_with("7xKXtg2C"));
        assert!(s.ends_with("JosgAsU"));
        assert!(s.contains('…'));
    }

    #[test]
    fn short_addr_leaves_short_input_alone() {
        assert_eq!(short_addr("abc"), "abc");
    }

    #[test]
    fn clean_transaction_passes() {
        let report = Report::verified(clean_effect(), vec![], vec![], vec![]);
        assert_eq!(report.verdict(), Verdict::Pass);
        assert!(report.is_signable());

        let out = report.render();
        assert!(out.starts_with("PASS · within envelope"));
        assert!(out.contains("Pay        25.00 USDC"));
        assert!(out.contains("EFX        "));
        assert!(out.ends_with("Effects match your limits."));
    }

    #[test]
    fn grants_none_is_printed_explicitly() {
        let out = Report::verified(clean_effect(), vec![], vec![], vec![]).render();
        assert!(
            out.contains("Grants     none"),
            "an empty grants list must still print a line"
        );
    }

    #[test]
    fn never_claims_safety() {
        let out = Report::verified(clean_effect(), vec![], vec![], vec![]).render();
        let lowered = out.to_lowercase();
        assert!(!lowered.contains("safe"));
    }

    #[test]
    fn injected_drain_fails_closed() {
        let effect = Effect {
            outflows: vec![usdc(2_140_000_000)],
            grants: vec![Grant {
                kind: GrantKind::Delegate,
                account_label: "your USDC account".to_string(),
                grantee: "9xQmR4vK3nBwZ7pLd2sT8yH5jC1fN6gX4mKpQr9wZ4mK".to_string(),
            }],
            fee_lamports: 5_000,
            counterparty: Some(Counterparty {
                address: "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU".to_string(),
                known: false,
            }),
            ..Default::default()
        };
        let report = Report::verified(
            effect,
            vec![
                "outflow 2140.00 USDC exceeds cap 50.00".to_string(),
                "delegate authority granted".to_string(),
            ],
            vec![],
            vec![("USDC".to_string(), "50.00".to_string())],
        );

        assert_eq!(report.verdict(), Verdict::Fail);
        assert!(!report.is_signable());

        let out = report.render();
        assert!(out.starts_with("FAIL · envelope exceeded, authority granted"));
        assert!(out.contains("(cap 50.00)"));
        assert!(out.contains("delegate over your USDC account"));
        assert!(out.contains("unknown"));
        assert!(out.ends_with("2 violations. Nothing signed."));
        // A failed report must never hand over a reusable approval token.
        assert!(!out.contains("EFX"));
    }

    #[test]
    fn unverifiable_uses_the_same_verdict_word_as_a_drain() {
        let report = Report::unverifiable("Simulation unavailable: RPC returned 429.");
        assert_eq!(report.verdict(), Verdict::Fail);

        let out = report.render();
        assert!(out.starts_with("FAIL · could not verify"));
        assert!(out.ends_with("Nothing verified. Do not sign."));
    }

    #[test]
    fn advisories_produce_warn_not_fail() {
        let report = Report::verified(
            clean_effect(),
            vec![],
            vec!["Calls 1 program Cupel does not recognise.".to_string()],
            vec![],
        );
        assert_eq!(report.verdict(), Verdict::Warn);
        assert!(report.is_signable());
    }

    #[test]
    fn digest_ignores_bytes_and_tracks_outcome() {
        let a = clean_effect();
        let mut b = clean_effect();
        assert_eq!(a.digest(), b.digest(), "same outcome, same digest");

        b.outflows = vec![usdc(25_000_001)];
        assert_ne!(a.digest(), b.digest(), "changed outcome must change digest");
    }

    #[test]
    fn every_verdict_stays_inside_the_token_budget() {
        // ~4 chars per token; the budget is 160 tokens.
        for out in [
            Report::verified(clean_effect(), vec![], vec![], vec![]).render(),
            Report::unverifiable("Simulation unavailable: RPC returned 429.").render(),
        ] {
            assert!(out.len() < 640, "verdict too long: {} chars", out.len());
        }
    }
}
