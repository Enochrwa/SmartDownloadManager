//! WebDAV client (Sprint 8): `PROPFIND` directory listing and `PUT`
//! upload, built directly on the `reqwest` stack already used for HTTP.
//!
//! WebDAV downloading is deliberately *not* reimplemented here: a
//! `webdav://`/`webdavs://` URL is plain HTTP/HTTPS with a few extra
//! verbs bolted on (`PROPFIND`, `MKCOL`, `PUT`, `LOCK`), so `GET` +
//! `Range` — including segmented, multi-connection, segment-stealing
//! `Range` requests — behaves identically to a normal HTTPS file server.
//! `crate::http::{probe, download_single, download_range}` already cover
//! that; this module's job is [`to_http_url`] (scheme translation) plus
//! the two genuinely WebDAV-specific operations, matching
//! `docs/SPRINT_PLAN_PHASE2.md` Sprint 8's framing that WebDAV "reuses
//! the Sprint 1-2 range-request and segment-splitting logic almost
//! unchanged."

use reqwest::{Client, StatusCode};

use crate::error::{classify_status, classify_transport_error, parse_retry_after};
use crate::http::ProtoError;

/// Translate a `webdav://`/`webdavs://` URL into the `http://`/`https://`
/// URL it actually is on the wire. Plain `http(s)://` URLs pass through
/// unchanged, so callers can accept either spelling.
pub fn to_http_url(url: &str) -> Result<String, ProtoError> {
    if let Some(rest) = url.strip_prefix("webdavs://") {
        Ok(format!("https://{rest}"))
    } else if let Some(rest) = url.strip_prefix("webdav://") {
        Ok(format!("http://{rest}"))
    } else if url.starts_with("http://") || url.starts_with("https://") {
        Ok(url.to_string())
    } else {
        Err(ProtoError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("not a webdav://, webdavs://, http://, or https:// URL: {url}"),
        )))
    }
}

/// One entry from a `PROPFIND` (`Depth: 1`) response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebDavEntry {
    /// Path (not full URL) of the resource, as reported in its `<D:href>`.
    pub href: String,
    pub is_collection: bool,
    pub content_length: Option<u64>,
}

const PROPFIND_BODY: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<D:propfind xmlns:D="DAV:">
  <D:prop>
    <D:resourcetype/>
    <D:getcontentlength/>
  </D:prop>
</D:propfind>"#;

/// List the immediate children of a WebDAV collection (directory) via
/// `PROPFIND` with `Depth: 1`.
pub async fn list_dir(client: &Client, url: &str) -> Result<Vec<WebDavEntry>, ProtoError> {
    let http_url = to_http_url(url)?;
    let method: reqwest::Method = "PROPFIND"
        .parse()
        .expect("PROPFIND is a valid HTTP method token");

    let resp = client
        .request(method, &http_url)
        .header("Depth", "1")
        .header("Content-Type", "application/xml")
        .body(PROPFIND_BODY)
        .send()
        .await
        .map_err(|e| {
            let class = classify_transport_error(&e);
            ProtoError::Transport(e, class)
        })?;

    let status = resp.status();
    if status != StatusCode::MULTI_STATUS && !status.is_success() {
        let retry_after = resp
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|v| v.to_str().ok())
            .and_then(parse_retry_after);
        let class = classify_status(status.as_u16(), retry_after);
        return Err(ProtoError::Http {
            status: status.as_u16(),
            class,
        });
    }

    let body = resp.text().await.map_err(|e| {
        let class = classify_transport_error(&e);
        ProtoError::Transport(e, class)
    })?;

    Ok(parse_multistatus(&body))
}

/// Minimal, dependency-free `multistatus` XML parser: WebDAV servers vary
/// wildly in namespace prefixes (`D:`, `d:`, `lp1:`, none at all), so
/// rather than pull in a full XML crate for a handful of fixed element
/// names, this scans tag *local names* (ignoring any `prefix:`) with
/// simple string search. Robust to attribute variations and whitespace;
/// not a general XML parser (no CDATA/entity handling beyond the basics
/// WebDAV servers actually emit for these elements).
fn parse_multistatus(body: &str) -> Vec<WebDavEntry> {
    let mut entries = Vec::new();
    for response_block in split_on_local_tag(body, "response") {
        let href = extract_local_tag_text(response_block, "href").map(|h| {
            percent_encoding::percent_decode_str(h.trim())
                .decode_utf8_lossy()
                .into_owned()
        });
        let Some(href) = href else { continue };

        let is_collection = find_local_tag_open(response_block, "collection").is_some();
        let content_length = extract_local_tag_text(response_block, "getcontentlength")
            .and_then(|s| s.trim().parse::<u64>().ok());

        entries.push(WebDavEntry {
            href,
            is_collection,
            content_length,
        });
    }
    entries
}

/// Find the byte range `(start_of_"</tag>", end_after_'>')` of the next
/// closing tag for `local_name`, matching on local name regardless of
/// namespace prefix (mirrors [`find_local_tag_open`]'s matching rules).
fn find_local_tag_close(xml: &str, local_name: &str) -> Option<(usize, usize)> {
    let lower = xml.to_lowercase();
    let local_lower = local_name.to_lowercase();
    let mut search_from = 0;
    while let Some(rel) = lower[search_from..].find("</") {
        let abs = search_from + rel;
        let after = &lower[abs + 2..];
        let name_end = after.find('>')?;
        let candidate = &after[..name_end];
        // `candidate` is e.g. "response" or "d:response" (no attributes
        // are legal inside a closing tag, so this is the whole name).
        let local_part = candidate.rsplit(':').next().unwrap_or(candidate);
        if local_part == local_lower {
            return Some((abs, abs + 2 + name_end + 1));
        }
        search_from = abs + 2;
    }
    None
}

/// Split `xml` into the inner contents of every `<[prefix:]tag>...</[prefix:]tag>`
/// block at any nesting depth (non-recursive: nested same-named tags
/// aren't a concern for the fixed `multistatus`/`response` shape WebDAV
/// servers emit).
fn split_on_local_tag<'a>(xml: &'a str, local_name: &str) -> Vec<&'a str> {
    let mut out = Vec::new();
    let mut rest = xml;
    while let Some(open_start) = find_local_tag_open(rest, local_name) {
        let after_open = &rest[open_start..];
        let Some(tag_end) = after_open.find('>') else {
            break;
        };
        let body_start = open_start + tag_end + 1;
        let Some((close_start, close_end)) = find_local_tag_close(&rest[body_start..], local_name)
        else {
            break;
        };
        out.push(&rest[body_start..body_start + close_start]);
        rest = &rest[body_start + close_end..];
    }
    out
}

/// Find the byte offset of the next `<[prefix:]tag` opening occurrence
/// (self-closing or not), matching on local name regardless of namespace
/// prefix, case-insensitively (WebDAV/XML tag names are technically
/// case-sensitive, but real-world servers are inconsistent enough that
/// matching case-insensitively here is far more robust in practice).
fn find_local_tag_open(xml: &str, local_name: &str) -> Option<usize> {
    let lower = xml.to_lowercase();
    let needle_bare = format!("<{}", local_name.to_lowercase());
    let mut search_from = 0;
    while let Some(rel) = lower[search_from..].find('<') {
        let abs = search_from + rel;
        let after = &lower[abs..];
        // Match "<tag" or "<ns:tag" followed by a non-alnum boundary.
        if after.starts_with(&needle_bare) {
            let boundary = after.as_bytes().get(needle_bare.len());
            if matches!(boundary, None | Some(b' ') | Some(b'>') | Some(b'/')) {
                return Some(abs);
            }
        } else if let Some(colon) = after[1..].find(':') {
            let tag_start = abs + 1 + colon + 1;
            if lower[tag_start..].starts_with(&local_name.to_lowercase()) {
                let boundary_idx = tag_start + local_name.len();
                let boundary = lower.as_bytes().get(boundary_idx);
                if matches!(boundary, None | Some(b' ') | Some(b'>') | Some(b'/')) {
                    return Some(abs);
                }
            }
        }
        search_from = abs + 1;
    }
    None
}

/// Extract the text content of the first `<[prefix:]tag>text</[prefix:]tag>`
/// element found in `xml`.
fn extract_local_tag_text<'a>(xml: &'a str, local_name: &str) -> Option<&'a str> {
    let open_start = find_local_tag_open(xml, local_name)?;
    let after_open = &xml[open_start..];
    let tag_end = after_open.find('>')?;
    if after_open.as_bytes()[tag_end - 1] == b'/' {
        return Some(""); // self-closing, e.g. <D:resourcetype/>
    }
    let body_start = open_start + tag_end + 1;
    let (close_start, _) = find_local_tag_close(&xml[body_start..], local_name)?;
    Some(&xml[body_start..body_start + close_start])
}

/// Upload `body` to `url` via `PUT`. WebDAV servers create the resource
/// (and, per RFC 4918, expect intermediate collections to already exist —
/// callers needing `MKCOL` for nested paths should issue it separately).
pub async fn upload(client: &Client, url: &str, body: Vec<u8>) -> Result<u64, ProtoError> {
    let http_url = to_http_url(url)?;
    let len = body.len() as u64;
    let resp = client.put(&http_url).body(body).send().await.map_err(|e| {
        let class = classify_transport_error(&e);
        ProtoError::Transport(e, class)
    })?;

    let status = resp.status();
    if !status.is_success() {
        let retry_after = resp
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|v| v.to_str().ok())
            .and_then(parse_retry_after);
        let class = classify_status(status.as_u16(), retry_after);
        return Err(ProtoError::Http {
            status: status.as_u16(),
            class,
        });
    }
    Ok(len)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn translates_webdav_scheme_to_http() {
        assert_eq!(
            to_http_url("webdav://example.com/dir/file.txt").unwrap(),
            "http://example.com/dir/file.txt"
        );
    }

    #[test]
    fn translates_webdavs_scheme_to_https() {
        assert_eq!(
            to_http_url("webdavs://example.com/file.txt").unwrap(),
            "https://example.com/file.txt"
        );
    }

    #[test]
    fn passes_through_plain_http_https() {
        assert_eq!(
            to_http_url("https://example.com/file.txt").unwrap(),
            "https://example.com/file.txt"
        );
    }

    #[test]
    fn rejects_unrelated_scheme() {
        assert!(to_http_url("ftp://example.com/file").is_err());
    }

    #[test]
    fn parses_multistatus_response_with_namespace_prefixes() {
        let body = r#"<?xml version="1.0"?>
<D:multistatus xmlns:D="DAV:">
  <D:response>
    <D:href>/dav/docs/</D:href>
    <D:propstat>
      <D:prop>
        <D:resourcetype><D:collection/></D:resourcetype>
      </D:prop>
      <D:status>HTTP/1.1 200 OK</D:status>
    </D:propstat>
  </D:response>
  <D:response>
    <D:href>/dav/docs/report.pdf</D:href>
    <D:propstat>
      <D:prop>
        <D:resourcetype/>
        <D:getcontentlength>10485760</D:getcontentlength>
      </D:prop>
      <D:status>HTTP/1.1 200 OK</D:status>
    </D:propstat>
  </D:response>
</D:multistatus>"#;

        let entries = parse_multistatus(body);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].href, "/dav/docs/");
        assert!(entries[0].is_collection);
        assert_eq!(entries[1].href, "/dav/docs/report.pdf");
        assert!(!entries[1].is_collection);
        assert_eq!(entries[1].content_length, Some(10_485_760));
    }

    #[test]
    fn parses_multistatus_without_namespace_prefixes() {
        // Some servers (notably a few embedded/appliance WebDAV
        // implementations) emit unprefixed elements against a default
        // namespace instead of the `D:`/`d:` convention.
        let body = r#"<?xml version="1.0"?>
<multistatus xmlns="DAV:">
  <response>
    <href>/share/file.bin</href>
    <propstat>
      <prop>
        <resourcetype/>
        <getcontentlength>42</getcontentlength>
      </prop>
    </propstat>
  </response>
</multistatus>"#;
        let entries = parse_multistatus(body);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].href, "/share/file.bin");
        assert_eq!(entries[0].content_length, Some(42));
    }
}
