//! Webhook signature verification and event envelope.
//!
//! `Stripe-Signature: t=<unix>,v1=<hex hmac>[,v1=...]`, where the MAC is
//! HMAC-SHA256(webhook secret, `"{t}.{raw body}"`). Requests older (or newer) than the
//! tolerance are rejected to stop replays. Verify against the RAW body bytes.

use hmac::{Hmac, Mac};
use serde::Deserialize;
use serde_json::Value;
use sha2::Sha256;

/// Stripe's recommended default tolerance.
pub const DEFAULT_TOLERANCE_SECS: i64 = 300;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SignatureError {
    #[error("malformed Stripe-Signature header")]
    Malformed,
    #[error("no v1 signature in header")]
    NoSignature,
    #[error("timestamp outside the tolerance window")]
    Expired,
    #[error("signature mismatch")]
    Mismatch,
}

fn hex_decode(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok())
        .collect()
}

fn mac(secret: &str, timestamp: &str, payload: &[u8]) -> Option<Hmac<Sha256>> {
    let mut m = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).ok()?;
    m.update(timestamp.as_bytes());
    m.update(b".");
    m.update(payload);
    Some(m)
}

/// Verify `header` against the raw `payload`. `now` is unix seconds (injected for tests).
pub fn verify_signature(
    payload: &[u8],
    header: &str,
    secret: &str,
    now: i64,
    tolerance_secs: i64,
) -> Result<(), SignatureError> {
    let mut timestamp: Option<&str> = None;
    let mut sigs: Vec<&str> = Vec::new();
    for part in header.split(',') {
        let (k, v) = part.trim().split_once('=').ok_or(SignatureError::Malformed)?;
        match k {
            "t" => timestamp = Some(v),
            "v1" => sigs.push(v),
            _ => {} // v0 (test-mode legacy) and future schemes are ignored
        }
    }
    let ts = timestamp.ok_or(SignatureError::Malformed)?;
    let ts_num: i64 = ts.parse().map_err(|_| SignatureError::Malformed)?;
    if sigs.is_empty() {
        return Err(SignatureError::NoSignature);
    }
    if (now - ts_num).abs() > tolerance_secs {
        return Err(SignatureError::Expired);
    }
    for s in sigs {
        let Some(bytes) = hex_decode(s) else { continue };
        let Some(m) = mac(secret, ts, payload) else {
            return Err(SignatureError::Mismatch);
        };
        // verify_slice compares in constant time.
        if m.verify_slice(&bytes).is_ok() {
            return Ok(());
        }
    }
    Err(SignatureError::Mismatch)
}

/// Compute the `v1` value (tests and local tooling).
pub fn sign(payload: &[u8], timestamp: i64, secret: &str) -> String {
    let Some(m) = mac(secret, &timestamp.to_string(), payload) else {
        return String::new();
    };
    m.finalize()
        .into_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Full header for a payload (tests).
pub fn signature_header(payload: &[u8], timestamp: i64, secret: &str) -> String {
    format!("t={timestamp},v1={}", sign(payload, timestamp, secret))
}

/// The event envelope: id, type and `data.object`.
#[derive(Debug, Clone, Deserialize)]
pub struct Event {
    pub id: String,
    #[serde(rename = "type")]
    pub event_type: String,
    pub data: EventData,
    #[serde(default)]
    pub created: Option<i64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct EventData {
    pub object: Value,
}

pub fn parse_event(payload: &[u8]) -> Result<Event, serde_json::Error> {
    serde_json::from_slice(payload)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    const SECRET: &str = "whsec_test_secret";
    const PAYLOAD: &str = r#"{"id":"evt_test","object":"event"}"#;
    const T: i64 = 1_700_000_000;
    // Independent vector computed with `openssl dgst -sha256 -hmac`.
    const VECTOR: &str = "8042376f6ca064adbe037642a718227dfd59047137eb49497a54c49eb0cfb724";

    #[test]
    fn known_vector() {
        assert_eq!(sign(PAYLOAD.as_bytes(), T, SECRET), VECTOR);
        let h = format!("t={T},v1={VECTOR}");
        assert_eq!(verify_signature(PAYLOAD.as_bytes(), &h, SECRET, T + 10, 300), Ok(()));
    }

    #[test]
    fn bad_signature_and_tampered_payload() {
        let h = format!("t={T},v1={VECTOR}");
        assert_eq!(
            verify_signature(b"{\"id\":\"evt_other\"}", &h, SECRET, T, 300),
            Err(SignatureError::Mismatch)
        );
        assert_eq!(
            verify_signature(PAYLOAD.as_bytes(), &h, "whsec_other", T, 300),
            Err(SignatureError::Mismatch)
        );
        let bad = format!("t={T},v1={}", "0".repeat(64));
        assert_eq!(
            verify_signature(PAYLOAD.as_bytes(), &bad, SECRET, T, 300),
            Err(SignatureError::Mismatch)
        );
        let not_hex = format!("t={T},v1=zz");
        assert_eq!(
            verify_signature(PAYLOAD.as_bytes(), &not_hex, SECRET, T, 300),
            Err(SignatureError::Mismatch)
        );
    }

    #[test]
    fn replay_outside_tolerance_is_rejected() {
        let h = format!("t={T},v1={VECTOR}");
        assert_eq!(
            verify_signature(PAYLOAD.as_bytes(), &h, SECRET, T + 301, 300),
            Err(SignatureError::Expired)
        );
        assert_eq!(
            verify_signature(PAYLOAD.as_bytes(), &h, SECRET, T - 301, 300),
            Err(SignatureError::Expired)
        );
        assert_eq!(verify_signature(PAYLOAD.as_bytes(), &h, SECRET, T + 300, 300), Ok(()));
    }

    #[test]
    fn malformed_headers() {
        for h in ["", "garbage", "t=abc,v1=00", "v1=00"] {
            assert_eq!(
                verify_signature(PAYLOAD.as_bytes(), h, SECRET, T, 300),
                Err(SignatureError::Malformed),
                "{h}"
            );
        }
        assert_eq!(
            verify_signature(PAYLOAD.as_bytes(), &format!("t={T}"), SECRET, T, 300),
            Err(SignatureError::NoSignature)
        );
    }

    #[test]
    fn any_matching_v1_passes_and_v0_is_ignored() {
        let h = format!("t={T},v0=ff,v1={},v1={VECTOR}", "1".repeat(64));
        assert_eq!(verify_signature(PAYLOAD.as_bytes(), &h, SECRET, T, 300), Ok(()));
    }

    #[test]
    fn parses_envelope() {
        let e = parse_event(
            br#"{"id":"evt_1","type":"invoice.paid","created":5,"data":{"object":{"id":"in_1"}}}"#,
        )
        .unwrap();
        assert_eq!(e.id, "evt_1");
        assert_eq!(e.event_type, "invoice.paid");
        assert_eq!(e.data.object["id"], "in_1");
    }
}
