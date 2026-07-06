//! HLS (`.m3u8`) support (Sprint 9), built on `m3u8-rs`.
//!
//! This module is deliberately thin, the same way `crate::webdav` is thin
//! over plain HTTP: `m3u8-rs` already does the actual playlist parsing, so
//! this module's job is (1) picking a variant out of a master playlist,
//! (2) resolving every relative segment/init/media-playlist URI in a
//! playlist against the URL it was fetched from (HLS URIs are frequently
//! relative), and (3) exposing "is this playlist still live" so the
//! engine knows whether to poll for new segments. Actually fetching each
//! resolved URL goes through the existing HTTP downloader
//! (`crate::http::download_single`) — no new transport logic needed here.

use url::Url;

use crate::http::ProtoError;

fn io_err(msg: impl Into<String>) -> ProtoError {
    ProtoError::Io(std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        msg.into(),
    ))
}

/// Parse an `.m3u8` document, auto-detecting master vs. media playlist —
/// thin re-export of `m3u8_rs::parse_playlist_res` with our error type.
pub fn parse(bytes: &[u8]) -> Result<m3u8_rs::Playlist, ProtoError> {
    m3u8_rs::parse_playlist_res(bytes).map_err(|e| io_err(format!("invalid m3u8 playlist: {e:?}")))
}

/// Resolve `relative` (which may itself already be absolute) against
/// `base`, the URL the playlist containing it was fetched from.
pub fn resolve_url(base: &str, relative: &str) -> Result<String, ProtoError> {
    let base_url =
        Url::parse(base).map_err(|e| io_err(format!("invalid base URL {base:?}: {e}")))?;
    let resolved = base_url
        .join(relative)
        .map_err(|e| io_err(format!("invalid segment URI {relative:?}: {e}")))?;
    Ok(resolved.to_string())
}

/// A candidate quality/variant from a master playlist, with its `uri`
/// already left as-authored (caller resolves it via [`resolve_url`]
/// against the master playlist's own URL).
#[derive(Debug, Clone, PartialEq)]
pub struct HlsVariant {
    pub uri: String,
    pub bandwidth: u64,
    pub resolution: Option<(u64, u64)>,
    pub codecs: Option<String>,
}

/// List every non-I-frame variant, sorted best-quality-first (highest
/// bandwidth first) — I-frame-only variants (trick-play/scrubbing
/// streams) are excluded since they aren't a playable-in-full quality
/// option.
pub fn master_variants(playlist: &m3u8_rs::MasterPlaylist) -> Vec<HlsVariant> {
    let mut variants: Vec<HlsVariant> = playlist
        .variants
        .iter()
        .filter(|v| !v.is_i_frame)
        .map(|v| HlsVariant {
            uri: v.uri.clone(),
            bandwidth: v.bandwidth,
            resolution: v.resolution.as_ref().map(|r| (r.width, r.height)),
            codecs: v.codecs.clone(),
        })
        .collect();
    variants.sort_by_key(|v| std::cmp::Reverse(v.bandwidth));
    variants
}

/// Which variant to pick from a master playlist.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VariantSelector {
    /// Highest bandwidth.
    Best,
    /// Lowest bandwidth.
    Worst,
    /// Index into `master_variants`'s best-first ordering (0 = best).
    ByIndex(usize),
}

impl VariantSelector {
    pub fn parse(s: &str) -> Result<Self, ProtoError> {
        match s.to_ascii_lowercase().as_str() {
            "best" => Ok(VariantSelector::Best),
            "worst" => Ok(VariantSelector::Worst),
            other => other
                .parse::<usize>()
                .map(VariantSelector::ByIndex)
                .map_err(|_| io_err(format!("invalid --quality value: {other:?} (expected \"best\", \"worst\", or a variant index)"))),
        }
    }
}

/// Select one variant out of an already best-first-sorted list (see
/// [`master_variants`]).
pub fn select_variant(variants: &[HlsVariant], selector: VariantSelector) -> Option<&HlsVariant> {
    if variants.is_empty() {
        return None;
    }
    match selector {
        VariantSelector::Best => variants.first(),
        VariantSelector::Worst => variants.last(),
        VariantSelector::ByIndex(i) => variants.get(i),
    }
}

/// One media segment, resolved to an absolute URL, ready to hand to
/// `crate::http::download_single`.
#[derive(Debug, Clone, PartialEq)]
pub struct HlsSegment {
    pub url: String,
    pub duration: f32,
}

/// Resolve every media segment URI in `media` against `playlist_url` (the
/// URL the media playlist itself was fetched from).
pub fn media_segments(
    playlist_url: &str,
    media: &m3u8_rs::MediaPlaylist,
) -> Result<Vec<HlsSegment>, ProtoError> {
    media
        .segments
        .iter()
        .map(|seg| {
            Ok(HlsSegment {
                url: resolve_url(playlist_url, &seg.uri)?,
                duration: seg.duration,
            })
        })
        .collect()
}

/// The fMP4 initialization segment URL (`#EXT-X-MAP`), if any — present
/// on fMP4-segmented playlists, absent on plain MPEG-TS ones. All
/// segments in a playlist conventionally share the same `#EXT-X-MAP`, so
/// the first segment that has one is authoritative.
pub fn init_segment_url(
    playlist_url: &str,
    media: &m3u8_rs::MediaPlaylist,
) -> Result<Option<String>, ProtoError> {
    let Some(map) = media.segments.iter().find_map(|s| s.map.as_ref()) else {
        return Ok(None);
    };
    Ok(Some(resolve_url(playlist_url, &map.uri)?))
}

/// A live (in-progress) playlist has no `#EXT-X-ENDLIST` tag yet — the
/// engine should keep polling it for new segments rather than treating
/// the current segment list as final (Sprint 9 scope: VOD is fully
/// supported; live capture gets segment polling but not the "trim to
/// live edge" niceties a dedicated live-DVR feature would add later).
pub fn is_live(media: &m3u8_rs::MediaPlaylist) -> bool {
    !media.end_list
}

#[cfg(test)]
mod tests {
    use super::*;

    const MASTER: &str = "#EXTM3U\n\
#EXT-X-STREAM-INF:BANDWIDTH=1280000,RESOLUTION=640x360,CODECS=\"avc1.4d401e,mp4a.40.2\"\n\
low/index.m3u8\n\
#EXT-X-STREAM-INF:BANDWIDTH=6400000,RESOLUTION=1920x1080,CODECS=\"avc1.640028,mp4a.40.2\"\n\
high/index.m3u8\n\
#EXT-X-I-FRAME-STREAM-INF:BANDWIDTH=100000,URI=\"iframe/index.m3u8\"\n";

    const MEDIA_VOD: &str = "#EXTM3U\n\
#EXT-X-VERSION:3\n\
#EXT-X-TARGETDURATION:10\n\
#EXT-X-MEDIA-SEQUENCE:0\n\
#EXTINF:9.009,\n\
segment0.ts\n\
#EXTINF:9.009,\n\
segment1.ts\n\
#EXT-X-ENDLIST\n";

    const MEDIA_LIVE: &str = "#EXTM3U\n\
#EXT-X-VERSION:3\n\
#EXT-X-TARGETDURATION:10\n\
#EXT-X-MEDIA-SEQUENCE:0\n\
#EXTINF:9.009,\n\
segment0.ts\n";

    #[test]
    fn parses_master_playlist_variants_best_first() {
        let pl = parse(MASTER.as_bytes()).unwrap();
        let m3u8_rs::Playlist::MasterPlaylist(master) = pl else {
            panic!("expected master playlist");
        };
        let variants = master_variants(&master);
        // I-frame variant excluded, remaining two sorted highest-bandwidth-first.
        assert_eq!(variants.len(), 2);
        assert_eq!(variants[0].uri, "high/index.m3u8");
        assert_eq!(variants[0].resolution, Some((1920, 1080)));
        assert_eq!(variants[1].uri, "low/index.m3u8");
    }

    #[test]
    fn selects_best_worst_and_by_index() {
        let pl = parse(MASTER.as_bytes()).unwrap();
        let m3u8_rs::Playlist::MasterPlaylist(master) = pl else {
            panic!("expected master playlist");
        };
        let variants = master_variants(&master);
        assert_eq!(
            select_variant(&variants, VariantSelector::Best)
                .unwrap()
                .uri,
            "high/index.m3u8"
        );
        assert_eq!(
            select_variant(&variants, VariantSelector::Worst)
                .unwrap()
                .uri,
            "low/index.m3u8"
        );
        assert_eq!(
            select_variant(&variants, VariantSelector::ByIndex(1))
                .unwrap()
                .uri,
            "low/index.m3u8"
        );
    }

    #[test]
    fn resolves_relative_variant_and_segment_urls() {
        let resolved = resolve_url(
            "https://cdn.example.com/videos/master.m3u8",
            "high/index.m3u8",
        )
        .unwrap();
        assert_eq!(resolved, "https://cdn.example.com/videos/high/index.m3u8");
    }

    #[test]
    fn resolves_media_segments_to_absolute_urls() {
        let pl = parse(MEDIA_VOD.as_bytes()).unwrap();
        let m3u8_rs::Playlist::MediaPlaylist(media) = pl else {
            panic!("expected media playlist");
        };
        let segments =
            media_segments("https://cdn.example.com/videos/high/index.m3u8", &media).unwrap();
        assert_eq!(segments.len(), 2);
        assert_eq!(
            segments[0].url,
            "https://cdn.example.com/videos/high/segment0.ts"
        );
        assert_eq!(
            segments[1].url,
            "https://cdn.example.com/videos/high/segment1.ts"
        );
    }

    #[test]
    fn detects_vod_vs_live() {
        let vod = parse(MEDIA_VOD.as_bytes()).unwrap();
        let m3u8_rs::Playlist::MediaPlaylist(vod) = vod else {
            panic!("expected media playlist");
        };
        assert!(!is_live(&vod));

        let live = parse(MEDIA_LIVE.as_bytes()).unwrap();
        let m3u8_rs::Playlist::MediaPlaylist(live) = live else {
            panic!("expected media playlist");
        };
        assert!(is_live(&live));
    }

    #[test]
    fn variant_selector_parses_named_and_numeric_forms() {
        assert_eq!(
            VariantSelector::parse("best").unwrap(),
            VariantSelector::Best
        );
        assert_eq!(
            VariantSelector::parse("WORST").unwrap(),
            VariantSelector::Worst
        );
        assert_eq!(
            VariantSelector::parse("2").unwrap(),
            VariantSelector::ByIndex(2)
        );
        assert!(VariantSelector::parse("bogus").is_err());
    }
}
