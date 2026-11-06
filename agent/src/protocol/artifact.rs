use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Artifact {
    pub id: String,
    pub file: String,
    pub commit: String,
    pub subject: String,
    pub created_at: i64,
    pub size: u64,
    pub built_by: String,
}

impl Artifact {
    pub fn is_valid_id(id: &str) -> bool {
        !id.is_empty()
            && id.len() <= 64
            && id
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_cannot_escape_the_artifacts_folder() {
        assert!(Artifact::is_valid_id("20261028-120000-abc1234"));
        for bad in [
            "",
            "../x",
            "a/b",
            "a\\b",
            "A",
            "a.apk",
            "a b",
            &"a".repeat(65),
        ] {
            assert!(!Artifact::is_valid_id(bad), "{bad:?}");
        }
    }
}
