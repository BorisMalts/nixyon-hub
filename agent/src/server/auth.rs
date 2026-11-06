use sha2::{Digest, Sha256};

pub struct TokenCheck {
    expected: Vec<u8>,
}

impl TokenCheck {
    pub fn new(token: &str) -> Option<Self> {
        let token = token.trim();
        if token.len() < 32 {
            return None;
        }
        Some(Self {
            expected: Sha256::digest(token.as_bytes()).to_vec(),
        })
    }

    pub fn matches(&self, given: &str) -> bool {
        let given = Sha256::digest(given.as_bytes());
        self.expected
            .iter()
            .zip(given.iter())
            .fold(0u8, |acc, (a, b)| acc | (a ^ b))
            == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_token_is_refused_and_match_is_exact() {
        assert!(TokenCheck::new("короткий").is_none());
        let check = TokenCheck::new(&"a".repeat(40)).unwrap();
        assert!(check.matches(&"a".repeat(40)));
        assert!(!check.matches(&"a".repeat(41)));
        assert!(!check.matches(""));
    }
}
