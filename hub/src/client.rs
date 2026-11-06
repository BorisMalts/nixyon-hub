use std::net::{IpAddr, SocketAddr};

use axum::extract::{ConnectInfo, FromRequestParts};
use axum::http::header;
use axum::http::request::Parts;

use crate::state::AppState;

const MAX_USER_AGENT: usize = 300;

#[derive(Debug, Clone)]
pub struct Client {
    pub ip: IpAddr,
    pub user_agent: String,
}

impl Client {
    pub fn ip_text(&self) -> String {
        self.ip.to_string()
    }
}

pub fn client_ip(parts: &Parts, trusted_proxies: &[IpAddr]) -> IpAddr {
    let unknown = IpAddr::from([0, 0, 0, 0]);
    let Some(peer) = parts
        .extensions
        .get::<ConnectInfo<SocketAddr>>()
        .map(|c| c.0.ip())
    else {
        return unknown;
    };
    if !trusted_proxies.contains(&peer) {
        return peer;
    }
    parts
        .headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| {
            v.rsplit(',')
                .filter_map(|ip| ip.trim().parse::<IpAddr>().ok())
                .find(|ip| !trusted_proxies.contains(ip))
        })
        .unwrap_or(peer)
}

impl FromRequestParts<AppState> for Client {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let user_agent = parts
            .headers
            .get(header::USER_AGENT)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .chars()
            .filter(|c| !c.is_control())
            .take(MAX_USER_AGENT)
            .collect();
        Ok(Self {
            ip: client_ip(parts, &state.config.trusted_proxies),
            user_agent,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::Request;

    const PROXY: [u8; 4] = [10, 233, 0, 2];

    fn parts(peer: [u8; 4], forwarded: Option<&str>) -> Parts {
        let mut builder = Request::builder().extension(ConnectInfo(SocketAddr::from((peer, 5000))));
        if let Some(f) = forwarded {
            builder = builder.header("x-forwarded-for", f);
        }
        builder.body(()).unwrap().into_parts().0
    }

    #[test]
    fn forwarded_header_from_untrusted_peer_is_ignored() {
        let trusted = [IpAddr::from(PROXY)];
        let p = parts([10, 231, 0, 20], Some("1.2.3.4"));
        assert_eq!(client_ip(&p, &trusted), IpAddr::from([10, 231, 0, 20]));
        assert_eq!(client_ip(&p, &[]), IpAddr::from([10, 231, 0, 20]));
    }

    #[test]
    fn trusted_proxy_gives_rightmost_untrusted_address() {
        let trusted = [IpAddr::from(PROXY)];
        assert_eq!(
            client_ip(&parts(PROXY, Some("6.6.6.6, 1.2.3.4")), &trusted),
            IpAddr::from([1, 2, 3, 4])
        );
        assert_eq!(
            client_ip(&parts(PROXY, Some("1.2.3.4, 10.233.0.2")), &trusted),
            IpAddr::from([1, 2, 3, 4])
        );
        assert_eq!(
            client_ip(&parts(PROXY, Some("2a01:4f8::1")), &trusted),
            "2a01:4f8::1".parse::<IpAddr>().unwrap()
        );
    }

    #[test]
    fn garbage_header_from_proxy_falls_back_to_proxy_address() {
        let trusted = [IpAddr::from(PROXY)];
        assert_eq!(
            client_ip(&parts(PROXY, Some("не адрес")), &trusted),
            IpAddr::from(PROXY)
        );
        assert_eq!(
            client_ip(&parts(PROXY, None), &trusted),
            IpAddr::from(PROXY)
        );
    }
}
