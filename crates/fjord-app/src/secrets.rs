// ── fjord-app · secrets.rs ───────────────────────────────────────────────────
//   derive_key         HKDF-SHA256(device_id, APP_SALT) -> 32-byte AES-256-GCM key
//   encrypt             plaintext -> base64(nonce || ciphertext); "" in, "" out
//   decrypt_field       base64(nonce||ciphertext) -> plaintext; on any failure,
//                       falls back to treating the stored value AS the plaintext
//                       (pre-encryption migration path — see doc comment below)
// ─────────────────────────────────────────────────────────────────────────────
//
// Threat model: protects config.json's secrets (Jellyfin token, Seerr API
// key/session cookie) against piecemeal exposure — pasted into a bug report,
// grepped for a known token pattern, backed up without context. It does NOT
// protect against an attacker who has the whole file: the key is derived from
// `device_id`, which lives in plaintext in that same file, using a publicly
// known (open-source) derivation. That's the right bar for a personal HTPC,
// not a false promise of defending a co-located attacker with full disk access.
//
// Migration: versions of Fjord before this change wrote `token` as plaintext.
// `decrypt_field` tries AES-GCM decryption first; if that fails for any reason
// (bad base64, bad auth tag — both expected for a pre-existing plaintext
// value, which is not valid ciphertext), it falls back to using the stored
// string as-is. `save_config` always encrypts going forward, so a pre-upgrade
// config self-migrates to the encrypted form on its very next save. Do not
// remove this fallback thinking it's dead code — it's the only thing standing
// between an upgrade and every existing saved session being silently signed out.

use aes_gcm::aead::{Aead, KeyInit, OsRng};
use aes_gcm::aead::rand_core::RngCore;
use aes_gcm::{Aes256Gcm, Nonce};
use base64::{engine::general_purpose::STANDARD, Engine};
use hkdf::Hkdf;
use sha2::Sha256;

const APP_SALT: &[u8] = b"fjord-secrets-v1";
const NONCE_LEN: usize = 12;

pub(crate) fn derive_key(device_id: &str) -> [u8; 32] {
    let hk = Hkdf::<Sha256>::new(Some(APP_SALT), device_id.as_bytes());
    let mut key = [0u8; 32];
    // 32 is always a valid HKDF-SHA256 output length (max is 255*32).
    hk.expand(b"fjord-config-secret", &mut key)
        .expect("32-byte HKDF expand cannot fail");
    key
}

pub(crate) fn encrypt(plaintext: &str, key: &[u8; 32]) -> String {
    if plaintext.is_empty() {
        return String::new();
    }
    let Ok(cipher) = Aes256Gcm::new_from_slice(key) else {
        tracing::error!("secrets: invalid key length building cipher — storing empty, not plaintext");
        return String::new();
    };
    let mut nonce_bytes = [0u8; NONCE_LEN];
    OsRng.fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);
    match cipher.encrypt(nonce, plaintext.as_bytes()) {
        Ok(ciphertext) => {
            let mut combined = Vec::with_capacity(NONCE_LEN + ciphertext.len());
            combined.extend_from_slice(&nonce_bytes);
            combined.extend_from_slice(&ciphertext);
            STANDARD.encode(combined)
        }
        Err(e) => {
            tracing::error!("secrets: encryption failed ({e}) — storing empty, not plaintext");
            String::new()
        }
    }
}

fn try_decrypt(encoded: &str, key: &[u8; 32]) -> Option<String> {
    let combined = STANDARD.decode(encoded).ok()?;
    if combined.len() < NONCE_LEN {
        return None;
    }
    let (nonce_bytes, ciphertext) = combined.split_at(NONCE_LEN);
    let cipher = Aes256Gcm::new_from_slice(key).ok()?;
    let plain = cipher.decrypt(Nonce::from_slice(nonce_bytes), ciphertext).ok()?;
    String::from_utf8(plain).ok()
}

/// Decrypts a field written by `encrypt`, or migrates a pre-encryption
/// plaintext value (see module doc comment) by returning it unchanged.
pub(crate) fn decrypt_field(stored: &str, key: &[u8; 32]) -> String {
    if stored.is_empty() {
        return String::new();
    }
    try_decrypt(stored, key).unwrap_or_else(|| stored.to_string())
}
