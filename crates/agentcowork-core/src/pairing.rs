//! P37 mobile QR pairing (P9.4): a QR-pairing session between the desktop
//! app and the mobile companion. The desktop shows a QR encoding the pairing
//! payload (endpoint + nonce); the mobile scans, posts the nonce back, and
//! the session confirms. The distinction recorded here (doc 68 §3 H18): this
//! is the **remote-control handoff** seam; the mobile monitor/steer surface
//! is a distinct post-v1 item.
//!
//! Deterministic and self-contained — the payload + confirmation are pure.

use serde::{Deserialize, Serialize};

/// The pairing lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PairingState {
    AwaitingScan,
    Confirmed,
    /// The nonce was consumed or expired — a fresh session is needed.
    Expired,
}

/// One QR pairing session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QrPairingSession {
    pub id: String,
    /// The local endpoint the mobile posts the nonce to (loopback-safe).
    pub endpoint: String,
    /// One-time nonce — the proof the scan really happened.
    pub nonce: String,
    /// Expiry (unix ms); 0 = never.
    pub expires_at_ms: u64,
    pub state: PairingState,
}

impl QrPairingSession {
    pub fn new(
        id: impl Into<String>,
        endpoint: impl Into<String>,
        nonce: impl Into<String>,
        expires_at_ms: u64,
    ) -> Self {
        Self {
            id: id.into(),
            endpoint: endpoint.into(),
            nonce: nonce.into(),
            expires_at_ms,
            state: PairingState::AwaitingScan,
        }
    }

    /// The exact payload the QR encodes — the mobile must post this back
    /// (endpoint + nonce). Deterministic.
    pub fn qr_payload(&self) -> String {
        format!("agentcowork-pair://{}?nonce={}", self.endpoint, self.nonce)
    }

    /// Confirm the pairing with the nonce the mobile posts back. Single-use:
    /// a consumed nonce cannot confirm a second time.
    pub fn confirm(&mut self, nonce: &str) -> bool {
        if self.state != PairingState::AwaitingScan {
            return false;
        }
        if self.expires_at_ms != 0 && now_ms() > self.expires_at_ms {
            self.state = PairingState::Expired;
            return false;
        }
        if nonce == self.nonce {
            self.state = PairingState::Confirmed;
            true
        } else {
            false
        }
    }

    /// Expire the session (abort / nonce consumed elsewhere).
    pub fn expire(&mut self) {
        self.state = PairingState::Expired;
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn confirm_requires_the_matching_nonce() {
        let mut s = QrPairingSession::new("p1", "127.0.0.1:47615", "n-123", 0);
        assert!(!s.confirm("wrong"));
        assert!(s.confirm("n-123"));
        assert_eq!(s.state, PairingState::Confirmed);
        // Single-use: already confirmed.
        assert!(!s.confirm("n-123"));
    }

    #[test]
    fn expired_session_cannot_confirm() {
        let mut s = QrPairingSession::new("p2", "127.0.0.1:47615", "n-1", 1); // expired long ago
        assert!(!s.confirm("n-1"));
        assert_eq!(s.state, PairingState::Expired);
    }

    #[test]
    fn qr_payload_is_deterministic() {
        let s = QrPairingSession::new("p3", "127.0.0.1:47615", "n-9", 0);
        assert_eq!(
            s.qr_payload(),
            "agentcowork-pair://127.0.0.1:47615?nonce=n-9"
        );
        assert_eq!(s.qr_payload(), s.qr_payload());
    }
}
