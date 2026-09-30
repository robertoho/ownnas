use argon2::{
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
use rand::rngs::OsRng;
use rand::RngCore;
use sha2::{Digest, Sha256};

pub const COOKIE_NAME: &str = "ownnas_session";
pub const SESSION_SECS: i64 = 14 * 24 * 60 * 60;

pub fn validate_username(username: &str) -> Result<(), &'static str> {
    let trimmed = username.trim();
    let len = trimmed.chars().count();
    if !(1..=64).contains(&len) {
        return Err("Username must be 1 to 64 characters");
    }
    if !trimmed
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '-')
    {
        return Err("Username may contain letters, numbers, dots, underscores, and hyphens");
    }
    Ok(())
}

pub fn validate_password(password: &str) -> Result<(), &'static str> {
    let chars = password.chars().count();
    if chars < 8 {
        return Err("Password must be at least 8 characters");
    }
    if chars > 200 || password.len() > 1024 {
        return Err("Password is too long");
    }
    Ok(())
}

pub fn hash_password(password: &str) -> Result<String, String> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|_| "Could not hash the password".to_string())
}

pub fn verify_password(password: &str, encoded: &str) -> bool {
    let Ok(parsed) = PasswordHash::new(encoded) else {
        return false;
    };
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok()
}

pub fn new_token() -> String {
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    hex::encode(bytes)
}

pub fn token_hash(token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    hex::encode(hasher.finalize())
}

pub fn session_cookie(token: &str, secure: bool) -> String {
    let mut cookie = format!(
        "{COOKIE_NAME}={token}; HttpOnly; SameSite=Lax; Path=/; Max-Age={SESSION_SECS}"
    );
    if secure {
        cookie.push_str("; Secure");
    }
    cookie
}

pub fn clear_cookie(secure: bool) -> String {
    let mut cookie = format!("{COOKIE_NAME}=; HttpOnly; SameSite=Lax; Path=/; Max-Age=0");
    if secure {
        cookie.push_str("; Secure");
    }
    cookie
}

pub fn read_cookie(header: &str, name: &str) -> Option<String> {
    for part in header.split(';') {
        let mut pieces = part.trim().splitn(2, '=');
        let key = pieces.next()?.trim();
        let value = pieces.next()?.trim();
        if key == name && !value.is_empty() {
            return Some(value.to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn password_roundtrip() {
        let hash = hash_password("correct horse").unwrap();
        assert!(verify_password("correct horse", &hash));
        assert!(!verify_password("wrong horse", &hash));
    }

    #[test]
    fn username_rules() {
        assert!(validate_username("admin").is_ok());
        assert!(validate_username("a b").is_err());
        assert!(validate_password("short").is_err());
        assert!(validate_password("longenough").is_ok());
    }
}
