use url::Url;

/// Validates a WELIST server URL to mitigate SSRF risks.
///
/// Checks:
/// 1. URL must be well-formed and parsable.
/// 2. Scheme must be `https://` (plain HTTP is rejected).
/// 3. Host must be present.
/// 4. Host must not be `localhost` or loopback strings.
/// 5. Host must not resolve to a loopback, private, link-local, unspecified, or multicast IP address.
/// 6. IPv6 Unique Local (fc00::/7) is also rejected.
///
/// Returns `true` if the URL is safe to use, `false` otherwise.
///
/// Examples:
/// - `is_valid_welist_url("https://welist.bitcoin-after.life")` -> `true`
/// - `is_valid_welist_url("https://welist.bitcoin-after.life:443")` -> `true`
/// - `is_valid_welist_url("https://example.com/ping")` -> `true`
/// - `is_valid_welist_url("http://welist.bitcoin-after.life")` -> `false` (not HTTPS)
/// - `is_valid_welist_url("https://localhost")` -> `false` (localhost loopback)
/// - `is_valid_welist_url("https://127.0.0.1")` -> `false` (IPv4 loopback)
/// - `is_valid_welist_url("https://169.254.169.254")` -> `false` (AWS metadata link-local)
/// - `is_valid_welist_url("https://192.168.1.1")` -> `false` (IPv4 private)
/// - `is_valid_welist_url("https://10.0.0.1")` -> `false` (IPv4 private RFC1918)
pub fn is_valid_welist_url(url_str: &str) -> bool {
    let url = match Url::parse(url_str) {
        Ok(u) => u,
        Err(_e) => return false,
    };

    if url.scheme() != "https" {
        return false;
    }

    let host = match url.host_str() {
        Some(h) => h.trim_start_matches('[').trim_end_matches(']'),
        None => return false,
    };

    if host.eq_ignore_ascii_case("localhost")
        || host.eq_ignore_ascii_case("127.0.0.1")
        || host.eq_ignore_ascii_case("::1")
    {
        return false;
    }

    if let Ok(ip) = host.parse::<std::net::IpAddr>() {
        match ip {
            std::net::IpAddr::V4(v4) => {
                if v4.is_loopback()
                    || v4.is_private()
                    || v4.is_link_local()
                    || v4.is_unspecified()
                    || v4.is_multicast()
                {
                    return false;
                }
            }
            std::net::IpAddr::V6(v6) => {
                if v6.is_loopback()
                    || v6.is_unicast_link_local()
                    || v6.is_unspecified()
                    || v6.is_multicast()
                    || v6.is_unique_local()
                {
                    return false;
                }
            }
        }
    }

    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_domains() {
        assert!(is_valid_welist_url("https://welist.bitcoin-after.life"));
        assert!(is_valid_welist_url("https://welist.bitcoin-after.life:443"));
        assert!(is_valid_welist_url("https://example.com/ping"));
        assert!(is_valid_welist_url("https://a.b.c.d.example.com"));
        assert!(is_valid_welist_url("https://welist.onion.tor"));
    }

    #[test]
    fn test_invalid_scheme() {
        assert!(!is_valid_welist_url("http://welist.bitcoin-after.life"));
        assert!(!is_valid_welist_url("ftp://welist.bitcoin-after.life"));
        assert!(!is_valid_welist_url("https://")); // no host
    }

    #[test]
    fn test_localhost_and_loopback() {
        assert!(!is_valid_welist_url("https://localhost"));
        assert!(!is_valid_welist_url("https://localhost:8080"));
        assert!(
            !is_valid_welist_url("https://LOCALHOST"),
            "Uppercase localhost should be blocked"
        );
        assert!(!is_valid_welist_url("https://127.0.0.1"));
        assert!(!is_valid_welist_url("https://127.0.0.1:8080"));
        assert!(
            !is_valid_welist_url("https://127.0.0.2"),
            "Other loopback in 127/8 should be blocked"
        );
        assert!(
            !is_valid_welist_url("https://[::1]"),
            "IPv6 loopback literal should be blocked"
        );
        assert!(
            !is_valid_welist_url("https://::1"),
            "Raw IPv6 loopback without brackets should be invalid (parse fails)"
        );
    }

    #[test]
    fn test_private_ips() {
        assert!(!is_valid_welist_url("https://192.168.1.1"));
        assert!(!is_valid_welist_url("https://10.0.0.1"));
        assert!(!is_valid_welist_url("https://172.16.0.1"));
        assert!(!is_valid_welist_url("https://172.31.255.255"));
        assert!(
            !is_valid_welist_url("https://169.254.169.254"),
            "AWS metadata link-local IP should be blocked"
        );
    }

    #[test]
    fn test_unspecified_and_multicast() {
        assert!(!is_valid_welist_url("https://0.0.0.0"));
        assert!(!is_valid_welist_url("https://224.0.0.1"));
    }

    #[test]
    fn test_ipv6_link_local() {
        assert!(
            !is_valid_welist_url("https://[fe80::1]"),
            "IPv6 link local should be blocked"
        );
    }

    #[test]
    fn test_ipv6_unique_local() {
        assert!(
            !is_valid_welist_url("https://[fc00::1]"),
            "IPv6 unique local (fc00::/7) should be blocked"
        );
        assert!(
            !is_valid_welist_url("https://[fd00::1]"),
            "IPv6 unique local (fd00::7) should be blocked"
        );
    }

    #[test]
    fn test_malformed_urls() {
        assert!(!is_valid_welist_url("not a url"));
        assert!(!is_valid_welist_url("welist.bitcoin-after.life")); // missing scheme
    }

    #[test]
    fn test_valid_public_ip() {
        assert!(is_valid_welist_url("https://1.2.3.4"));
        assert!(is_valid_welist_url("https://8.8.8.8"));
    }
}
