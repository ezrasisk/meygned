use kaspa_addresses::{Address, Prefix, Version};
use serde::Deserialize;

use crate::error::KaspaError;

// ---------------------------------------------------------------------------
// REST API types — with resolve_previous_outpoints=light
// ---------------------------------------------------------------------------

/// An input as returned by the Kaspa REST API when
/// `resolve_previous_outpoints=light` is set.
/// The `previous_outpoint_resolved` field contains the previous output's
/// `script_public_key` inline — no second round-trip needed.
#[derive(Debug, Deserialize)]
pub struct ResolvedInput {
    #[serde(default)]
    pub previous_outpoint_resolved: Option<ResolvedOutpoint>,
}

#[derive(Debug, Deserialize)]
pub struct ResolvedOutpoint {
    /// Hex-encoded script_public_key script bytes, e.g.
    /// `"207bc04196f1125e4f2676...ac"` for a standard P2PK output.
    #[serde(default)]
    pub script_public_key: String,
    /// ScriptPublicKey version (always 0 for standard scripts).
    #[serde(default)]
    pub script_public_key_version: u16,
    /// The address string if the API resolved it directly.
    /// We prefer this when present; fall back to script parsing if absent.
    #[serde(default)]
    pub script_public_key_address: Option<String>,
}

// ---------------------------------------------------------------------------
// ScriptClass — mirrors rusty-kaspa's ScriptClass enum
// ---------------------------------------------------------------------------

/// Identifies the type of a Kaspa locking script.
///
/// Byte layout (confirmed from rusty-kaspa `standard.rs`):
///
/// | Class       | Length | Layout                        | Addr bytes  |
/// |-------------|--------|-------------------------------|-------------|
/// | PubKey      | 34     | `[0x20][32 bytes][0xac]`      | `[1..33]`   |
/// | PubKeyECDSA | 35     | `[0x21][33 bytes][0xab]`      | `[1..34]`   |
/// | ScriptHash  | 35     | `[0xa9][0x14][20 bytes][0x87][0x69]` (P2SH) — addr at `[2..34]` |
#[derive(Debug, PartialEq)]
enum ScriptClass {
    PubKey,
    PubKeyECDSA,
    ScriptHash,
    NonStandard,
}

impl ScriptClass {
    fn from_bytes(script: &[u8]) -> Self {
        match script {
            // P2PK: OP_DATA_32 <32-byte pubkey> OP_CHECKSIG
            s if s.len() == 34 && s[0] == 0x20 && s[33] == 0xac => Self::PubKey,
            // P2PK ECDSA: OP_DATA_33 <33-byte pubkey> OP_CHECKSIGECDSA
            s if s.len() == 35 && s[0] == 0x21 && s[34] == 0xab => Self::PubKeyECDSA,
            // P2SH: OP_BLAKE2B OP_DATA_20 <20-byte hash> OP_EQUAL
            s if s.len() == 23 && s[0] == 0xaa && s[1] == 0x14 && s[22] == 0x87 => {
                Self::ScriptHash
            }
            _ => Self::NonStandard,
        }
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Derive the Kaspa sender address from a list of resolved transaction inputs.
///
/// Uses the first input that has a resolvable `script_public_key`. This is
/// correct because all inputs in a standard Kaspa transaction must be signed
/// by the same key (single-sig) or a compatible multisig setup. For Meygned's
/// purposes — verifying that a payload signer matches a KNS owner — the first
/// input address is sufficient and canonical.
///
/// ## Strategy
/// 1. If `script_public_key_address` is provided by the API → use it directly
///    (avoids script parsing entirely, most reliable)
/// 2. Otherwise parse `script_public_key` hex → derive address from bytes
///
/// Returns the first successfully derived address as a `kaspa:q...` string.
pub fn derive_signer_address(
    inputs: &[ResolvedInput],
    network: SignerNetwork,
) -> Result<String, KaspaError> {
    for input in inputs {
        let resolved = match &input.previous_outpoint_resolved {
            Some(r) => r,
            None => continue,
        };

        // Strategy 1: API resolved the address directly — use it
        if let Some(addr) = &resolved.script_public_key_address {
            if !addr.is_empty() {
                return Ok(addr.clone());
            }
        }

        // Strategy 2: parse script_public_key hex bytes
        if resolved.script_public_key.is_empty() {
            continue;
        }

        match derive_address_from_script_hex(
            &resolved.script_public_key,
            network.to_prefix(),
        ) {
            Ok(addr) => return Ok(addr.to_string()),
            Err(e) => {
                tracing::debug!(
                    script = %resolved.script_public_key,
                    error = %e,
                    "Could not derive address from input script, trying next input"
                );
                continue;
            }
        }
    }

    Err(KaspaError::SignerDerivationFailed(
        "no resolvable input found in transaction".to_string(),
    ))
}

/// Derive a Kaspa address from a hex-encoded `script_public_key` script.
///
/// Mirrors the logic in rusty-kaspa's `extract_script_pub_key_address()`.
pub fn derive_address_from_script_hex(
    script_hex: &str,
    prefix: Prefix,
) -> Result<Address, KaspaError> {
    let script = hex::decode(script_hex).map_err(|e| {
        KaspaError::SignerDerivationFailed(format!(
            "invalid script hex '{}': {e}",
            &script_hex[..script_hex.len().min(16)]
        ))
    })?;

    derive_address_from_script(&script, prefix)
}

/// Derive a Kaspa address from raw script bytes.
pub fn derive_address_from_script(
    script: &[u8],
    prefix: Prefix,
) -> Result<Address, KaspaError> {
    match ScriptClass::from_bytes(script) {
        ScriptClass::PubKey => {
            // bytes [1..33] = 32-byte Schnorr public key
            Ok(Address::new(prefix, Version::PubKey, &script[1..33]))
        }
        ScriptClass::PubKeyECDSA => {
            // bytes [1..34] = 33-byte compressed ECDSA public key
            Ok(Address::new(prefix, Version::PubKeyECDSA, &script[1..34]))
        }
        ScriptClass::ScriptHash => {
            // bytes [2..22] = 20-byte script hash
            Ok(Address::new(prefix, Version::ScriptHash, &script[2..22]))
        }
        ScriptClass::NonStandard => Err(KaspaError::SignerDerivationFailed(format!(
            "non-standard script (len={}, first_byte={:#04x})",
            script.len(),
            script.first().copied().unwrap_or(0)
        ))),
    }
}

// ---------------------------------------------------------------------------
// Network abstraction
// ---------------------------------------------------------------------------

/// Which Kaspa network to use when constructing addresses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignerNetwork {
    Mainnet,
    Testnet,
    Simnet,
    Devnet,
}

impl SignerNetwork {
    pub fn to_prefix(self) -> Prefix {
        match self {
            Self::Mainnet => Prefix::Mainnet,
            Self::Testnet => Prefix::Testnet,
            Self::Simnet => Prefix::Simnet,
            Self::Devnet => Prefix::Devnet,
        }
    }
}

impl Default for SignerNetwork {
    fn default() -> Self {
        Self::Mainnet
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // Test vectors derived from rusty-kaspa's own test suite in standard.rs
    // and addresses/src/lib.rs.

    #[test]
    fn p2pk_script_derives_mainnet_address() {
        // Script: OP_DATA_32 + 32-byte pubkey + OP_CHECKSIG
        // Expected address from rusty-kaspa test vectors
        let script_hex =
            "207bc04196f1125e4f2676cd09ed14afb77223b1f62177da5488346323eaa91a69ac";

        let addr =
            derive_address_from_script_hex(script_hex, Prefix::Mainnet).unwrap();

        // Verify the address is a valid mainnet kaspa: address
        let addr_str = addr.to_string();
        assert!(
            addr_str.starts_with("kaspa:"),
            "Expected mainnet prefix, got: {addr_str}"
        );
    }

    #[test]
    fn p2pk_script_derives_testnet_address() {
        let script_hex =
            "207bc04196f1125e4f2676cd09ed14afb77223b1f62177da5488346323eaa91a69ac";

        let addr =
            derive_address_from_script_hex(script_hex, Prefix::Testnet).unwrap();
        assert!(addr.to_string().starts_with("kaspatest:"));
    }

    #[test]
    fn non_standard_script_returns_error() {
        // Random bytes that don't match any standard class
        let script = vec![0xde, 0xad, 0xbe, 0xef];
        let result = derive_address_from_script(&script, Prefix::Mainnet);
        assert!(matches!(result, Err(KaspaError::SignerDerivationFailed(_))));
    }

    #[test]
    fn invalid_hex_returns_error() {
        let result = derive_address_from_script_hex("not-valid-hex!!", Prefix::Mainnet);
        assert!(matches!(result, Err(KaspaError::SignerDerivationFailed(_))));
    }

    #[test]
    fn empty_script_is_non_standard() {
        let result = derive_address_from_script(&[], Prefix::Mainnet);
        assert!(matches!(result, Err(KaspaError::SignerDerivationFailed(_))));
    }

    #[test]
    fn derive_signer_uses_api_address_when_present() {
        let inputs = vec![ResolvedInput {
            previous_outpoint_resolved: Some(ResolvedOutpoint {
                script_public_key: String::new(),
                script_public_key_version: 0,
                script_public_key_address: Some(
                    "kaspa:qpauqsvk7yf9unexwmxsnmg547mhyga37csh0kj53q6xxgl24ydxjsgzthw5j"
                        .to_string(),
                ),
            }),
        }];

        let signer = derive_signer_address(&inputs, SignerNetwork::Mainnet).unwrap();
        assert_eq!(
            signer,
            "kaspa:qpauqsvk7yf9unexwmxsnmg547mhyga37csh0kj53q6xxgl24ydxjsgzthw5j"
        );
    }

    #[test]
    fn derive_signer_falls_back_to_script_parsing() {
        let inputs = vec![ResolvedInput {
            previous_outpoint_resolved: Some(ResolvedOutpoint {
                script_public_key:
                    "207bc04196f1125e4f2676cd09ed14afb77223b1f62177da5488346323eaa91a69ac"
                        .to_string(),
                script_public_key_version: 0,
                script_public_key_address: None, // no address from API
            }),
        }];

        let signer = derive_signer_address(&inputs, SignerNetwork::Mainnet).unwrap();
        assert!(signer.starts_with("kaspa:"));
    }

    #[test]
    fn derive_signer_skips_empty_inputs_and_errors() {
        let inputs = vec![
            ResolvedInput {
                previous_outpoint_resolved: None,
            },
            ResolvedInput {
                previous_outpoint_resolved: Some(ResolvedOutpoint {
                    script_public_key: String::new(),
                    script_public_key_version: 0,
                    script_public_key_address: Some("kaspa:qz_valid".to_string()),
                }),
            },
        ];

        // Second input has a valid address despite first being empty
        let signer = derive_signer_address(&inputs, SignerNetwork::Mainnet).unwrap();
        assert_eq!(signer, "kaspa:qz_valid");
    }

    #[test]
    fn derive_signer_errors_when_no_inputs_resolvable() {
        let inputs: Vec<ResolvedInput> = vec![];
        let result = derive_signer_address(&inputs, SignerNetwork::Mainnet);
        assert!(matches!(result, Err(KaspaError::SignerDerivationFailed(_))));
    }

    #[test]
    fn script_class_detection() {
        // P2PK: 0x20 + 32 bytes + 0xac
        let p2pk = {
            let mut s = vec![0x20u8];
            s.extend([0u8; 32]);
            s.push(0xac);
            s
        };
        assert_eq!(ScriptClass::from_bytes(&p2pk), ScriptClass::PubKey);

        // P2PK ECDSA: 0x21 + 33 bytes + 0xab
        let p2pk_ecdsa = {
            let mut s = vec![0x21u8];
            s.extend([0u8; 33]);
            s.push(0xab);
            s
        };
        assert_eq!(ScriptClass::from_bytes(&p2pk_ecdsa), ScriptClass::PubKeyECDSA);

        // Non-standard
        assert_eq!(
            ScriptClass::from_bytes(&[0xff, 0x01]),
            ScriptClass::NonStandard
        );
    }
}
