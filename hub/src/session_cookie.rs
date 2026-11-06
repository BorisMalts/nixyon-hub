use axum::http::HeaderMap;
use axum::http::header::COOKIE;

pub const NAME_SECURE: &str = "__Host-nixyon_session";
pub const NAME_LOCAL: &str = "nixyon_session";

pub fn name(secure: bool) -> &'static str {
    if secure { NAME_SECURE } else { NAME_LOCAL }
}

pub fn read(headers: &HeaderMap, secure: bool) -> Option<String> {
    let wanted = name(secure);
    headers
        .get_all(COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(k, _)| *k == wanted)
        .map(|(_, v)| v.to_string())
        .filter(|v| v.len() == 64 && v.bytes().all(|b| b.is_ascii_hexdigit()))
}

pub fn set(token: &str, secure: bool, max_age: i64) -> String {
    let secure_attr = if secure { "; Secure" } else { "" };
    format!(
        "{}={token}; Path=/; HttpOnly; SameSite=Strict; Max-Age={max_age}{secure_attr}",
        name(secure)
    )
}

pub fn clear(secure: bool) -> String {
    set("", secure, 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    #[test]
    fn secure_cookie_has_host_prefix_and_strict_flags() {
        let c = set(&"a".repeat(64), true, 100);
        assert!(c.starts_with("__Host-nixyon_session="));
        for flag in [
            "Path=/",
            "HttpOnly",
            "SameSite=Strict",
            "Secure",
            "Max-Age=100",
        ] {
            assert!(c.contains(flag), "{flag}");
        }
        assert!(!c.contains("Domain"));
    }

    #[test]
    fn only_well_formed_token_is_read() {
        let mut h = HeaderMap::new();
        h.insert(
            COOKIE,
            HeaderValue::from_str(&format!("x=1; __Host-nixyon_session={}", "ab".repeat(32)))
                .unwrap(),
        );
        assert_eq!(read(&h, true), Some("ab".repeat(32)));
        assert_eq!(read(&h, false), None);
        h.insert(
            COOKIE,
            HeaderValue::from_bytes("__Host-nixyon_session=короткий".as_bytes()).unwrap(),
        );
        assert_eq!(read(&h, true), None);
    }
}
