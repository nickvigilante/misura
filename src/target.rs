//! What a request is sent to, and how redirects move it.

use std::fmt;

use hyper::Uri;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scheme {
    Http,
    Https,
}

/// An absolute `http://` or `https://` URL, split into the parts each phase
/// needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub scheme: Scheme,
    pub host: String,
    pub port: u16,
    /// Path and query, always starting with `/`.
    pub path: String,
}

impl Target {
    pub fn parse(url: &str) -> Result<Self, String> {
        let uri: Uri = url
            .parse()
            .map_err(|e| format!("invalid URL {url:?}: {e}"))?;
        let scheme = match uri.scheme_str() {
            Some("http") => Scheme::Http,
            Some("https") => Scheme::Https,
            Some(other) => return Err(format!("unsupported scheme {other:?}; use http or https")),
            None => return Err(format!("{url:?} needs a scheme, such as https://")),
        };
        let host = uri
            .host()
            .ok_or_else(|| format!("{url:?} has no host"))?
            .trim_start_matches('[')
            .trim_end_matches(']')
            .to_owned();
        let port = uri.port_u16().unwrap_or(match scheme {
            Scheme::Http => 80,
            Scheme::Https => 443,
        });
        let path = uri
            .path_and_query()
            .map_or("/".to_owned(), |p| p.as_str().to_owned());
        let path = if path.is_empty() {
            "/".to_owned()
        } else {
            path
        };
        Ok(Self {
            scheme,
            host,
            port,
            path,
        })
    }

    /// The `Host` header and HTTP/2 `:authority` value.
    pub fn authority(&self) -> String {
        let host = if self.host.contains(':') {
            format!("[{}]", self.host)
        } else {
            self.host.clone()
        };
        let default = match self.scheme {
            Scheme::Http => 80,
            Scheme::Https => 443,
        };
        if self.port == default {
            host
        } else {
            format!("{host}:{}", self.port)
        }
    }

    /// Whether a connection to `self` can carry a request for `other`.
    pub fn same_origin(&self, other: &Target) -> bool {
        self.scheme == other.scheme
            && self.host.eq_ignore_ascii_case(&other.host)
            && self.port == other.port
    }

    /// Resolves a `Location` header against this URL.
    pub fn redirect(&self, location: &str) -> Result<Target, String> {
        if location.contains("://") {
            return Target::parse(location);
        }
        let scheme = match self.scheme {
            Scheme::Http => "http",
            Scheme::Https => "https",
        };
        if let Some(rest) = location.strip_prefix("//") {
            return Target::parse(&format!("{scheme}://{rest}"));
        }
        let path = if location.starts_with('/') {
            location.to_owned()
        } else {
            // Relative to the current path's directory.
            let base = self.path.split('?').next().unwrap_or("/");
            let dir = &base[..=base.rfind('/').unwrap_or(0)];
            format!("{dir}{location}")
        };
        Ok(Target {
            path,
            ..self.clone()
        })
    }
}

impl fmt::Display for Target {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let scheme = match self.scheme {
            Scheme::Http => "http",
            Scheme::Https => "https",
        };
        write!(f, "{scheme}://{}{}", self.authority(), self.path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_urls() {
        let t = Target::parse("https://example.com").unwrap();
        assert_eq!(
            (t.scheme, t.host.as_str(), t.port, t.path.as_str()),
            (Scheme::Https, "example.com", 443, "/")
        );
        let t = Target::parse("http://[::1]:8080/a?b=1").unwrap();
        assert_eq!(
            (t.host.as_str(), t.port, t.path.as_str()),
            ("::1", 8080, "/a?b=1")
        );
        assert_eq!(t.authority(), "[::1]:8080");
        assert!(Target::parse("ftp://x").is_err());
        assert!(Target::parse("example.com").is_err());
    }

    #[test]
    fn resolves_redirects() {
        let t = Target::parse("https://example.com/docs/page?x=1").unwrap();
        assert_eq!(
            t.redirect("/login").unwrap().to_string(),
            "https://example.com/login"
        );
        assert_eq!(
            t.redirect("other").unwrap().to_string(),
            "https://example.com/docs/other"
        );
        assert_eq!(
            t.redirect("//cdn.example.com/a").unwrap().to_string(),
            "https://cdn.example.com/a"
        );
        assert_eq!(
            t.redirect("http://example.org/").unwrap().to_string(),
            "http://example.org/"
        );
        assert!(t.same_origin(&t.redirect("/x").unwrap()));
        assert!(!t.same_origin(&t.redirect("//cdn.example.com/").unwrap()));
    }
}
