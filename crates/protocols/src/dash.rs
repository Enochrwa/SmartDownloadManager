//! MPEG-DASH (`.mpd`) support (Sprint 9), built on the `dash-mpd` crate
//! (published as `dash-mpd` on crates.io; the project's upstream repo is
//! `emarsden/dash-mpd-rs`, which is where `docs/TECH_DECISIONS.md` §5's
//! "dash-mpd-rs" name comes from). Only the crate's manifest-parsing
//! struct model is used (`default-features = false` in the root
//! `Cargo.toml` drops its own `fetch` feature and the ffmpeg/subprocess
//! muxing it pulls in) — segment fetching goes through the existing HTTP
//! downloader, and audio/video representations are downloaded to separate
//! files per `docs/SPRINT_PLAN_PHASE2.md` Sprint 9 ("a DASH manifest with
//! separate audio/video adaptation sets downloads both"); muxing them
//! together is Sprint 10's FFmpeg remux step.
//!
//! This module resolves `SegmentTemplate` addressing (the by-far most
//! common scheme in real-world manifests) via `SegmentTimeline` when
//! present, or a fixed `@duration` otherwise. `SegmentBase`/`SegmentList`
//! addressing is out of scope for this sprint (documented limitation,
//! same spirit as `crate::scp`'s "non-resumable, documented rather than
//! faked" choice).

pub use dash_mpd::{Representation as MpdRepresentation, MPD};
use url::Url;

use crate::http::ProtoError;

fn io_err(msg: impl Into<String>) -> ProtoError {
    ProtoError::Io(std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        msg.into(),
    ))
}

/// Parse an `.mpd` manifest document.
pub fn parse_manifest(xml: &str) -> Result<MPD, ProtoError> {
    dash_mpd::parse(xml).map_err(|e| io_err(format!("invalid DASH manifest: {e}")))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaKind {
    Video,
    Audio,
    Text,
    Other,
}

/// One selectable representation, flattened out of the
/// `Period`/`AdaptationSet`/`Representation` nesting with enough indices
/// to look it back up for segment resolution ([`resolve_segments`]).
#[derive(Debug, Clone)]
pub struct DashRepresentation {
    pub period_index: usize,
    pub adaptation_index: usize,
    pub representation_id: String,
    pub kind: MediaKind,
    pub bandwidth: Option<u64>,
    pub width: Option<u64>,
    pub height: Option<u64>,
    pub codecs: Option<String>,
}

fn classify(
    adaptation: &dash_mpd::AdaptationSet,
    representation: &dash_mpd::Representation,
) -> MediaKind {
    let content_type = adaptation
        .contentType
        .as_deref()
        .or(representation.contentType.as_deref());
    if let Some(ct) = content_type {
        return match ct {
            "video" => MediaKind::Video,
            "audio" => MediaKind::Audio,
            "text" => MediaKind::Text,
            _ => MediaKind::Other,
        };
    }
    let mime = representation
        .mimeType
        .as_deref()
        .or(adaptation.mimeType.as_deref());
    match mime {
        Some(m) if m.starts_with("video") => MediaKind::Video,
        Some(m) if m.starts_with("audio") => MediaKind::Audio,
        Some(m) if m.starts_with("text") || m.contains("ttml") || m.contains("vtt") => {
            MediaKind::Text
        }
        _ => MediaKind::Other,
    }
}

/// List every representation across every period/adaptation set in the
/// manifest.
pub fn list_representations(mpd: &MPD) -> Vec<DashRepresentation> {
    let mut out = Vec::new();
    for (period_index, period) in mpd.periods.iter().enumerate() {
        for (adaptation_index, adaptation) in period.adaptations.iter().enumerate() {
            for representation in &adaptation.representations {
                let Some(id) = representation.id.clone() else {
                    continue; // no id (e.g. unresolved xlink) — nothing stable to select by
                };
                out.push(DashRepresentation {
                    period_index,
                    adaptation_index,
                    representation_id: id,
                    kind: classify(adaptation, representation),
                    bandwidth: representation.bandwidth,
                    width: representation.width,
                    height: representation.height,
                    codecs: representation.codecs.clone(),
                });
            }
        }
    }
    out
}

/// Highest-bandwidth representation of a given kind, if any.
pub fn select_best(reps: &[DashRepresentation], kind: MediaKind) -> Option<&DashRepresentation> {
    reps.iter()
        .filter(|r| r.kind == kind)
        .max_by_key(|r| r.bandwidth.unwrap_or(0))
}

/// Resolved download plan for one representation: an optional
/// initialization segment (fMP4) followed by the ordered media segments.
#[derive(Debug, Clone)]
pub struct DashSegmentPlan {
    pub init_url: Option<String>,
    pub media_urls: Vec<String>,
}

/// Substitute `$RepresentationID$`, `$Bandwidth$`, `$Number[%0Nd]$`, and
/// `$Time[%0Nd]$` identifiers in a `SegmentTemplate` URL pattern, and
/// `$$` as a literal `$` — per ISO/IEC 23009-1 §5.3.9.4.4. Any other/
/// unrecognized `$token$` is left untouched rather than silently dropped,
/// since silently producing a broken URL is worse than an obviously
/// wrong one a caller can debug.
fn substitute_template(
    template: &str,
    representation_id: &str,
    bandwidth: Option<u64>,
    number: Option<u64>,
    time: Option<u64>,
) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(dollar) = rest.find('$') {
        out.push_str(&rest[..dollar]);
        let after = &rest[dollar + 1..];
        let Some(end) = after.find('$') else {
            // Unterminated '$' — copy the rest verbatim and stop.
            out.push_str(&rest[dollar..]);
            rest = "";
            break;
        };
        let token = &after[..end];
        if token.is_empty() {
            out.push('$'); // "$$" -> literal "$"
        } else {
            let mut parts = token.splitn(2, '%');
            let name = parts.next().unwrap_or("");
            let fmt = parts.next();
            let value = match name {
                "RepresentationID" => Some(representation_id.to_string()),
                "Bandwidth" => bandwidth.map(|b| b.to_string()),
                "Number" => number.map(|n| n.to_string()),
                "Time" => time.map(|t| t.to_string()),
                _ => None,
            };
            match value {
                Some(v) => {
                    let width = fmt.and_then(|f| f.trim_end_matches('d').parse::<usize>().ok());
                    match width {
                        Some(w) => out.push_str(&format!("{v:0>w$}")),
                        None => out.push_str(&v),
                    }
                }
                None => out.push_str(&format!("${token}$")),
            }
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    out
}

/// Join `relative` against `base`, tolerating `relative` already being
/// absolute (matches `Url::join`'s normal behavior).
fn join_url(base: &str, relative: &str) -> Result<String, ProtoError> {
    let base_url =
        Url::parse(base).map_err(|e| io_err(format!("invalid base URL {base:?}: {e}")))?;
    let joined = base_url
        .join(relative)
        .map_err(|e| io_err(format!("invalid relative URL {relative:?}: {e}")))?;
    Ok(joined.to_string())
}

/// Resolve the effective `BaseURL` for a representation by joining, in
/// order, the manifest URL and every `BaseURL` element found at the MPD,
/// Period, AdaptationSet, and Representation levels (each relative to
/// the one before it) — per ISO/IEC 23009-1 §5.6. Only the first
/// `BaseURL` at each level is used (real-world manifests use a `Vec` for
/// CDN-redundancy alternatives, not for combining multiple prefixes at
/// once), matching this sprint's scope of "one representation, follow
/// its primary path."
fn resolve_base_url(
    manifest_url: &str,
    mpd: &MPD,
    period: &dash_mpd::Period,
    adaptation: &dash_mpd::AdaptationSet,
    representation: &dash_mpd::Representation,
) -> Result<String, ProtoError> {
    let mut current = manifest_url.to_string();
    for candidate in [
        mpd.base_url.first(),
        period.BaseURL.first(),
        adaptation.BaseURL.first(),
        representation.BaseURL.first(),
    ]
    .into_iter()
    .flatten()
    {
        current = join_url(&current, &candidate.base)?;
    }
    Ok(current)
}

/// Resolve every segment URL (init + media) for the representation
/// identified by `(period_index, adaptation_index, representation_id)`.
pub fn resolve_segments(
    manifest_url: &str,
    mpd: &MPD,
    period_index: usize,
    adaptation_index: usize,
    representation_id: &str,
) -> Result<DashSegmentPlan, ProtoError> {
    let period = mpd
        .periods
        .get(period_index)
        .ok_or_else(|| io_err(format!("no such period index {period_index}")))?;
    let adaptation = period
        .adaptations
        .get(adaptation_index)
        .ok_or_else(|| io_err(format!("no such adaptation set index {adaptation_index}")))?;
    let representation = adaptation
        .representations
        .iter()
        .find(|r| r.id.as_deref() == Some(representation_id))
        .ok_or_else(|| io_err(format!("no such representation id {representation_id:?}")))?;

    let base_url = resolve_base_url(manifest_url, mpd, period, adaptation, representation)?;

    let template = representation
        .SegmentTemplate
        .as_ref()
        .or(adaptation.SegmentTemplate.as_ref())
        .or(period.SegmentTemplate.as_ref())
        .ok_or_else(|| {
            io_err(
                "representation has no SegmentTemplate (SegmentBase/SegmentList addressing \
                 is not yet supported)",
            )
        })?;

    let rep_id = representation_id;
    let bandwidth = representation.bandwidth;

    let init_url = template
        .initialization
        .as_ref()
        .map(|pattern| {
            let resolved = substitute_template(pattern, rep_id, bandwidth, None, None);
            join_url(&base_url, &resolved)
        })
        .transpose()?;

    let media_pattern = template.media.as_ref().ok_or_else(|| {
        io_err("SegmentTemplate has no @media attribute to resolve media segment URLs from")
    })?;

    let start_number = template.startNumber.unwrap_or(1);
    let mut media_urls = Vec::new();

    if let Some(timeline) = &template.SegmentTimeline {
        let mut number = start_number;
        let mut time = 0u64;
        let mut have_explicit_time = false;
        for s in &timeline.segments {
            if let Some(t) = s.t {
                time = t;
                have_explicit_time = true;
            }
            let repeat = s.r.unwrap_or(0).max(0) as u64;
            for _ in 0..=repeat {
                let resolved =
                    substitute_template(media_pattern, rep_id, bandwidth, Some(number), Some(time));
                media_urls.push(join_url(&base_url, &resolved)?);
                number += 1;
                time += s.d;
            }
        }
        let _ = have_explicit_time; // only affects the first segment's starting point
    } else if let Some(duration) = template.duration {
        let timescale = template.timescale.unwrap_or(1).max(1) as f64;
        let segment_seconds = duration / timescale;
        if segment_seconds <= 0.0 {
            return Err(io_err("SegmentTemplate @duration resolved to <= 0 seconds"));
        }
        let total_seconds = period
            .duration
            .map(|d| d.as_secs_f64())
            .or_else(|| mpd.mediaPresentationDuration.map(|d| d.as_secs_f64()))
            .ok_or_else(|| {
                io_err(
                    "cannot determine segment count: manifest has neither a SegmentTimeline \
                     nor a Period/MPD duration to divide by the fixed segment @duration \
                     (typical of a live/dynamic manifest, which Sprint 9 doesn't cover)",
                )
            })?;
        let count = (total_seconds / segment_seconds).ceil().max(1.0) as u64;
        for n in start_number..start_number + count {
            let resolved = substitute_template(media_pattern, rep_id, bandwidth, Some(n), None);
            media_urls.push(join_url(&base_url, &resolved)?);
        }
    } else {
        return Err(io_err(
            "SegmentTemplate has neither SegmentTimeline nor @duration — cannot enumerate segments",
        ));
    }

    Ok(DashSegmentPlan {
        init_url,
        media_urls,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const MANIFEST_TIMELINE: &str = r#"<?xml version="1.0"?>
<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" mediaPresentationDuration="PT30S">
  <Period>
    <AdaptationSet contentType="video" mimeType="video/mp4">
      <SegmentTemplate initialization="$RepresentationID$/init.mp4"
                        media="$RepresentationID$/seg-$Number$.m4s"
                        startNumber="1">
        <SegmentTimeline>
          <S t="0" d="10" r="2"/>
        </SegmentTimeline>
      </SegmentTemplate>
      <Representation id="video-1080p" bandwidth="5000000" width="1920" height="1080" codecs="avc1.640028"/>
      <Representation id="video-480p" bandwidth="1000000" width="854" height="480" codecs="avc1.4d401e"/>
    </AdaptationSet>
    <AdaptationSet contentType="audio" mimeType="audio/mp4">
      <SegmentTemplate initialization="$RepresentationID$/init.mp4"
                        media="$RepresentationID$/seg-$Number$.m4s"
                        startNumber="1">
        <SegmentTimeline>
          <S t="0" d="10" r="2"/>
        </SegmentTimeline>
      </SegmentTemplate>
      <Representation id="audio-en" bandwidth="128000" codecs="mp4a.40.2"/>
    </AdaptationSet>
  </Period>
</MPD>"#;

    const MANIFEST_DURATION: &str = r#"<?xml version="1.0"?>
<MPD xmlns="urn:mpeg:dash:schema:mpd:2011">
  <Period duration="PT20S">
    <AdaptationSet contentType="video">
      <SegmentTemplate initialization="init-$RepresentationID$.mp4"
                        media="chunk-$RepresentationID$-$Number%05d$.m4s"
                        startNumber="1" duration="10" timescale="1"/>
      <Representation id="v1" bandwidth="2000000"/>
    </AdaptationSet>
  </Period>
</MPD>"#;

    #[test]
    fn lists_representations_across_adaptation_sets() {
        let mpd = parse_manifest(MANIFEST_TIMELINE).unwrap();
        let reps = list_representations(&mpd);
        assert_eq!(reps.len(), 3);
        assert!(reps
            .iter()
            .any(|r| r.representation_id == "video-1080p" && r.kind == MediaKind::Video));
        assert!(reps
            .iter()
            .any(|r| r.representation_id == "audio-en" && r.kind == MediaKind::Audio));
    }

    #[test]
    fn selects_highest_bandwidth_per_kind() {
        let mpd = parse_manifest(MANIFEST_TIMELINE).unwrap();
        let reps = list_representations(&mpd);
        let best_video = select_best(&reps, MediaKind::Video).unwrap();
        assert_eq!(best_video.representation_id, "video-1080p");
        let best_audio = select_best(&reps, MediaKind::Audio).unwrap();
        assert_eq!(best_audio.representation_id, "audio-en");
    }

    #[test]
    fn resolves_segments_via_segment_timeline() {
        let mpd = parse_manifest(MANIFEST_TIMELINE).unwrap();
        let plan = resolve_segments(
            "https://cdn.example.com/streams/manifest.mpd",
            &mpd,
            0,
            0,
            "video-1080p",
        )
        .unwrap();
        assert_eq!(
            plan.init_url.as_deref(),
            Some("https://cdn.example.com/streams/video-1080p/init.mp4")
        );
        // t=0 d=10 r=2 => 3 segments total (numbers 1, 2, 3).
        assert_eq!(plan.media_urls.len(), 3);
        assert_eq!(
            plan.media_urls[0],
            "https://cdn.example.com/streams/video-1080p/seg-1.m4s"
        );
        assert_eq!(
            plan.media_urls[2],
            "https://cdn.example.com/streams/video-1080p/seg-3.m4s"
        );
    }

    #[test]
    fn resolves_segments_via_fixed_duration_and_period_length() {
        let mpd = parse_manifest(MANIFEST_DURATION).unwrap();
        let plan =
            resolve_segments("https://cdn.example.com/manifest.mpd", &mpd, 0, 0, "v1").unwrap();
        // 20s period / 10s segments = 2 segments, zero-padded to 5 digits.
        assert_eq!(plan.media_urls.len(), 2);
        assert_eq!(
            plan.media_urls[0],
            "https://cdn.example.com/chunk-v1-00001.m4s"
        );
        assert_eq!(
            plan.media_urls[1],
            "https://cdn.example.com/chunk-v1-00002.m4s"
        );
    }

    #[test]
    fn substitutes_representation_id_and_bandwidth() {
        let resolved = substitute_template(
            "$RepresentationID$/$Bandwidth$/seg-$Number%03d$.mp4",
            "v1",
            Some(500000),
            Some(7),
            None,
        );
        assert_eq!(resolved, "v1/500000/seg-007.mp4");
    }

    #[test]
    fn substitutes_literal_dollar_escape() {
        let resolved = substitute_template("price_$$5.mp4", "v1", None, None, None);
        assert_eq!(resolved, "price_$5.mp4");
    }
}
