//! Metalink (`.metalink`/`.meta4`, RFC 5854) parsing (Sprint 9).
//!
//! A Metalink document is just a structured way to describe "one file,
//! several mirror URLs, one or more pre-supplied hashes" — exactly the
//! shape `crates/engine`'s mirror-failover and checksum-verification
//! machinery (Sprint 4) already consumes. This module's only job is
//! turning the XML into that shape; no new download logic is needed (see
//! `docs/SPRINT_PLAN_PHASE2.md` Sprint 9).
//!
//! Like `crate::webdav`'s `multistatus` parser, this is a small
//! dependency-free tag scanner rather than a full XML parser: Metalink's
//! element set is fixed and shallow enough that pulling in a full XML
//! crate for it isn't worth the extra dependency, and real-world
//! generators vary in namespace prefix (`metalink:`, `m:`, none at all)
//! the same way WebDAV servers do.

use crate::http::ProtoError;

/// One `<hash>` entry for a file: `type` is the algorithm name as it
/// appears in the document (e.g. `"sha-256"`, `"sha256"`, `"md5"`),
/// lowercased; `hex` is the lowercased hex digest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetalinkHash {
    pub algorithm: String,
    pub hex: String,
}

/// One `<url>` mirror entry for a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetalinkUrl {
    pub url: String,
    /// Lower number = higher priority per RFC 5854 §4.1.4; `None` when
    /// the document didn't supply one (all such URLs are treated as
    /// equal, lowest priority, and ordered after any explicitly
    /// prioritized ones).
    pub priority: Option<u32>,
}

/// One `<file>` entry: a single logical download with its mirrors and
/// verification hashes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetalinkFile {
    pub name: String,
    pub size: Option<u64>,
    pub hashes: Vec<MetalinkHash>,
    /// Mirror URLs, already sorted best (lowest priority number) first.
    pub urls: Vec<MetalinkUrl>,
}

impl MetalinkFile {
    /// The strongest hash present, preferring algorithms in the order
    /// SHA-512 > SHA-256 > SHA-1 > MD5 (weakest-last, matching Sprint 4's
    /// `ChecksumAlgorithm` set — `crc32` never appears in Metalink
    /// documents in practice, so it isn't considered here).
    pub fn strongest_hash(&self) -> Option<&MetalinkHash> {
        const RANK: &[&str] = &["sha512", "sha256", "sha1", "md5"];
        RANK.iter().find_map(|want| {
            self.hashes
                .iter()
                .find(|h| normalize_algo(&h.algorithm) == *want)
        })
    }
}

/// Normalize a Metalink hash `type` string to the spelling
/// `crate::verify::ChecksumAlgorithm::parse` (in `crates/engine`) expects:
/// strip hyphens and lowercase, so `"sha-256"`/`"SHA256"`/`"sha256"` all
/// map to `"sha256"`.
pub fn normalize_algo(algo: &str) -> String {
    algo.to_ascii_lowercase().replace('-', "")
}

/// Parse a Metalink XML document (`.metalink` v3 or `.meta4` v4 — the
/// element names this parser looks at are the same in both) into its list
/// of files. A document with no `<file>` elements at all is treated as
/// malformed input, not "zero files"; a `<file>` with no `<url>` mirrors
/// is skipped (nothing to download from), since resuming a job with a
/// destination but no reachable source would just fail later anyway.
pub fn parse(xml: &str) -> Result<Vec<MetalinkFile>, ProtoError> {
    let file_blocks = split_on_local_tag(xml, "file");
    if file_blocks.is_empty() {
        return Err(ProtoError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "no <file> elements found in Metalink document",
        )));
    }

    let mut files = Vec::with_capacity(file_blocks.len());
    for (block, attrs) in &file_blocks {
        let name = attrs
            .get("name")
            .cloned()
            .unwrap_or_else(|| "download".to_string());

        let size = extract_local_tag_text(block, "size").and_then(|s| s.trim().parse::<u64>().ok());

        let mut hashes = Vec::new();
        for (hash_block, hash_attrs) in split_on_local_tag(block, "hash") {
            let Some(algo) = hash_attrs.get("type").cloned() else {
                continue;
            };
            let hex = hash_block.trim().to_ascii_lowercase();
            if hex.is_empty() {
                continue;
            }
            hashes.push(MetalinkHash {
                algorithm: algo.to_ascii_lowercase(),
                hex,
            });
        }

        let mut urls = Vec::new();
        for (url_block, url_attrs) in split_on_local_tag(block, "url") {
            let url = url_block.trim();
            if url.is_empty() {
                continue;
            }
            let priority = url_attrs
                .get("priority")
                .and_then(|p| p.parse::<u32>().ok());
            urls.push(MetalinkUrl {
                url: url.to_string(),
                priority,
            });
        }
        // Also accept v3-style <resources><url>...</url></resources>,
        // which the generic `split_on_local_tag(block, "url")` scan above
        // already finds regardless of the enclosing <resources> wrapper,
        // so no separate handling is needed there.

        urls.sort_by_key(|u| u.priority.unwrap_or(u32::MAX));

        if urls.is_empty() {
            continue;
        }

        files.push(MetalinkFile {
            name,
            size,
            hashes,
            urls,
        });
    }

    if files.is_empty() {
        return Err(ProtoError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "Metalink document had <file> elements but none had a usable <url>",
        )));
    }

    Ok(files)
}

/// Find the byte offset of the next `<[prefix:]tag` opening occurrence,
/// matching on local name regardless of namespace prefix (mirrors
/// `crate::webdav`'s tag scanner).
fn find_local_tag_open(xml: &str, local_name: &str) -> Option<usize> {
    let lower = xml.to_lowercase();
    let needle_bare = format!("<{}", local_name.to_lowercase());
    let mut search_from = 0;
    while let Some(rel) = lower[search_from..].find('<') {
        let abs = search_from + rel;
        let after = &lower[abs..];
        if after.starts_with(&needle_bare) {
            let boundary = after.as_bytes().get(needle_bare.len());
            if matches!(
                boundary,
                None | Some(b' ')
                    | Some(b'>')
                    | Some(b'/')
                    | Some(b'\t')
                    | Some(b'\n')
                    | Some(b'\r')
            ) {
                return Some(abs);
            }
        } else if let Some(colon) = after[1..].find(':') {
            let tag_start = abs + 1 + colon + 1;
            if lower[tag_start..].starts_with(&local_name.to_lowercase()) {
                let boundary_idx = tag_start + local_name.len();
                let boundary = lower.as_bytes().get(boundary_idx);
                if matches!(
                    boundary,
                    None | Some(b' ')
                        | Some(b'>')
                        | Some(b'/')
                        | Some(b'\t')
                        | Some(b'\n')
                        | Some(b'\r')
                ) {
                    return Some(abs);
                }
            }
        }
        search_from = abs + 1;
    }
    None
}

fn find_local_tag_close(xml: &str, local_name: &str) -> Option<(usize, usize)> {
    let lower = xml.to_lowercase();
    let local_lower = local_name.to_lowercase();
    let mut search_from = 0;
    while let Some(rel) = lower[search_from..].find("</") {
        let abs = search_from + rel;
        let after = &lower[abs + 2..];
        let name_end = after.find('>')?;
        let candidate = &after[..name_end];
        let local_part = candidate.rsplit(':').next().unwrap_or(candidate);
        if local_part.trim() == local_lower {
            return Some((abs, abs + 2 + name_end + 1));
        }
        search_from = abs + 2;
    }
    None
}

/// Parse the attributes of the *opening* tag starting at `xml[open_start..]`
/// (up to its `>`) into a lowercase-key map. Handles both `'` and `"`
/// quoting; ignores an `xmlns`/`xmlns:*` attribute's prefix-scoping
/// semantics since this parser only ever matches on local tag names.
fn parse_attrs(tag_text: &str) -> std::collections::HashMap<String, String> {
    let mut attrs = std::collections::HashMap::new();
    let bytes = tag_text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        while i < bytes.len() && (bytes[i] as char).is_whitespace() {
            i += 1;
        }
        let name_start = i;
        while i < bytes.len() && bytes[i] != b'=' && !(bytes[i] as char).is_whitespace() {
            i += 1;
        }
        if i >= bytes.len() || name_start == i {
            break;
        }
        let name = tag_text[name_start..i].trim().to_ascii_lowercase();
        while i < bytes.len() && (bytes[i] as char).is_whitespace() {
            i += 1;
        }
        if i >= bytes.len() || bytes[i] != b'=' {
            continue;
        }
        i += 1; // skip '='
        while i < bytes.len() && (bytes[i] as char).is_whitespace() {
            i += 1;
        }
        if i >= bytes.len() {
            break;
        }
        let quote = bytes[i];
        if quote != b'"' && quote != b'\'' {
            continue;
        }
        i += 1;
        let value_start = i;
        while i < bytes.len() && bytes[i] != quote {
            i += 1;
        }
        let value = tag_text[value_start..i.min(tag_text.len())].to_string();
        if i < bytes.len() {
            i += 1; // skip closing quote
        }
        // Local name only (strip any namespace prefix, e.g. "metalink:priority").
        let local_name = name.rsplit(':').next().unwrap_or(&name).to_string();
        attrs.insert(local_name, value);
    }
    attrs
}

/// Split `xml` into `(inner_content, opening_tag_attributes)` pairs for
/// every `<[prefix:]tag ...>...</[prefix:]tag>` block matching
/// `local_name`, at any nesting depth (non-recursive, same limitation as
/// `crate::webdav`'s scanner — fine for Metalink's flat `file`/`hash`/`url`
/// shape).
fn split_on_local_tag<'a>(
    xml: &'a str,
    local_name: &str,
) -> Vec<(&'a str, std::collections::HashMap<String, String>)> {
    let mut out = Vec::new();
    let mut rest = xml;
    let mut consumed = 0usize;
    while let Some(open_start) = find_local_tag_open(rest, local_name) {
        let after_open = &rest[open_start..];
        let Some(tag_end) = after_open.find('>') else {
            break;
        };
        let tag_text = &after_open[1..tag_end];
        // Self-closing tag, e.g. <url priority="1"/> with no body — skip
        // (Metalink never uses this for url/hash/file in practice, but
        // guard against it rather than mis-splitting).
        if tag_text.trim_end().ends_with('/') {
            let attrs = parse_attrs(tag_text.trim_end().trim_end_matches('/'));
            out.push(("", attrs));
            rest = &rest[open_start + tag_end + 1..];
            consumed += open_start + tag_end + 1;
            let _ = consumed;
            continue;
        }

        let attrs = parse_attrs(tag_text);
        let body_start = open_start + tag_end + 1;
        let Some((close_start, close_end)) = find_local_tag_close(&rest[body_start..], local_name)
        else {
            break;
        };
        out.push((&rest[body_start..body_start + close_start], attrs));
        rest = &rest[body_start + close_end..];
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_V4: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<metalink xmlns="urn:ietf:params:xml:ns:metalink">
  <file name="example.iso">
    <size>14680064</size>
    <hash type="sha-256">e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b85</hash>
    <hash type="md5">d41d8cd98f00b204e9800998ecf8427e</hash>
    <url priority="1">https://mirror-a.example.com/example.iso</url>
    <url priority="2">https://mirror-b.example.com/example.iso</url>
    <url priority="3">https://mirror-c.example.com/example.iso</url>
  </file>
</metalink>"#;

    #[test]
    fn parses_file_metadata() {
        let files = parse(SAMPLE_V4).unwrap();
        assert_eq!(files.len(), 1);
        let f = &files[0];
        assert_eq!(f.name, "example.iso");
        assert_eq!(f.size, Some(14_680_064));
    }

    #[test]
    fn sorts_urls_by_priority() {
        let files = parse(SAMPLE_V4).unwrap();
        let urls: Vec<&str> = files[0].urls.iter().map(|u| u.url.as_str()).collect();
        assert_eq!(
            urls,
            vec![
                "https://mirror-a.example.com/example.iso",
                "https://mirror-b.example.com/example.iso",
                "https://mirror-c.example.com/example.iso",
            ]
        );
    }

    #[test]
    fn picks_strongest_hash() {
        let files = parse(SAMPLE_V4).unwrap();
        let strongest = files[0].strongest_hash().unwrap();
        assert_eq!(strongest.algorithm, "sha-256");
        assert_eq!(
            strongest.hex,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b85"
        );
    }

    #[test]
    fn normalizes_hash_algorithm_spelling() {
        assert_eq!(normalize_algo("SHA-256"), "sha256");
        assert_eq!(normalize_algo("sha256"), "sha256");
        assert_eq!(normalize_algo("MD5"), "md5");
    }

    #[test]
    fn handles_missing_priority_by_ordering_last() {
        let xml = r#"<metalink>
  <file name="f.bin">
    <url>https://no-priority.example.com/f.bin</url>
    <url priority="1">https://has-priority.example.com/f.bin</url>
  </file>
</metalink>"#;
        let files = parse(xml).unwrap();
        assert_eq!(
            files[0].urls[0].url,
            "https://has-priority.example.com/f.bin"
        );
        assert_eq!(
            files[0].urls[1].url,
            "https://no-priority.example.com/f.bin"
        );
    }

    #[test]
    fn rejects_document_with_no_files() {
        assert!(parse("<metalink></metalink>").is_err());
    }

    #[test]
    fn skips_file_with_no_urls() {
        let xml = r#"<metalink>
  <file name="unreachable.bin">
    <size>10</size>
  </file>
  <file name="reachable.bin">
    <url>https://example.com/reachable.bin</url>
  </file>
</metalink>"#;
        let files = parse(xml).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].name, "reachable.bin");
    }

    #[test]
    fn tolerates_metalink_namespace_prefix() {
        let xml = r#"<m:metalink xmlns:m="urn:ietf:params:xml:ns:metalink">
  <m:file name="prefixed.bin">
    <m:hash type="sha-1">da39a3ee5e6b4b0d3255bfef95601890afd80709</m:hash>
    <m:url priority="1">https://example.com/prefixed.bin</m:url>
  </m:file>
</m:metalink>"#;
        let files = parse(xml).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].name, "prefixed.bin");
        assert_eq!(files[0].urls[0].url, "https://example.com/prefixed.bin");
    }
}
