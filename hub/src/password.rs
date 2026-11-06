use argon2::Argon2;
use argon2::password_hash::phc::PasswordHash;
use argon2::password_hash::{PasswordHasher, PasswordVerifier};

pub const MIN_LENGTH: usize = 12;
const MAX_LENGTH: usize = 256;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum PasswordError {
    #[error("пароль короче {MIN_LENGTH} символов")]
    TooShort,
    #[error("пароль длиннее {MAX_LENGTH} символов")]
    TooLong,
    #[error("не удалось посчитать хеш пароля")]
    Hash,
}

pub fn hash(password: &str) -> Result<String, PasswordError> {
    let length = password.chars().count();
    if length < MIN_LENGTH {
        return Err(PasswordError::TooShort);
    }
    if length > MAX_LENGTH {
        return Err(PasswordError::TooLong);
    }
    Argon2::default()
        .hash_password(password.as_bytes())
        .map(|h| h.to_string())
        .map_err(|_| PasswordError::Hash)
}

pub fn verify(password: &str, stored: &str) -> bool {
    if password.len() > MAX_LENGTH * 4 {
        return false;
    }
    match PasswordHash::new(stored) {
        Ok(parsed) => Argon2::default()
            .verify_password(password.as_bytes(), &parsed)
            .is_ok(),
        Err(_) => false,
    }
}

pub fn dummy_hash() -> &'static str {
    "$argon2id$v=19$m=19456,t=2,p=1$c29tZXNhbHRzb21lc2FsdA$H4UrMbNWa0WGB7TqpcFw2ysKFYw7Dz8bOWdXkwOMJCo"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn right_password_verifies_and_wrong_does_not() {
        let stored = hash("правильный-пароль").unwrap();
        assert!(stored.starts_with("$argon2id$"));
        assert!(verify("правильный-пароль", &stored));
        assert!(!verify("неправильный-пароль", &stored));
    }

    #[test]
    fn same_password_gets_different_salt() {
        assert_ne!(
            hash("одинаковый-пароль").unwrap(),
            hash("одинаковый-пароль").unwrap()
        );
    }

    #[test]
    fn length_limits_are_in_characters() {
        assert_eq!(hash("короткий"), Err(PasswordError::TooShort));
        assert!(hash(&"ж".repeat(MIN_LENGTH)).is_ok());
        assert_eq!(
            hash(&"ж".repeat(MAX_LENGTH + 1)),
            Err(PasswordError::TooLong)
        );
    }

    #[test]
    fn broken_stored_hash_never_verifies() {
        assert!(!verify("что угодно", "не хеш"));
        assert!(!verify("что угодно", ""));
    }

    #[test]
    fn dummy_hash_parses_and_rejects_everything() {
        assert!(PasswordHash::new(dummy_hash()).is_ok());
        assert!(!verify("что угодно", dummy_hash()));
    }
}
