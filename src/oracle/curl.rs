//! Parse a Chrome DevTools "Copy as cURL" snippet into Oracle credentials.
//!
//! This is the zero-browser fallback: the owner copies any authenticated
//! request from `wosoracle.com`, pastes it into a modal, and we pull out
//! `js_auth` (cookie) and `x-api-token` (header).

/// Credentials extracted from a paste.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedCurl {
    pub js_auth: String,
    pub api_token: String,
    pub extra_cookie: Option<String>,
}

impl ParsedCurl {
    pub fn extra_cookie_ref(&self) -> Option<&str> {
        self.extra_cookie.as_deref()
    }
}

/// Errors are user-facing, so they explain exactly what was missing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CurlError {
    Empty,
    MissingCookie,
    MissingToken,
}

impl std::fmt::Display for CurlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => write!(f, "the paste is empty"),
            Self::MissingCookie => write!(
                f,
                "no `js_auth` cookie found — copy a request while signed in to wosoracle.com"
            ),
            Self::MissingToken => write!(
                f,
                "no `x-api-token` header found — copy the request from DevTools → Network"
            ),
        }
    }
}

impl std::error::Error for CurlError {}

/// Accepts:
/// * a full cURL command (`curl '...' -H '...' -b '...'`)
/// * just a cookie string (`js_auth=...; other=...`)
/// * just a token
/// * a JSON object `{"js_auth": "...", "api_token": "..."}`
pub fn parse(raw: &str) -> Result<ParsedCurl, CurlError> {
    let text = raw.trim();
    if text.is_empty() {
        return Err(CurlError::Empty);
    }

    // JSON paste.
    if text.starts_with('{') {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(text) {
            let js_auth = value
                .get("js_auth")
                .or_else(|| value.get("jsAuth"))
                .or_else(|| value.get("js-auth"))
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .map(str::to_string);
            let api_token = value
                .get("api_token")
                .or_else(|| value.get("apiToken"))
                .or_else(|| value.get("x-api-token"))
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .map(str::to_string);
            let extra = value
                .get("extra_cookie")
                .or_else(|| value.get("extraCookie"))
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .map(str::to_string);
            if let (Some(js_auth), Some(api_token)) = (js_auth, api_token) {
                return Ok(ParsedCurl {
                    js_auth,
                    api_token,
                    extra_cookie: extra,
                });
            }
        }
    }

    let cookie_header = find_cookie_header(text);
    let api_token = find_header(text, &["x-api-token", "x_api_token", "apitoken"])
        .or_else(|| find_bearer(text))
        .ok_or(CurlError::MissingToken)?;

    let (js_auth, extra_cookie) = match cookie_header.as_deref() {
        // `Cookie: js_auth=…` header.
        Some(cookies) => split_cookies(cookies),
        // Bare cookie string without the header prefix.
        None => split_cookies(text),
    };
    let js_auth = js_auth.ok_or(CurlError::MissingCookie)?;

    Ok(ParsedCurl {
        js_auth,
        api_token,
        extra_cookie,
    })
}

/// Pull the cookie header out of `-b '...'`, `--cookie '...'` or `Cookie: ...`.
fn find_cookie_header(text: &str) -> Option<String> {
    let lower = text.to_ascii_lowercase();

    for flag in ["-b ", "--cookie "] {
        let mut search_from = 0usize;
        while let Some(relative) = lower[search_from..].find(flag) {
            let start = search_from + relative + flag.len();
            if let Some(value) = read_quoted(&text[start..]) {
                return Some(value);
            }
            search_from = start;
        }
    }

    if let Some(idx) = lower.find("cookie:") {
        let rest = &text[idx + "cookie:".len()..];
        let end = rest
            .find(['\n', '\r'])
            .unwrap_or_else(|| rest.find("'").unwrap_or(rest.len()));
        let value = rest[..end].trim().trim_matches('\'').trim_matches('"');
        if !value.is_empty() {
            return Some(value.to_string());
        }
    }
    None
}

/// Read a single- or double-quoted shell string starting at `text`.
fn read_quoted(text: &str) -> Option<String> {
    let trimmed = text.trim_start();
    let quote = trimmed.chars().next()?;
    if quote != '\'' && quote != '"' {
        // Unquoted argument: read until whitespace.
        let value = trimmed
            .split_whitespace()
            .next()
            .unwrap_or("")
            .trim()
            .to_string();
        return (!value.is_empty()).then_some(value);
    }
    let rest = &trimmed[quote.len_utf8()..];
    let end = rest.find(quote)?;
    Some(rest[..end].to_string())
}

/// Read a header value by name from `-H 'Name: value'` pairs.
fn find_header(text: &str, names: &[&str]) -> Option<String> {
    let lower = text.to_ascii_lowercase();
    for name in names {
        let needle = format!("{name}:");
        let mut search_from = 0usize;
        while let Some(relative) = lower[search_from..].find(&needle) {
            let at = search_from + relative;
            // Header values can be inside single or double quotes.
            let after = &text[at + needle.len()..];
            let value = read_header_value(after);
            if let Some(value) = value {
                if !value.trim().is_empty() {
                    return Some(value.trim().to_string());
                }
            }
            search_from = at + needle.len();
        }
    }
    None
}

fn read_header_value(after: &str) -> Option<String> {
    let trimmed = after.trim_start();
    // Stop at the next quote/line break: inside a cURL header the value runs
    // up to the closing quote.
    let mut out = String::new();
    for ch in trimmed.chars() {
        match ch {
            '\'' | '"' | '\n' | '\r' => break,
            _ => out.push(ch),
        }
    }
    Some(out)
}

fn find_bearer(text: &str) -> Option<String> {
    let lower = text.to_ascii_lowercase();
    let idx = lower.find("authorization:")?;
    let rest = text[idx + "authorization:".len()..].trim_start();
    let rest = rest
        .strip_prefix("Bearer ")
        .or_else(|| rest.strip_prefix("bearer "))
        .unwrap_or(rest);
    let value = read_header_value(rest)?;
    (!value.trim().is_empty()).then(|| value.trim().to_string())
}

/// Split `a=1; b=2; js_auth=xyz; c=3` into the auth token plus the rest.
fn split_cookies(raw: &str) -> (Option<String>, Option<String>) {
    let mut auth = None;
    let mut extra = Vec::new();

    for part in raw.split(';') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let (name, value) = match part.split_once('=') {
            Some((n, v)) => (n.trim(), v.trim()),
            None => continue,
        };
        if name.eq_ignore_ascii_case("js_auth") {
            auth = Some(value.trim_matches('"').to_string());
        } else {
            extra.push(part.to_string());
        }
    }

    let extra = if extra.is_empty() {
        None
    } else {
        Some(extra.join("; "))
    };
    (auth.filter(|v| !v.is_empty()), extra)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_curl() {
        let raw = "curl 'https://wosoracle.com/api/codes' \\
            -H 'accept: */*' \\
            -H 'x-api-token: TOKEN123' \\
            -b 'cf_clearance=abc; js_auth=JWT456; theme=dark'";
        let parsed = parse(raw).expect("parses");
        assert_eq!(parsed.js_auth, "JWT456");
        assert_eq!(parsed.api_token, "TOKEN123");
        assert_eq!(
            parsed.extra_cookie.as_deref(),
            Some("cf_clearance=abc; theme=dark")
        );
    }

    #[test]
    fn pasted_header_block() {
        let raw = "Cookie: js_auth=AAA; cf_clearance=z\nX-Api-Token: BBB";
        let parsed = parse(raw).expect("parses");
        assert_eq!(parsed.js_auth, "AAA");
        assert_eq!(parsed.api_token, "BBB");
    }

    #[test]
    fn json_paste() {
        let raw = r#"{"js_auth":"A","api_token":"B"}"#;
        let parsed = parse(raw).expect("parses");
        assert_eq!(parsed.js_auth, "A");
        assert_eq!(parsed.api_token, "B");
    }

    #[test]
    fn missing_token() {
        let raw = "curl 'https://wosoracle.com/api/codes' -b 'js_auth=X'";
        assert_eq!(parse(raw), Err(CurlError::MissingToken));
    }

    #[test]
    fn missing_cookie() {
        let raw = "curl 'https://wosoracle.com/api/codes' -H 'x-api-token: X'";
        assert_eq!(parse(raw), Err(CurlError::MissingCookie));
    }
}
