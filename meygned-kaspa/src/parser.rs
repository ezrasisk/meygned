use meygned_core::{KaspaPayload, PayloadOp};

use crate::error::KaspaError;

/// The current payload version this build understands.
/// Payloads with a higher version are skipped (forward compat).
pub const CURRENT_PAYLOAD_VERSION: u8 = 1;

/// Attempt to parse raw transaction payload bytes into a [`KaspaPayload`].
///
/// Returns:
/// - `Ok(Some(payload))` — valid, recognized Meygned payload
/// - `Ok(None)`          — bytes are not a Meygned payload (skip silently)
/// - `Err(_)`            — storage or internal error (not a parse failure)
///
/// Unknown payload versions return `Ok(None)` so the indexer skips them
/// without crashing — this is intentional forward-compatibility behaviour.
pub fn parse_payload(raw: &[u8]) -> Result<Option<KaspaPayload>, KaspaError> {
    if raw.is_empty() {
        return Ok(None);
    }

    // Attempt JSON deserialization — most non-Meygned payloads will fail here
    let payload: KaspaPayload = match serde_json::from_slice(raw) {
        Ok(p) => p,
        Err(_) => return Ok(None), // Not a Meygned payload — skip
    };

    // Skip unrecognized versions for forward compatibility
    if payload.version > CURRENT_PAYLOAD_VERSION {
        tracing::debug!(
            version = payload.version,
            "skipping payload with unknown version"
        );
        return Ok(None);
    }

    Ok(Some(payload))
}

/// Validate that a parsed [`PayloadOp`] is internally consistent.
/// Returns `Ok(())` if valid, `Err` with a reason string if not.
///
/// This is a best-effort check before handing off to the indexer state machine.
/// The indexer performs its own ownership checks independently.
pub fn validate_op(op: &PayloadOp) -> Result<(), String> {
    match op {
        PayloadOp::Register { name, .. } | PayloadOp::Update { name, .. } => {
            validate_name(name)?;
        }
        PayloadOp::Transfer { name } => {
            validate_name(name)?;
        }
    }
    Ok(())
}

fn validate_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("name is empty".to_string());
    }
    if !name.contains('.') {
        return Err(format!("name '{name}' has no suffix"));
    }
    // Basic sanity: no whitespace, no control chars
    if name.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(format!("name '{name}' contains invalid characters"));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use meygned_core::{ContentRef, KaspaPayload, PayloadOp};

    use super::*;

    fn make_register_payload(name: &str) -> Vec<u8> {
        let p = KaspaPayload {
            version: 1,
            op: PayloadOp::Register {
                name: name.to_string(),
                content_ref: ContentRef::Blob {
                    hash: "abc123".to_string(),
                },
                access_policy: None,
            },
        };
        serde_json::to_vec(&p).unwrap()
    }

    #[test]
    fn parses_valid_register_payload() {
        let bytes = make_register_payload("ezra.p2phost");
        let result = parse_payload(&bytes).unwrap();
        assert!(result.is_some());
    }

    #[test]
    fn empty_bytes_returns_none() {
        let result = parse_payload(&[]).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn non_meygned_bytes_returns_none() {
        let result = parse_payload(b"hello world not json").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn unknown_version_returns_none() {
        let p = KaspaPayload {
            version: 255,
            op: PayloadOp::Register {
                name: "ezra.p2phost".to_string(),
                content_ref: ContentRef::Blob {
                    hash: "abc123".to_string(),
                },
                access_policy: None,
            },
        };
        let bytes = serde_json::to_vec(&p).unwrap();
        let result = parse_payload(&bytes).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn validates_good_name() {
        let op = PayloadOp::Register {
            name: "ezra.p2phost".to_string(),
            content_ref: ContentRef::Blob {
                hash: "abc".to_string(),
            },
            access_policy: None,
        };
        assert!(validate_op(&op).is_ok());
    }

    #[test]
    fn rejects_name_without_suffix() {
        let op = PayloadOp::Register {
            name: "nosuffix".to_string(),
            content_ref: ContentRef::Blob {
                hash: "abc".to_string(),
            },
            access_policy: None,
        };
        assert!(validate_op(&op).is_err());
    }

    #[test]
    fn rejects_empty_name() {
        let op = PayloadOp::Transfer {
            name: "".to_string(),
        };
        assert!(validate_op(&op).is_err());
    }
}
