//! Password hashing and session-token helpers shared by the application and
//! infrastructure layers. Pure cryptography: no HTTP or database types here.

use argon2::Argon2;
use argon2::password_hash::{
    PasswordHash, PasswordHasher, PasswordVerifier, SaltString, rand_core::OsRng,
};
use sha2::{Digest, Sha256};

/// Sessions live 30 days.
pub const SESSION_TTL_MS: i64 = 30 * 24 * 3600 * 1000;

/// Hash a password with Argon2id into a PHC string.
pub fn hash_password(password: &str) -> Result<String, argon2::password_hash::Error> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
}

/// Verify a password against an Argon2id PHC hash.
pub fn verify_password(hash: &str, password: &str) -> bool {
    PasswordHash::new(hash)
        .ok()
        .and_then(|parsed| {
            Argon2::default()
                .verify_password(password.as_bytes(), &parsed)
                .ok()
        })
        .is_some()
}

/// SHA-256 hex digest, used for session tokens (only the digest is stored).
pub fn sha256_hex(input: &str) -> String {
    hex::encode(Sha256::digest(input.as_bytes()))
}
