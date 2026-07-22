//! The operator's declared limits, parsed from the host-injected `__config`.
//!
//! `__config` arrives as a flat `HashMap<String, String>` — no nesting, no
//! numbers — so every value is a string parsed defensively here. Two rules:
//!
//! 1. **A malformed value is an error, never a default.** Silently falling back
//!    to a permissive default is how a typo becomes an unlimited spend cap.
//! 2. **An absent envelope is an error.** If the operator declared no limits,
//!    there is nothing to verify against, and Cupel says so rather than
//!    passing everything.
//!
//! The host strips any caller-supplied `__config` before injecting the real
//! one, so nothing parsed here can be influenced by the model or by a poisoned
//! message. That guarantee is the foundation of Cupel's threat model.

use std::collections::HashMap;

/// Native SOL has nine decimal places.
pub const SOL_DECIMALS: u8 = 9;

/// What to do when a transaction touches a program Cupel does not recognise.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnknownProgramPolicy {
    /// Advisory only: produces a `WARN`.
    Warn,
    /// Blocking: produces a `FAIL`.
    Fail,
}

/// The operator's declared limits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Envelope {
    /// Ceiling on native SOL outflow, in lamports.
    pub max_sol_out: Option<u128>,
    /// Per-mint ceilings, held as decimal strings until the mint's decimals are
    /// known from chain.
    pub max_out_per_mint: Vec<(String, String)>,
    /// Mints the agent may move at all. Empty means no allowlist is enforced.
    pub mint_allowlist: Vec<String>,
    pub deny_authority_grants: bool,
    pub deny_account_close: bool,
    pub unknown_program_policy: UnknownProgramPolicy,
}

impl Envelope {
    /// Parse an envelope out of the injected config section.
    ///
    /// Returns `Err` when a value is malformed, and when no spending limit of
    /// any kind was declared.
    pub fn from_config(config: &HashMap<String, String>) -> Result<Self, String> {
        let max_sol_out = match config.get("max_sol_out").map(String::as_str) {
            None | Some("") => None,
            Some(raw) => Some(parse_decimal(raw, SOL_DECIMALS).map_err(|e| {
                format!("max_sol_out is not a valid amount: {e}")
            })?),
        };

        let max_out_per_mint = parse_mint_caps(config.get("max_out_per_mint"))?;
        let mint_allowlist = parse_list(config.get("mint_allowlist"));

        let deny_authority_grants = parse_bool(config.get("deny_authority_grants"), true)
            .map_err(|e| format!("deny_authority_grants {e}"))?;
        let deny_account_close = parse_bool(config.get("deny_account_close"), true)
            .map_err(|e| format!("deny_account_close {e}"))?;

        let unknown_program_policy = match config
            .get("unknown_program_policy")
            .map(String::as_str)
            .unwrap_or("warn")
        {
            "warn" => UnknownProgramPolicy::Warn,
            "fail" => UnknownProgramPolicy::Fail,
            other => {
                return Err(format!(
                    "unknown_program_policy must be 'warn' or 'fail', got '{other}'"
                ))
            }
        };

        if max_sol_out.is_none() && max_out_per_mint.is_empty() {
            return Err(
                "no spending limit declared: set max_sol_out or max_out_per_mint".to_string(),
            );
        }

        Ok(Self {
            max_sol_out,
            max_out_per_mint,
            mint_allowlist,
            deny_authority_grants,
            deny_account_close,
            unknown_program_policy,
        })
    }

    /// The declared cap for a mint, converted to base units now that the mint's
    /// decimals are known.
    pub fn cap_for(&self, mint: &str, decimals: u8) -> Option<Result<u128, String>> {
        self.max_out_per_mint
            .iter()
            .find(|(m, _)| m == mint)
            .map(|(_, raw)| parse_decimal(raw, decimals))
    }

    /// Whether a mint may be moved at all. An empty allowlist enforces nothing.
    pub fn mint_allowed(&self, mint: &str) -> bool {
        self.mint_allowlist.is_empty() || self.mint_allowlist.iter().any(|m| m == mint)
    }
}

/// Parse a decimal string into base units without ever touching a float.
///
/// `"25.00"` at six decimals is `25_000_000`. More fractional digits than the
/// mint supports is an error, not a rounding opportunity.
pub fn parse_decimal(raw: &str, decimals: u8) -> Result<u128, String> {
    let trimmed = raw.trim().replace(',', "");
    if trimmed.is_empty() {
        return Err("empty".to_string());
    }
    if trimmed.starts_with('-') {
        return Err("negative amounts are not valid limits".to_string());
    }

    let (whole, fraction) = match trimmed.split_once('.') {
        Some((w, f)) => (w, f),
        None => (trimmed.as_str(), ""),
    };

    if whole.is_empty() && fraction.is_empty() {
        return Err("no digits".to_string());
    }
    if !whole.chars().all(|c| c.is_ascii_digit()) || !fraction.chars().all(|c| c.is_ascii_digit()) {
        return Err(format!("'{raw}' is not a decimal number"));
    }
    if fraction.len() > decimals as usize {
        return Err(format!(
            "'{raw}' has more than {decimals} decimal places"
        ));
    }

    let whole_units: u128 = if whole.is_empty() {
        0
    } else {
        whole.parse().map_err(|_| format!("'{raw}' is too large"))?
    };

    let scale = 10u128
        .checked_pow(u32::from(decimals))
        .ok_or_else(|| "decimals out of range".to_string())?;

    let mut padded = fraction.to_string();
    while padded.len() < decimals as usize {
        padded.push('0');
    }
    let fraction_units: u128 = if padded.is_empty() {
        0
    } else {
        padded.parse().map_err(|_| format!("'{raw}' is too large"))?
    };

    whole_units
        .checked_mul(scale)
        .and_then(|w| w.checked_add(fraction_units))
        .ok_or_else(|| format!("'{raw}' overflows"))
}

fn parse_list(raw: Option<&String>) -> Vec<String> {
    raw.map(|s| {
        s.split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from)
            .collect()
    })
    .unwrap_or_default()
}

fn parse_mint_caps(raw: Option<&String>) -> Result<Vec<(String, String)>, String> {
    let Some(raw) = raw else {
        return Ok(Vec::new());
    };
    let mut caps = Vec::new();
    for entry in raw.split(',').map(str::trim).filter(|e| !e.is_empty()) {
        let (mint, amount) = entry.split_once(':').ok_or_else(|| {
            format!("max_out_per_mint entry '{entry}' must be MINT:AMOUNT")
        })?;
        let mint = mint.trim();
        let amount = amount.trim();
        if mint.is_empty() || amount.is_empty() {
            return Err(format!("max_out_per_mint entry '{entry}' is incomplete"));
        }
        // Validated against a generous scale here; re-parsed at the mint's real
        // decimals once those are known from chain.
        parse_decimal(amount, 18)
            .map_err(|e| format!("max_out_per_mint amount for {mint}: {e}"))?;
        caps.push((mint.to_string(), amount.to_string()));
    }
    Ok(caps)
}

fn parse_bool(raw: Option<&String>, default: bool) -> Result<bool, String> {
    match raw.map(String::as_str) {
        None | Some("") => Ok(default),
        Some("true") => Ok(true),
        Some("false") => Ok(false),
        Some(other) => Err(format!("must be 'true' or 'false', got '{other}'")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    #[test]
    fn parses_decimals_into_base_units() {
        assert_eq!(parse_decimal("25.00", 6).unwrap(), 25_000_000);
        assert_eq!(parse_decimal("0.05", 9).unwrap(), 50_000_000);
        assert_eq!(parse_decimal("2,140.00", 6).unwrap(), 2_140_000_000);
        assert_eq!(parse_decimal("1", 6).unwrap(), 1_000_000);
        assert_eq!(parse_decimal("0.000005", 9).unwrap(), 5_000);
    }

    #[test]
    fn rejects_junk_rather_than_defaulting() {
        for bad in ["abc", "1.2.3", "", "-5", "1e6", "50 USDC"] {
            assert!(parse_decimal(bad, 6).is_err(), "'{bad}' must not parse");
        }
    }

    #[test]
    fn rejects_more_precision_than_the_mint_has() {
        assert!(parse_decimal("1.0000001", 6).is_err());
    }

    #[test]
    fn builds_an_envelope_from_flat_strings() {
        let cfg = config(&[
            ("max_sol_out", "0.05"),
            ("max_out_per_mint", "EPjFWdd5:50.00, So11111:0.5"),
            ("mint_allowlist", "EPjFWdd5,So11111"),
            ("deny_authority_grants", "true"),
            ("unknown_program_policy", "fail"),
        ]);
        let env = Envelope::from_config(&cfg).expect("valid config parses");

        assert_eq!(env.max_sol_out, Some(50_000_000));
        assert_eq!(env.max_out_per_mint.len(), 2);
        assert!(env.deny_authority_grants);
        assert!(env.deny_account_close, "defaults to denying closes");
        assert_eq!(env.unknown_program_policy, UnknownProgramPolicy::Fail);
    }

    #[test]
    fn an_envelope_with_no_limits_is_an_error() {
        let err = Envelope::from_config(&config(&[("deny_authority_grants", "true")]))
            .expect_err("no declared limit must not silently pass everything");
        assert!(err.contains("no spending limit"));
    }

    #[test]
    fn a_malformed_cap_is_an_error_not_a_default() {
        let cfg = config(&[("max_sol_out", "loads")]);
        assert!(Envelope::from_config(&cfg).is_err());
    }

    #[test]
    fn malformed_mint_caps_are_rejected() {
        for bad in ["EPjFWdd5", "EPjFWdd5:", ":50.00", "EPjFWdd5:abc"] {
            let cfg = config(&[("max_out_per_mint", bad)]);
            assert!(Envelope::from_config(&cfg).is_err(), "'{bad}' must not parse");
        }
    }

    #[test]
    fn a_bad_boolean_is_an_error() {
        let cfg = config(&[("max_sol_out", "1"), ("deny_authority_grants", "yes")]);
        assert!(Envelope::from_config(&cfg).is_err());
    }

    #[test]
    fn caps_resolve_once_decimals_are_known() {
        let cfg = config(&[("max_out_per_mint", "EPjFWdd5:50.00")]);
        let env = Envelope::from_config(&cfg).unwrap();
        assert_eq!(env.cap_for("EPjFWdd5", 6).unwrap().unwrap(), 50_000_000);
        assert!(env.cap_for("SomeOtherMint", 6).is_none());
    }

    #[test]
    fn an_empty_allowlist_enforces_nothing() {
        let cfg = config(&[("max_sol_out", "1")]);
        let env = Envelope::from_config(&cfg).unwrap();
        assert!(env.mint_allowed("anything"));
    }

    #[test]
    fn a_populated_allowlist_excludes_everything_else() {
        let cfg = config(&[("max_sol_out", "1"), ("mint_allowlist", "EPjFWdd5")]);
        let env = Envelope::from_config(&cfg).unwrap();
        assert!(env.mint_allowed("EPjFWdd5"));
        assert!(!env.mint_allowed("EvilMint"));
    }
}
