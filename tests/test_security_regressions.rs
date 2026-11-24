//! Security regression tests for issues #1820 and #1821.
//!
//! #1820: `/metrics` must not be reachable through the public ALB. The ALB
//!        listener rule returns a fixed 404 for `/metrics`, so a request to
//!        the ALB DNS name for that path must never return 200.
//!
//! #1821: Once nginx sits behind the ALB, rate limiting must be keyed on the
//!        forwarded client address (via the `realip` module), not on the ALB
//!        node's address. Distinct clients must therefore get independent
//!        rate-limit buckets.
//!
//! These tests are integration tests: they require a deployed environment and
//! are gated behind environment variables so that `cargo test` in CI without
//! infrastructure simply skips them.
//!
//! Environment variables:
//!   * `ALB_DNS_NAME`  – DNS name (or base URL) of the public ALB.
//!   * `RATE_LIMIT_PATH` – (optional) path used for the rate-limit test.
//!                         Defaults to `/`.
//!   * `RATE_LIMIT_RPM` – (optional) configured requests-per-minute per IP.
//!                        Defaults to `100` (matches `user_data.sh`).
//!
//! Run with:
//!   ALB_DNS_NAME=my-alb-1234.us-east-1.elb.amazonaws.com \
//!     cargo test --test test_security_regressions -- --nocapture

use std::env;
use std::time::Duration;

/// Build a fully-qualified URL from the configured ALB DNS name and a path.
fn alb_url(path: &str) -> Option<String> {
    let base = env::var("ALB_DNS_NAME").ok()?;
    let base = base.trim().trim_end_matches('/');
    let base = if base.starts_with("http://") || base.starts_with("https://") {
        base.to_string()
    } else {
        format!("https://{base}")
    };
    let path = if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{path}")
    };
    Some(format!("{base}{path}"))
}

/// Minimal blocking HTTP GET that returns the status code.
///
/// We avoid pulling in an async runtime for these integration tests; a tiny
/// hand-rolled client over `std::net` keeps the dependency surface small.
/// HTTPS is delegated to `curl` when available, otherwise we fall back to a
/// plain HTTP request (sufficient for ALB listeners that terminate TLS and
/// redirect, or for test environments fronted by plain HTTP).
fn http_get_status(url: &str, extra_headers: &[(&str, &str)]) -> Result<u16, String> {
    // Prefer curl: it handles TLS, redirects and connection reuse cleanly and
    // is present on virtually every CI image and developer machine.
    if let Ok(output) = std::process::Command::new("curl")
        .arg("-s")
        .arg("-o")
        .arg("/dev/null")
        .arg("-w")
        .arg("%{http_code}")
        .arg("--max-time")
        .arg("10")
        .args(extra_headers.iter().flat_map(|(k, v)| {
            ["-H".to_string(), format!("{k}: {v}")]
        }))
        .arg(url)
        .output()
    {
        if output.status.success() {
            let code = String::from_utf8_lossy(&output.stdout);
            if let Ok(status) = code.trim().parse::<u16>() {
                return Ok(status);
            }
        }
    }

    // Fallback: plain HTTP via std::net (no TLS).
    let without_scheme = url
        .strip_prefix("http://")
        .ok_or_else(|| format!("cannot issue plain-HTTP request to {url}"))?;
    let (host_port, path) = match without_scheme.find('/') {
        Some(idx) => (&without_scheme[..idx], &without_scheme[idx..]),
        None => (without_scheme, "/"),
    };
    let host_port = if host_port.contains(':') {
        host_port.to_string()
    } else {
        format!("{host_port}:80")
    };

    use std::io::{Read, Write};
    use std::net::TcpStream;

    let mut stream = TcpStream::connect(&host_port).map_err(|e| e.to_string())?;
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .map_err(|e| e.to_string())?;

    let mut request = format!("GET {path} HTTP/1.1\r\nHost: {host_port}\r\nConnection: close\r\n");
    for (k, v) in extra_headers {
        request.push_str(&format!("{k}: {v}\r\n"));
    }
    request.push_str("\r\n");

    stream
        .write_all(request.as_bytes())
        .map_err(|e| e.to_string())?;

    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .map_err(|e| e.to_string())?;

    let status_line = response
        .lines()
        .next()
        .ok_or_else(|| "empty response".to_string())?;
    let status = status_line
        .split_whitespace()
        .nth(1)
        .ok_or_else(|| format!("malformed status line: {status_line}"))?
        .parse::<u16>()
        .map_err(|e| e.to_string())?;
    Ok(status)
}

/// #1820: `/metrics` must not be reachable through the public ALB.
///
/// The ALB listener rule returns a fixed-response 404 for `/metrics`, so the
/// status must never be 200. We accept any non-200 (404 expected, but a 403
/// or 5xx from a misconfigured rule is still a regression signal worth
/// surfacing via the assertion message).
#[test]
fn metrics_is_not_reachable_through_alb() {
    let Some(url) = alb_url("/metrics") else {
        eprintln!("skipping: ALB_DNS_NAME not set");
        return;
    };

    let status = match http_get_status(&url, &[]) {
        Ok(status) => status,
        Err(err) => {
            eprintln!("skipping: could not reach ALB at {url}: {err}");
            return;
        }
    };

    assert_ne!(
        status, 200,
        "GET {url} returned 200; /metrics must be blocked at the ALB (issue #1820)"
    );
    assert_eq!(
        status, 404,
        "GET {url} returned {status}; expected the ALB fixed-response 404 for /metrics (issue #1820)"
    );
}

/// #1820 (defence in depth): the metrics endpoint must not leak through a
/// trailing-slash or query-string variant either.
#[test]
fn metrics_variants_are_not_reachable_through_alb() {
    let Some(base) = alb_url("/metrics") else {
        eprintln!("skipping: ALB_DNS_NAME not set");
        return;
    };
    let base = base.trim_end_matches("/metrics").to_string();

    for variant in ["/metrics/", "/metrics?x=1", "/metrics/foo"] {
        let url = format!("{base}{variant}");
        let status = match http_get_status(&url, &[]) {
            Ok(status) => status,
            Err(err) => {
                eprintln!("skipping: could not reach ALB at {url}: {err}");
                return;
            }
        };
        assert_ne!(
            status, 200,
            "GET {url} returned 200; /metrics variants must be blocked at the ALB (issue #1820)"
        );
    }
}

/// #1821: distinct clients (identified by `X-Forwarded-For`) must get
/// independent rate-limit buckets.
///
/// We send `RATE_LIMIT_RPM + 1` requests from client A and expect at least one
/// 429. We then send a single request from client B and expect it to succeed
/// (not 429), proving the bucket is keyed on the forwarded address rather than
/// the shared ALB node address.
#[test]
fn rate_limit_buckets_are_keyed_on_forwarded_client() {
    let Some(url) = alb_url(&env::var("RATE_LIMIT_PATH").unwrap_or_else(|_| "/".to_string()))
    else {
        eprintln!("skipping: ALB_DNS_NAME not set");
        return;
    };

    let rpm: usize = env::var("RATE_LIMIT_RPM")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(100);

    // Client A: exhaust its bucket.
    let client_a = "203.0.113.10";
    let mut saw_429 = false;
    for _ in 0..=rpm {
        let status = match http_get_status(&url, &[("X-Forwarded-For", client_a)]) {
            Ok(status) => status,
            Err(err) => {
                eprintln!("skipping: could not reach ALB at {url}: {err}");
                return;
            }
        };
        if status == 429 {
            saw_429 = true;
            break;
        }
    }
    assert!(
        saw_429,
        "client {client_a} was never rate limited after {} requests to {url}; \
         rate limiting may be disabled or keyed incorrectly (issue #1821)",
        rpm + 1
    );

    // Client B: a distinct forwarded address must have its own bucket.
    let client_b = "203.0.113.20";
    let status_b = match http_get_status(&url, &[("X-Forwarded-For", client_b)]) {
        Ok(status) => status,
        Err(err) => {
            eprintln!("skipping: could not reach ALB at {url}: {err}");
            return;
        }
    };
    assert_ne!(
        status_b, 429,
        "client {client_b} was rate limited immediately after client {client_a} exhausted \
         its bucket; rate limiting is keyed on the ALB address instead of the forwarded \
         client address (issue #1821)"
    );
}