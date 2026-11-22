#![cfg(not(target_arch = "wasm32"))]
#![deny(unsafe_code)]

use std::collections::HashMap;
use std::fs;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

#[derive(Debug, Clone)]
struct SmokeConfig {
    target_url: Option<String>,
    target_addr: Option<String>,
    bearer_token: Option<String>,
    tls_ca_pem: Option<Vec<u8>>,
    tls_cert_pem: Option<Vec<u8>>,
    tls_key_pem: Option<Vec<u8>>,
    timeout: Duration,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ParsedTargetUrl {
    connect_host: String,
    port: u16,
    base_path: String,
    host_header: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ParsedConsoleAssets {
    css_href: String,
    js_src: String,
    wasm_src: String,
    css_version: String,
    wasm_version: String,
}

#[derive(Debug, Clone)]
struct HttpResponse {
    status: u16,
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

fn read_trimmed_secret_file(var_name: &str, path_raw: &str) -> Result<String, String> {
    let path = path_raw.trim();
    if path.is_empty() {
        return Err(format!("{var_name} was set to an empty path"));
    }
    let raw = fs::read_to_string(path)
        .map_err(|e| format!("Failed to read {var_name} at '{path}': {e}"))?;
    let trimmed = raw.trim().to_string();
    if trimmed.is_empty() {
        return Err(format!("{var_name} at '{path}' is empty"));
    }
    Ok(trimmed)
}

fn read_non_empty_file_bytes(var_name: &str, path_raw: &str) -> Result<Vec<u8>, String> {
    let path = path_raw.trim();
    if path.is_empty() {
        return Err(format!("{var_name} was set to an empty path"));
    }
    let bytes =
        fs::read(path).map_err(|e| format!("Failed to read {var_name} at '{path}': {e}"))?;
    if bytes.is_empty() {
        return Err(format!("{var_name} at '{path}' is empty"));
    }
    Ok(bytes)
}

fn load_smoke_config_with<F>(get_env: F) -> Result<SmokeConfig, String>
where
    F: Fn(&str) -> Option<String>,
{
    let target_url = get_env("MU_SMOKE_TARGET_URL")
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let target_addr = get_env("MU_SMOKE_TARGET_ADDR")
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    let bearer_token = match get_env("MU_SMOKE_BEARER_TOKEN_FILE")
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
    {
        Some(path) => Some(read_trimmed_secret_file(
            "MU_SMOKE_BEARER_TOKEN_FILE",
            &path,
        )?),
        None => None,
    };

    let tls_ca_pem = match get_env("MU_SMOKE_TLS_CA_FILE")
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
    {
        Some(path) => Some(read_non_empty_file_bytes("MU_SMOKE_TLS_CA_FILE", &path)?),
        None => None,
    };

    let cert_path = get_env("MU_SMOKE_TLS_CERT_FILE")
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let key_path = get_env("MU_SMOKE_TLS_KEY_FILE")
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    let (tls_cert_pem, tls_key_pem) = match (cert_path, key_path) {
        (Some(cert), Some(key)) => (
            Some(read_non_empty_file_bytes("MU_SMOKE_TLS_CERT_FILE", &cert)?),
            Some(read_non_empty_file_bytes("MU_SMOKE_TLS_KEY_FILE", &key)?),
        ),
        (Some(_), None) | (None, Some(_)) => {
            return Err(
                "MU_SMOKE_TLS_CERT_FILE and MU_SMOKE_TLS_KEY_FILE must both be set together"
                    .to_string(),
            );
        }
        (None, None) => (None, None),
    };

    let timeout_secs = match get_env("MU_SMOKE_TIMEOUT_SECS")
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
    {
        Some(raw) => {
            let secs = raw.parse::<u64>().map_err(|e| {
                format!("Invalid MU_SMOKE_TIMEOUT_SECS '{raw}': expected positive integer ({e})")
            })?;
            if secs == 0 {
                return Err("MU_SMOKE_TIMEOUT_SECS must be greater than zero".to_string());
            }
            secs
        }
        None => 10,
    };

    if target_addr.is_some() && target_url.is_none() {
        return Err(
            "MU_SMOKE_TARGET_ADDR requires MU_SMOKE_TARGET_URL to supply the HTTP Host header and path"
                .to_string(),
        );
    }

    Ok(SmokeConfig {
        target_url,
        target_addr,
        bearer_token,
        tls_ca_pem,
        tls_cert_pem,
        tls_key_pem,
        timeout: Duration::from_secs(timeout_secs),
    })
}

fn load_smoke_config() -> Result<SmokeConfig, String> {
    load_smoke_config_with(|k| std::env::var(k).ok())
}

/// Parses `http://<host>[:<port>][<base_path>][#host=<vhost>]`.
///
/// An optional `#host=<vhost>` fragment overrides the HTTP/1.1 `Host:` header while keeping
/// the TCP dial target pinned to `<host>:<port>`, allowing direct-IP probing of multi-vhost
/// reverse proxies without embedding hostnames into test code.
fn parse_http_url(raw: &str) -> Result<ParsedTargetUrl, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.chars().any(char::is_whitespace) {
        return Err(format!("Invalid HTTP target URL '{raw}'"));
    }

    let (without_fragment, host_override) = match trimmed.split_once('#') {
        Some((base, frag)) => {
            let vhost = frag.strip_prefix("host=").ok_or_else(|| {
                format!("Unsupported URL fragment '#{frag}' (expected #host=<vhost>)")
            })?;
            if vhost.is_empty()
                || vhost.contains('\r')
                || vhost.contains('\n')
                || vhost.chars().any(char::is_whitespace)
            {
                return Err(format!("Invalid virtual host override '{vhost}'"));
            }
            (base, Some(vhost.to_string()))
        }
        None => (trimmed, None),
    };

    let rest = without_fragment
        .strip_prefix("http://")
        .ok_or_else(|| format!("MU_SMOKE_TARGET_URL must start with http:// (got '{raw}')"))?;
    let (authority, path_raw) = match rest.find('/') {
        Some(idx) => (&rest[..idx], &rest[idx..]),
        None => (rest, ""),
    };
    if authority.is_empty() {
        return Err(format!("Missing host in target URL '{raw}'"));
    }

    let (connect_host, port) = if let Some(bracket_end) = authority.rfind(']') {
        let host_part = &authority[..=bracket_end];
        let after = &authority[bracket_end + 1..];
        let port = if let Some(port_str) = after.strip_prefix(':') {
            port_str
                .parse::<u16>()
                .map_err(|e| format!("Invalid port '{port_str}' in '{raw}': {e}"))?
        } else if after.is_empty() {
            80
        } else {
            return Err(format!("Malformed IPv6 authority '{authority}' in '{raw}'"));
        };
        (host_part.to_string(), port)
    } else if let Some((h, p)) = authority.rsplit_once(':') {
        let port = p
            .parse::<u16>()
            .map_err(|e| format!("Invalid port '{p}' in '{raw}': {e}"))?;
        if h.is_empty() {
            return Err(format!("Empty host in '{raw}'"));
        }
        (h.to_string(), port)
    } else {
        (authority.to_string(), 80)
    };

    if port == 0 {
        return Err(format!("Target URL port must be non-zero in '{raw}'"));
    }

    let base_path = path_raw.trim_end_matches('/').to_string();
    let host_header = host_override.unwrap_or_else(|| {
        if port == 80 {
            connect_host.clone()
        } else {
            format!("{connect_host}:{port}")
        }
    });

    Ok(ParsedTargetUrl {
        connect_host,
        port,
        base_path,
        host_header,
    })
}

/// Parses an explicit `host:port` (or `[ipv6]:port`) TCP dial address supplied via
/// `MU_SMOKE_TARGET_ADDR`.
fn parse_dial_addr(raw: &str) -> Result<(String, u16), String> {
    let trimmed = raw.trim();
    if trimmed.is_empty()
        || trimmed.chars().any(char::is_whitespace)
        || trimmed.contains("://")
        || trimmed.contains('/')
    {
        return Err(format!("Invalid MU_SMOKE_TARGET_ADDR '{raw}'"));
    }

    let (host, port_str) = if let Some(bracket_end) = trimmed.rfind(']') {
        let host_part = &trimmed[..=bracket_end];
        let after = &trimmed[bracket_end + 1..];
        let port_str = after.strip_prefix(':').ok_or_else(|| {
            format!("MU_SMOKE_TARGET_ADDR '{raw}' must include :<port> after IPv6 address")
        })?;
        (host_part, port_str)
    } else {
        trimmed
            .rsplit_once(':')
            .ok_or_else(|| format!("MU_SMOKE_TARGET_ADDR '{raw}' must be in host:port form"))?
    };

    if host.is_empty() {
        return Err(format!("Empty host in MU_SMOKE_TARGET_ADDR '{raw}'"));
    }
    let port = port_str
        .parse::<u16>()
        .map_err(|e| format!("Invalid port '{port_str}' in MU_SMOKE_TARGET_ADDR '{raw}': {e}"))?;
    if port == 0 {
        return Err(format!(
            "MU_SMOKE_TARGET_ADDR port must be non-zero in '{raw}'"
        ));
    }

    Ok((host.to_string(), port))
}

/// Resolves the final HTTP target, applying an optional `MU_SMOKE_TARGET_ADDR` override for
/// the TCP dial endpoint while preserving the `Host:` header derived from `MU_SMOKE_TARGET_URL`.
fn resolve_target(target_url: &str, target_addr: Option<&str>) -> Result<ParsedTargetUrl, String> {
    let mut parsed = parse_http_url(target_url)?;
    if let Some(addr) = target_addr {
        let (dial_host, dial_port) = parse_dial_addr(addr)?;
        parsed.connect_host = dial_host;
        parsed.port = dial_port;
    }
    Ok(parsed)
}

fn extract_quoted_asset(html: &str, needle: &str) -> Result<String, String> {
    let pos = html
        .find(needle)
        .ok_or_else(|| format!("index.html missing expected asset reference '{needle}'"))?;
    let prefix = &html[..pos];
    let start = prefix
        .rfind(['"', '\''])
        .map(|i| i + 1)
        .ok_or_else(|| format!("Unterminated opening quote before '{needle}'"))?;
    let rest = &html[start..];
    let end = rest
        .find(['"', '\''])
        .ok_or_else(|| format!("Unterminated closing quote after '{needle}'"))?;
    Ok(rest[..end].to_string())
}

fn extract_version_tag(asset_ref: &str) -> Result<String, String> {
    let (_, tag) = asset_ref.split_once("?v=").ok_or_else(|| {
        format!("Asset reference '{asset_ref}' missing '?v=<hash>' query busting")
    })?;
    if tag.len() < 6 || !tag.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(format!(
            "Asset reference '{asset_ref}' has invalid hex version hash '{tag}'"
        ));
    }
    Ok(tag.to_string())
}

fn parse_hashed_console_assets(index_html: &str) -> Result<ParsedConsoleAssets, String> {
    let css_href = extract_quoted_asset(index_html, "tokens.css?v=")?;
    let js_src = extract_quoted_asset(index_html, "muconsole-ui.js?v=")?;
    let wasm_src = extract_quoted_asset(index_html, "muconsole-ui_bg.wasm?v=")?;

    let css_version = extract_version_tag(&css_href)?;
    let js_version = extract_version_tag(&js_src)?;
    let wasm_version = extract_version_tag(&wasm_src)?;
    if js_version != wasm_version {
        return Err(format!(
            "Mismatched JS/WASM cache-busting versions: js={js_version}, wasm={wasm_version}"
        ));
    }

    Ok(ParsedConsoleAssets {
        css_href,
        js_src,
        wasm_src,
        css_version,
        wasm_version,
    })
}

fn resolve_asset_request_path(base_path: &str, asset_ref: &str) -> String {
    let stripped = asset_ref.trim_start_matches("./").trim_start_matches('/');
    if base_path.is_empty() {
        format!("/{stripped}")
    } else {
        format!("{base_path}/{stripped}")
    }
}

fn assert_cache_control_tokens(header_val: &str, required: &[&str], path: &str) {
    let tokens: Vec<String> = header_val
        .split(',')
        .map(|s| s.trim().to_ascii_lowercase())
        .filter(|s| !s.is_empty())
        .collect();
    for expected in required {
        assert!(
            tokens.iter().any(|t| t == expected),
            "Cache-Control for '{path}' ('{header_val}') missing required directive '{expected}'"
        );
    }
}

fn decode_chunked_body(raw: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    let mut idx = 0usize;
    while idx < raw.len() {
        let Some(rel_crlf) = raw[idx..].windows(2).position(|w| w == b"\r\n") else {
            return Err("Malformed chunked HTTP response: missing chunk size CRLF".to_string());
        };
        let size_line = std::str::from_utf8(&raw[idx..idx + rel_crlf])
            .map_err(|e| format!("Invalid UTF-8 chunk size line: {e}"))?;
        let hex_part = size_line.split(';').next().unwrap_or("").trim();
        let chunk_len = usize::from_str_radix(hex_part, 16)
            .map_err(|e| format!("Invalid chunk length '{hex_part}': {e}"))?;
        idx += rel_crlf + 2;
        if chunk_len == 0 {
            break;
        }
        if idx + chunk_len + 2 > raw.len() {
            return Err("Truncated chunked HTTP response payload".to_string());
        }
        out.extend_from_slice(&raw[idx..idx + chunk_len]);
        idx += chunk_len + 2;
    }
    Ok(out)
}

fn http_get(
    target: &ParsedTargetUrl,
    path: &str,
    bearer_token: Option<&str>,
    timeout: Duration,
) -> Result<HttpResponse, String> {
    let connect_addr = format!("{}:{}", target.connect_host, target.port);
    let mut stream = TcpStream::connect(&connect_addr)
        .map_err(|e| format!("TCP connect to {connect_addr} failed: {e}"))?;
    stream
        .set_read_timeout(Some(timeout))
        .map_err(|e| format!("set_read_timeout failed: {e}"))?;
    stream
        .set_write_timeout(Some(timeout))
        .map_err(|e| format!("set_write_timeout failed: {e}"))?;

    let mut req = format!(
        "GET {path} HTTP/1.1\r\nHost: {}\r\nUser-Agent: mu-console-smoke/1.0\r\nConnection: close\r\n",
        target.host_header
    );
    if let Some(tok) = bearer_token {
        req.push_str(&format!("Authorization: Bearer {tok}\r\n"));
    }
    req.push_str("\r\n");

    stream
        .write_all(req.as_bytes())
        .map_err(|e| format!("Failed writing HTTP GET {path}: {e}"))?;

    let mut raw = Vec::new();
    stream
        .read_to_end(&mut raw)
        .map_err(|e| format!("Failed reading HTTP response for {path}: {e}"))?;

    let header_end = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or_else(|| format!("Malformed HTTP response for {path}: missing header terminator"))?;
    let header_str = std::str::from_utf8(&raw[..header_end])
        .map_err(|e| format!("Non-UTF-8 HTTP headers for {path}: {e}"))?;

    let mut lines = header_str.split("\r\n");
    let status_line = lines
        .next()
        .ok_or_else(|| format!("Empty HTTP response for {path}"))?;
    let status = status_line
        .split_whitespace()
        .nth(1)
        .ok_or_else(|| format!("Malformed HTTP status line '{status_line}'"))?
        .parse::<u16>()
        .map_err(|e| format!("Invalid HTTP status code in '{status_line}': {e}"))?;

    let mut headers = HashMap::new();
    for line in lines {
        if let Some((k, v)) = line.split_once(':') {
            headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
        }
    }

    let raw_body = &raw[header_end + 4..];
    let body = if headers
        .get("transfer-encoding")
        .is_some_and(|v| v.to_ascii_lowercase().contains("chunked"))
    {
        decode_chunked_body(raw_body)?
    } else {
        raw_body.to_vec()
    };

    Ok(HttpResponse {
        status,
        headers,
        body,
    })
}

fn spawn_ephemeral_console_server(
    expected_vhost: &'static str,
) -> (SocketAddr, Arc<AtomicBool>, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral loopback listener");
    listener
        .set_nonblocking(true)
        .expect("set listener nonblocking");
    let addr = listener.local_addr().expect("listener local_addr");
    let stop = Arc::new(AtomicBool::new(false));
    let stop_flag = Arc::clone(&stop);

    let index_html = concat!(
        "<!DOCTYPE html><html lang=\"en\"><head>",
        "<meta charset=\"utf-8\" />",
        "<meta http-equiv=\"Cache-Control\" content=\"no-cache, no-store, must-revalidate\" />",
        "<title>MuConsole</title>",
        "<link rel=\"stylesheet\" href=\"/tokens.css?v=b83ac1a45a\" />",
        "</head><body><script type=\"module\">",
        "import init from '/muconsole-ui.js?v=f761a5171e';",
        "init({ module_or_path: '/muconsole-ui_bg.wasm?v=f761a5171e' });",
        "</script></body></html>"
    )
    .as_bytes()
    .to_vec();
    let css_bytes = b":root { --mu-bg: #0e1116; }\n".to_vec();
    let js_bytes = b"export default async function init() { return true; }\n".to_vec();
    let wasm_bytes = b"\x00asm\x01\x00\x00\x00\x01\x04\x01\x60\x00\x00".to_vec();

    let handle = thread::spawn(move || {
        while !stop_flag.load(Ordering::Relaxed) {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    let _ = stream.set_nonblocking(false);
                    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
                    let mut buf = [0u8; 4096];
                    let n = stream.read(&mut buf).unwrap_or(0);
                    let req = String::from_utf8_lossy(&buf[..n]);
                    let mut lines = req.lines();
                    let first_line = lines.next().unwrap_or("");
                    let path = first_line.split_whitespace().nth(1).unwrap_or("/");
                    let req_host = lines
                        .find_map(|l| {
                            let (k, v) = l.split_once(':')?;
                            k.trim()
                                .eq_ignore_ascii_case("host")
                                .then(|| v.trim().to_string())
                        })
                        .unwrap_or_default();

                    if !req_host.eq_ignore_ascii_case(expected_vhost) {
                        let wrong = b"<html><body>default-other-vhost</body></html>";
                        let hdr = format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nCache-Control: no-cache, no-store, must-revalidate\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            wrong.len()
                        );
                        let _ = stream.write_all(hdr.as_bytes());
                        let _ = stream.write_all(wrong);
                        continue;
                    }

                    let (status_line, content_type, cache_control, body): (
                        &str,
                        &str,
                        &str,
                        &[u8],
                    ) = match path {
                        "/" | "/index.html" => (
                            "HTTP/1.1 200 OK",
                            "text/html; charset=utf-8",
                            "no-cache, no-store, must-revalidate",
                            &index_html,
                        ),
                        "/tokens.css?v=b83ac1a45a" => (
                            "HTTP/1.1 200 OK",
                            "text/css; charset=utf-8",
                            "no-cache, must-revalidate",
                            &css_bytes,
                        ),
                        "/muconsole-ui.js?v=f761a5171e" => (
                            "HTTP/1.1 200 OK",
                            "application/javascript; charset=utf-8",
                            "no-cache, must-revalidate",
                            &js_bytes,
                        ),
                        "/muconsole-ui_bg.wasm?v=f761a5171e" => (
                            "HTTP/1.1 200 OK",
                            "application/wasm",
                            "no-cache, must-revalidate",
                            &wasm_bytes,
                        ),
                        _ => (
                            "HTTP/1.1 404 Not Found",
                            "text/plain",
                            "no-store",
                            b"not found",
                        ),
                    };

                    let header = format!(
                        "{status_line}\r\nContent-Type: {content_type}\r\nCache-Control: {cache_control}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = stream.write_all(header.as_bytes());
                    let _ = stream.write_all(body);
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(5));
                }
                Err(_) => break,
            }
        }
    });

    (addr, stop, handle)
}

#[test]
fn smoke_mu_console_serves_index_and_hashed_wasm_assets_with_dual_layer_cache() {
    let cfg = load_smoke_config().expect("valid MU_SMOKE_* environment configuration");

    let (target_url, target_addr, mock_guard) = match cfg.target_url.clone() {
        Some(url) => (url, cfg.target_addr.clone(), None),
        None => {
            let (addr, stop, handle) = spawn_ephemeral_console_server("console.example.test");
            (
                "http://console.example.test".to_string(),
                Some(addr.to_string()),
                Some((stop, handle)),
            )
        }
    };

    let target = resolve_target(&target_url, target_addr.as_deref())
        .expect("valid http:// MU_SMOKE_TARGET_URL and optional MU_SMOKE_TARGET_ADDR");
    let root_path = if target.base_path.is_empty() {
        "/".to_string()
    } else {
        format!("{}/", target.base_path)
    };

    let root_resp = http_get(
        &target,
        &root_path,
        cfg.bearer_token.as_deref(),
        cfg.timeout,
    )
    .expect("GET / must succeed");
    assert_eq!(root_resp.status, 200, "GET / must return 200 OK");
    let root_cc = root_resp
        .headers
        .get("cache-control")
        .expect("GET / response must include Cache-Control header");
    assert_cache_control_tokens(root_cc, &["no-cache", "no-store", "must-revalidate"], "/");

    let html = String::from_utf8(root_resp.body).expect("index.html response must be UTF-8");
    assert!(
        html.contains("MuConsole"),
        "Served index.html must identify MuConsole"
    );
    let assets = parse_hashed_console_assets(&html)
        .expect("Served MuConsole index.html must reference content-hashed CSS/JS/WASM assets");

    let css_path = resolve_asset_request_path(&target.base_path, &assets.css_href);
    let css_resp = http_get(&target, &css_path, cfg.bearer_token.as_deref(), cfg.timeout)
        .expect("GET hashed tokens.css must succeed");
    assert_eq!(css_resp.status, 200, "GET {css_path} must return 200 OK");
    assert!(!css_resp.body.is_empty(), "CSS asset must not be empty");
    let css_cc = css_resp
        .headers
        .get("cache-control")
        .expect("Hashed CSS response must include Cache-Control header");
    assert_cache_control_tokens(css_cc, &["no-cache", "must-revalidate"], &css_path);

    let js_path = resolve_asset_request_path(&target.base_path, &assets.js_src);
    let js_resp = http_get(&target, &js_path, cfg.bearer_token.as_deref(), cfg.timeout)
        .expect("GET hashed muconsole-ui.js must succeed");
    assert_eq!(js_resp.status, 200, "GET {js_path} must return 200 OK");
    assert!(!js_resp.body.is_empty(), "JS asset must not be empty");
    let js_cc = js_resp
        .headers
        .get("cache-control")
        .expect("Hashed JS response must include Cache-Control header");
    assert_cache_control_tokens(js_cc, &["no-cache", "must-revalidate"], &js_path);

    let wasm_path = resolve_asset_request_path(&target.base_path, &assets.wasm_src);
    let wasm_resp = http_get(
        &target,
        &wasm_path,
        cfg.bearer_token.as_deref(),
        cfg.timeout,
    )
    .expect("GET hashed muconsole-ui_bg.wasm must succeed");
    assert_eq!(wasm_resp.status, 200, "GET {wasm_path} must return 200 OK");
    assert!(
        wasm_resp.body.starts_with(b"\x00asm\x01\x00\x00\x00"),
        "Served WASM binary must start with valid WebAssembly magic header"
    );
    let wasm_cc = wasm_resp
        .headers
        .get("cache-control")
        .expect("Hashed WASM response must include Cache-Control header");
    assert_cache_control_tokens(wasm_cc, &["no-cache", "must-revalidate"], &wasm_path);

    if let Some((stop, handle)) = mock_guard {
        stop.store(true, Ordering::Relaxed);
        let _ = handle.join();
    }
}

#[test]
fn smoke_mu_console_fail_closed_config_and_parsers() {
    let missing_tok = load_smoke_config_with(|k| match k {
        "MU_SMOKE_BEARER_TOKEN_FILE" => Some("/nonexistent/mu-console-smoke-token".to_string()),
        _ => None,
    })
    .unwrap_err();
    assert!(missing_tok.contains("MU_SMOKE_BEARER_TOKEN_FILE"));

    let missing_ca = load_smoke_config_with(|k| match k {
        "MU_SMOKE_TLS_CA_FILE" => Some("/nonexistent/mu-console-smoke-ca.pem".to_string()),
        _ => None,
    })
    .unwrap_err();
    assert!(missing_ca.contains("MU_SMOKE_TLS_CA_FILE"));

    let partial_mtls = load_smoke_config_with(|k| match k {
        "MU_SMOKE_TLS_CERT_FILE" => Some("/tmp/only-cert.pem".to_string()),
        _ => None,
    })
    .unwrap_err();
    assert!(partial_mtls.contains("MU_SMOKE_TLS_KEY_FILE"));

    let zero_timeout = load_smoke_config_with(|k| match k {
        "MU_SMOKE_TIMEOUT_SECS" => Some("0".to_string()),
        _ => None,
    })
    .unwrap_err();
    assert!(zero_timeout.contains("greater than zero"));

    let addr_without_url = load_smoke_config_with(|k| match k {
        "MU_SMOKE_TARGET_ADDR" => Some("192.0.2.10:80".to_string()),
        _ => None,
    })
    .unwrap_err();
    assert!(addr_without_url.contains("MU_SMOKE_TARGET_URL"));

    let parsed_vhost = parse_http_url("http://192.0.2.10:8080#host=console.example.test").unwrap();
    assert_eq!(parsed_vhost.connect_host, "192.0.2.10");
    assert_eq!(parsed_vhost.port, 8080);
    assert_eq!(parsed_vhost.host_header, "console.example.test");
    assert_eq!(parsed_vhost.base_path, "");

    let resolved_80 = resolve_target("http://console.example.test", Some("192.0.2.10:80")).unwrap();
    assert_eq!(resolved_80.connect_host, "192.0.2.10");
    assert_eq!(resolved_80.port, 80);
    assert_eq!(resolved_80.host_header, "console.example.test");
    assert_eq!(resolved_80.base_path, "");

    let resolved_8080 =
        resolve_target("http://console.example.test:8080", Some("192.0.2.11:8080")).unwrap();
    assert_eq!(resolved_8080.connect_host, "192.0.2.11");
    assert_eq!(resolved_8080.port, 8080);
    assert_eq!(resolved_8080.host_header, "console.example.test:8080");

    let resolved_v6 =
        resolve_target("http://console.example.test", Some("[2001:db8::1]:8080")).unwrap();
    assert_eq!(resolved_v6.connect_host, "[2001:db8::1]");
    assert_eq!(resolved_v6.port, 8080);
    assert_eq!(resolved_v6.host_header, "console.example.test");

    assert!(parse_dial_addr("192.0.2.10").is_err());
    assert!(parse_dial_addr("192.0.2.10:0").is_err());
    assert!(parse_dial_addr("http://192.0.2.10:80").is_err());
    assert!(parse_dial_addr("192.0.2.10:80/path").is_err());

    let parsed_plain = parse_http_url("http://192.0.2.10:8080/console/").unwrap();
    assert_eq!(parsed_plain.host_header, "192.0.2.10:8080");
    assert_eq!(parsed_plain.base_path, "/console");

    assert!(parse_http_url("https://192.0.2.10:443").is_err());
    assert!(parse_http_url("http://192.0.2.10:0").is_err());
    assert!(parse_http_url("http://192.0.2.10:8080#bad=value").is_err());
    assert!(parse_http_url("http://192.0.2.10:8080#host=").is_err());

    let sample_html = r#"
        <link rel="stylesheet" href="/tokens.css?v=b83ac1a45a" />
        <script type="module">
          import init from '/muconsole-ui.js?v=f761a5171e';
          init({ module_or_path: '/muconsole-ui_bg.wasm?v=f761a5171e' });
        </script>
    "#;
    let assets = parse_hashed_console_assets(sample_html).unwrap();
    assert_eq!(assets.css_version, "b83ac1a45a");
    assert_eq!(assets.wasm_version, "f761a5171e");
    assert_eq!(
        resolve_asset_request_path("", &assets.wasm_src),
        "/muconsole-ui_bg.wasm?v=f761a5171e"
    );

    let mismatched_html = sample_html.replace(
        "muconsole-ui_bg.wasm?v=f761a5171e",
        "muconsole-ui_bg.wasm?v=deadbeef00",
    );
    assert!(parse_hashed_console_assets(&mismatched_html).is_err());

    let tmp_dir = std::env::temp_dir().join(format!("mu-console-smoke-{}", std::process::id()));
    let _ = fs::create_dir_all(&tmp_dir);
    let tok_path = tmp_dir.join("token.txt");
    fs::write(&tok_path, "  smoke-console-tok  \n").unwrap();
    let ok_cfg = load_smoke_config_with(|k| match k {
        "MU_SMOKE_BEARER_TOKEN_FILE" => Some(tok_path.to_string_lossy().into_owned()),
        _ => None,
    })
    .unwrap();
    assert_eq!(ok_cfg.bearer_token.as_deref(), Some("smoke-console-tok"));
    assert!(ok_cfg.tls_ca_pem.is_none());
    assert!(ok_cfg.tls_cert_pem.is_none());
    assert!(ok_cfg.tls_key_pem.is_none());
    assert_eq!(ok_cfg.timeout, Duration::from_secs(10));
    let _ = fs::remove_dir_all(&tmp_dir);
}
