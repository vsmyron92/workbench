//! Which push endpoints Workbench sends to.
//!
//! A subscription's endpoint comes from a browser, and the server POSTs to it: this
//! is an SSRF boundary. Only https URLs on the browsers' push services (plus the
//! owner's `[push] extra_endpoint_hosts`) on the default port are accepted, without
//! credentials, IP literals or fragments; redirects are never followed.

use reqwest::Url;

/// Hosts of the push services browsers use.
const KNOWN_HOSTS: &[(&str, &str)] = &[
    ("fcm.googleapis.com", "Google (FCM)"),
    ("updates.push.services.mozilla.com", "Mozilla"),
    ("web.push.apple.com", "Apple"),
];
/// Windows Push Notification Services (Edge on Windows): `wns2-*.notify.windows.com`.
const WNS_SUFFIX: &str = ".notify.windows.com";
const MAX_ENDPOINT_LEN: usize = 2048;

#[derive(Debug, Clone, Default)]
pub struct EndpointPolicy {
    /// `[push] extra_endpoint_hosts`: exact host names, or `*.example.com` for subdomains.
    pub extra_hosts: Vec<String>,
    /// Tests only: accept `http://127.0.0.1:<port>` so a mock push service can receive.
    #[cfg(test)]
    pub allow_loopback_http: bool,
}

impl EndpointPolicy {
    pub fn new(extra_hosts: &[String]) -> Self {
        Self { extra_hosts: extra_hosts.to_vec(), ..Default::default() }
    }

    #[cfg(test)]
    pub fn for_tests() -> Self {
        Self { allow_loopback_http: true, ..Default::default() }
    }

    /// The endpoint as a URL, or why it is refused.
    pub fn check(&self, raw: &str) -> Result<Url, String> {
        if raw.len() > MAX_ENDPOINT_LEN {
            return Err("endpoint is too long".into());
        }
        let url = Url::parse(raw.trim()).map_err(|e| format!("endpoint is not a URL: {e}"))?;
        if !url.username().is_empty() || url.password().is_some() {
            return Err("endpoint must not carry credentials".into());
        }
        if url.fragment().is_some() {
            return Err("endpoint must not have a fragment".into());
        }
        #[cfg(test)]
        if self.allow_loopback_http
            && url.scheme() == "http"
            && url.host_str() == Some("127.0.0.1")
        {
            return Ok(url);
        }
        if url.scheme() != "https" {
            return Err("endpoint must be https".into());
        }
        if url.port().is_some_and(|p| p != 443) {
            return Err("endpoint must use the default https port".into());
        }
        // `domain()` is `None` for IP literals.
        let host = match (url.domain(), url.host_str()) {
            (Some(h), _) => h.to_ascii_lowercase(),
            (None, Some(_)) => return Err("endpoint must name a push service host, not an IP address".into()),
            (None, None) => return Err("endpoint has no host".into()),
        };
        if service_name(&host).is_some() || self.extra_allows(&host) {
            Ok(url)
        } else {
            Err(format!(
                "{host} is not a known push service (add it to [push] extra_endpoint_hosts in config.toml to allow it)"
            ))
        }
    }

    fn extra_allows(&self, host: &str) -> bool {
        self.extra_hosts.iter().any(|e| {
            let e = e.trim().to_ascii_lowercase();
            match e.strip_prefix("*.") {
                Some(suffix) => !suffix.is_empty() && host.len() > suffix.len() + 1 && host.ends_with(&format!(".{suffix}")),
                None => !e.is_empty() && host == e,
            }
        })
    }
}

/// A readable name for a known push service host.
pub fn service_name(host: &str) -> Option<&'static str> {
    let host = host.to_ascii_lowercase();
    if let Some((_, name)) = KNOWN_HOSTS.iter().find(|(h, _)| *h == host) {
        return Some(name);
    }
    (host.ends_with(WNS_SUFFIX) && host.len() > WNS_SUFFIX.len() && !host[..host.len() - WNS_SUFFIX.len()].contains('.'))
        .then_some("Microsoft (WNS)")
}

/// `https://host` of an endpoint: the VAPID audience.
pub fn origin(url: &Url) -> String {
    url.origin().ascii_serialization()
}

/// Whether an `extra_endpoint_hosts` entry is well formed (Settings validation).
pub fn valid_extra_host(entry: &str) -> bool {
    let h = entry.strip_prefix("*.").unwrap_or(entry);
    !h.is_empty()
        && h.contains('.')
        && h.len() <= 253
        && h.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.')
        && !h.starts_with('.')
        && !h.ends_with('.')
        && h.parse::<std::net::IpAddr>().is_err()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_the_browsers_push_services() {
        let p = EndpointPolicy::default();
        for ok in [
            "https://fcm.googleapis.com/fcm/send/dXJ5c3Q6APA91bH",
            "https://updates.push.services.mozilla.com/wpush/v2/gAAAAABk",
            "https://web.push.apple.com/QGx3dW1qc2Q",
            "https://wns2-par02p.notify.windows.com/w/?token=BQYAAAD",
            "https://FCM.googleapis.com:443/fcm/send/x",
        ] {
            assert!(p.check(ok).is_ok(), "{ok}: {:?}", p.check(ok));
        }
    }

    #[test]
    fn refuses_anything_else() {
        let p = EndpointPolicy::default();
        for bad in [
            "http://fcm.googleapis.com/fcm/send/x",            // not https
            "https://fcm.googleapis.com:8443/fcm/send/x",      // another port
            "https://user:pw@fcm.googleapis.com/fcm/send/x",   // credentials
            "https://fcm.googleapis.com.evil.example/x",       // look-alike
            "https://evilfcm.googleapis.com/x",                // another subdomain
            "https://notify.windows.com/x",                    // bare suffix
            "https://a.b.notify.windows.com/x",                // nested subdomain
            "https://127.0.0.1/x",                             // IP literal
            "https://[::1]/x",
            "https://169.254.169.254/latest/meta-data",
            "https://localhost/x",
            "https://fcm.googleapis.com/x#frag",
            "ftp://fcm.googleapis.com/x",
            "http://127.0.0.1:7846/push",                      // loopback only in the tests' policy
            "not a url",
        ] {
            assert!(p.check(bad).is_err(), "{bad} should be refused");
        }
        assert!(p.check(&format!("https://fcm.googleapis.com/{}", "a".repeat(2100))).is_err());
    }

    #[test]
    fn extra_hosts_are_exact_or_wildcard_suffixes() {
        let p = EndpointPolicy::new(&["push.example.org".into(), "*.push.corp.example".into()]);
        assert!(p.check("https://push.example.org/sub/1").is_ok());
        assert!(p.check("https://eu.push.corp.example/sub/1").is_ok());
        assert!(p.check("https://push.corp.example/sub/1").is_err(), "the wildcard wants a subdomain");
        assert!(p.check("https://x.push.example.org/sub/1").is_err());
        assert!(p.check("https://evilpush.corp.example/sub/1").is_err());
        assert!(valid_extra_host("*.push.corp.example"));
        assert!(valid_extra_host("push.example.org"));
        assert!(!valid_extra_host("https://push.example.org"));
        assert!(!valid_extra_host("10.0.0.1"));
        assert!(!valid_extra_host("*."));
        assert!(!valid_extra_host("localhost"));
    }

    #[test]
    fn test_policy_only_adds_loopback_http() {
        let p = EndpointPolicy::for_tests();
        assert!(p.check("http://127.0.0.1:7846/push/abc").is_ok());
        assert!(p.check("http://10.0.0.1:7846/push/abc").is_err());
        assert!(p.check("http://localhost:7846/push/abc").is_err());
    }

    #[test]
    fn names_and_origins() {
        assert_eq!(service_name("fcm.googleapis.com"), Some("Google (FCM)"));
        assert_eq!(service_name("wns2-par02p.notify.windows.com"), Some("Microsoft (WNS)"));
        assert_eq!(service_name("push.example.org"), None);
        let u = Url::parse("https://web.push.apple.com/QGx3dW1qc2Q").unwrap();
        assert_eq!(origin(&u), "https://web.push.apple.com");
    }
}
