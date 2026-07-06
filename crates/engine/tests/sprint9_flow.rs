//! Sprint 9 end-to-end tests: Metalink (mirror failover reusing Sprint 4
//! machinery), HLS (master -> media playlist -> segment concatenation),
//! and MPEG-DASH (separate video/audio representation downloads). Same
//! `wiremock`-driven approach as `sprint4_flow.rs`.

use sdm_engine::{
    ConnectionsOption, DashDownloadRequest, DuplicatePolicy, Engine, HlsDownloadRequest,
    MetalinkSource,
};
use sha2::{Digest, Sha256};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hex::encode(hasher.finalize())
}

// ---------------------------------------------------------------------
// Metalink
// ---------------------------------------------------------------------

#[tokio::test]
async fn metalink_falls_over_to_a_working_mirror_and_verifies_checksum() {
    let data: Vec<u8> = (0..10_000u32).map(|i| (i % 251) as u8).collect();
    let expected_hex = sha256_hex(&data);

    let server_a = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&server_a)
        .await;

    let server_b = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(data.clone()))
        .mount(&server_b)
        .await;

    let metalink_xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<metalink xmlns="urn:ietf:params:xml:ns:metalink">
  <file name="payload.bin">
    <size>{}</size>
    <hash type="sha-256">{}</hash>
    <url priority="1">{}/payload.bin</url>
    <url priority="2">{}/payload.bin</url>
  </file>
</metalink>"#,
        data.len(),
        expected_hex,
        server_a.uri(),
        server_b.uri()
    );

    let tmp = tempfile::tempdir().unwrap();
    let metalink_path = tmp.path().join("job.metalink");
    tokio::fs::write(&metalink_path, metalink_xml)
        .await
        .unwrap();

    let pool = sdm_storage::connect_in_memory().await.unwrap();
    let engine = Engine::new(pool.clone());
    let (tx, _rx) = sdm_engine::channel();

    let job = engine
        .start_metalink_download(
            MetalinkSource::LocalPath(metalink_path),
            tmp.path().to_path_buf(),
            ConnectionsOption::Fixed(1),
            DuplicatePolicy::default(),
            tx,
        )
        .await
        .expect("metalink download should succeed via the second mirror");

    assert_eq!(job.status, sdm_engine::JobStatus::Completed);
    assert!(job.checksum_verified);
    let downloaded = tokio::fs::read(&job.destination).await.unwrap();
    assert_eq!(downloaded, data);
}

#[tokio::test]
async fn metalink_document_with_no_files_is_rejected() {
    let tmp = tempfile::tempdir().unwrap();
    let metalink_path = tmp.path().join("empty.metalink");
    tokio::fs::write(&metalink_path, "<metalink></metalink>")
        .await
        .unwrap();

    let pool = sdm_storage::connect_in_memory().await.unwrap();
    let engine = Engine::new(pool.clone());
    let (tx, _rx) = sdm_engine::channel();

    let err = engine
        .start_metalink_download(
            MetalinkSource::LocalPath(metalink_path),
            tmp.path().to_path_buf(),
            ConnectionsOption::Auto,
            DuplicatePolicy::default(),
            tx,
        )
        .await
        .unwrap_err();
    assert!(matches!(err, sdm_engine::EngineError::Protocol(_)));
}

// ---------------------------------------------------------------------
// HLS
// ---------------------------------------------------------------------

#[tokio::test]
async fn hls_vod_downloads_master_variant_and_concatenates_segments() {
    let server = MockServer::start().await;

    let master = "#EXTM3U\n\
#EXT-X-STREAM-INF:BANDWIDTH=800000,RESOLUTION=640x360\n\
low/index.m3u8\n\
#EXT-X-STREAM-INF:BANDWIDTH=4000000,RESOLUTION=1920x1080\n\
high/index.m3u8\n";
    Mock::given(method("GET"))
        .and(path("/master.m3u8"))
        .respond_with(ResponseTemplate::new(200).set_body_string(master))
        .mount(&server)
        .await;

    let media = "#EXTM3U\n\
#EXT-X-VERSION:3\n\
#EXT-X-TARGETDURATION:6\n\
#EXT-X-MEDIA-SEQUENCE:0\n\
#EXTINF:6.0,\n\
seg0.ts\n\
#EXTINF:6.0,\n\
seg1.ts\n\
#EXT-X-ENDLIST\n";
    Mock::given(method("GET"))
        .and(path("/high/index.m3u8"))
        .respond_with(ResponseTemplate::new(200).set_body_string(media))
        .mount(&server)
        .await;

    let seg0: Vec<u8> = (0..1000u32).map(|i| (i % 251) as u8).collect();
    let seg1: Vec<u8> = (0..1500u32).map(|i| ((i + 7) % 251) as u8).collect();
    Mock::given(method("GET"))
        .and(path("/high/seg0.ts"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(seg0.clone()))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/high/seg1.ts"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(seg1.clone()))
        .mount(&server)
        .await;

    let pool = sdm_storage::connect_in_memory().await.unwrap();
    let engine = Engine::new(pool.clone());
    let tmp = tempfile::tempdir().unwrap();
    let dest = tmp.path().join("movie.ts");
    let (tx, _rx) = sdm_engine::channel();

    let req = HlsDownloadRequest {
        url: format!("{}/master.m3u8", server.uri()),
        destination: dest.clone(),
        variant: sdm_protocols::hls::VariantSelector::Best,
        expected_checksum: None,
        duplicate_policy: DuplicatePolicy::default(),
        max_live_polls: Some(0),
    };
    let job = engine
        .start_hls_download(req, tx)
        .await
        .expect("HLS VOD download should succeed");

    assert_eq!(job.status, sdm_engine::JobStatus::Completed);
    let downloaded = tokio::fs::read(&dest).await.unwrap();
    let mut expected = seg0.clone();
    expected.extend_from_slice(&seg1);
    assert_eq!(
        downloaded, expected,
        "concatenated file must be seg0 ++ seg1"
    );

    // High-bandwidth variant should have been selected, not low.
    let requests = server.received_requests().await.unwrap();
    assert!(requests.iter().any(|r| r.url.path() == "/high/index.m3u8"));
    assert!(!requests.iter().any(|r| r.url.path() == "/low/index.m3u8"));
}

#[tokio::test]
async fn hls_single_media_playlist_without_master_downloads_directly() {
    let server = MockServer::start().await;

    let media = "#EXTM3U\n\
#EXT-X-VERSION:3\n\
#EXT-X-TARGETDURATION:6\n\
#EXTINF:6.0,\n\
only.ts\n\
#EXT-X-ENDLIST\n";
    Mock::given(method("GET"))
        .and(path("/stream.m3u8"))
        .respond_with(ResponseTemplate::new(200).set_body_string(media))
        .mount(&server)
        .await;

    let seg: Vec<u8> = (0..500u32).map(|i| (i % 251) as u8).collect();
    Mock::given(method("GET"))
        .and(path("/only.ts"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(seg.clone()))
        .mount(&server)
        .await;

    let pool = sdm_storage::connect_in_memory().await.unwrap();
    let engine = Engine::new(pool.clone());
    let tmp = tempfile::tempdir().unwrap();
    let dest = tmp.path().join("clip.ts");
    let (tx, _rx) = sdm_engine::channel();

    let req = HlsDownloadRequest {
        url: format!("{}/stream.m3u8", server.uri()),
        destination: dest.clone(),
        variant: sdm_protocols::hls::VariantSelector::Best,
        expected_checksum: None,
        duplicate_policy: DuplicatePolicy::default(),
        max_live_polls: Some(0),
    };
    let job = engine.start_hls_download(req, tx).await.unwrap();

    assert_eq!(job.status, sdm_engine::JobStatus::Completed);
    let downloaded = tokio::fs::read(&dest).await.unwrap();
    assert_eq!(downloaded, seg);
}

// ---------------------------------------------------------------------
// MPEG-DASH
// ---------------------------------------------------------------------

#[tokio::test]
async fn dash_downloads_video_and_audio_representations_separately() {
    let server = MockServer::start().await;

    let manifest = r#"<?xml version="1.0"?>
<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" mediaPresentationDuration="PT20S">
  <Period>
    <AdaptationSet contentType="video" mimeType="video/mp4">
      <SegmentTemplate initialization="video/init.mp4" media="video/seg-$Number$.m4s" startNumber="1" duration="10" timescale="1"/>
      <Representation id="v-high" bandwidth="5000000"/>
      <Representation id="v-low" bandwidth="500000"/>
    </AdaptationSet>
    <AdaptationSet contentType="audio" mimeType="audio/mp4">
      <SegmentTemplate initialization="audio/init.mp4" media="audio/seg-$Number$.m4s" startNumber="1" duration="10" timescale="1"/>
      <Representation id="a-en" bandwidth="128000"/>
    </AdaptationSet>
  </Period>
</MPD>"#;
    Mock::given(method("GET"))
        .and(path("/manifest.mpd"))
        .respond_with(ResponseTemplate::new(200).set_body_string(manifest))
        .mount(&server)
        .await;

    let video_init = b"VIDEO-INIT".to_vec();
    let video_seg1 = b"VIDEO-SEG-1".to_vec();
    let video_seg2 = b"VIDEO-SEG-2".to_vec();
    let audio_init = b"AUDIO-INIT".to_vec();
    let audio_seg1 = b"AUDIO-SEG-1".to_vec();
    let audio_seg2 = b"AUDIO-SEG-2".to_vec();

    for (p, body) in [
        ("/video/init.mp4", &video_init),
        ("/video/seg-1.m4s", &video_seg1),
        ("/video/seg-2.m4s", &video_seg2),
        ("/audio/init.mp4", &audio_init),
        ("/audio/seg-1.m4s", &audio_seg1),
        ("/audio/seg-2.m4s", &audio_seg2),
    ] {
        Mock::given(method("GET"))
            .and(path(p))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(body.clone()))
            .mount(&server)
            .await;
    }

    let pool = sdm_storage::connect_in_memory().await.unwrap();
    let engine = Engine::new(pool.clone());
    let tmp = tempfile::tempdir().unwrap();
    let (tx, _rx) = sdm_engine::channel();

    let req = DashDownloadRequest {
        url: format!("{}/manifest.mpd", server.uri()),
        destination_dir: tmp.path().to_path_buf(),
        file_stem: "movie".to_string(),
        duplicate_policy: DuplicatePolicy::default(),
    };
    let job = engine
        .start_dash_download(req, tx)
        .await
        .expect("DASH download should succeed");

    assert_eq!(job.status, sdm_engine::JobStatus::Completed);
    assert!(job.destination.contains("movie.video.mp4"));

    let video_bytes = tokio::fs::read(&job.destination).await.unwrap();
    let mut expected_video = video_init.clone();
    expected_video.extend_from_slice(&video_seg1);
    expected_video.extend_from_slice(&video_seg2);
    assert_eq!(video_bytes, expected_video);

    let audio_path =
        sdm_engine::dash::audio_destination_for(std::path::Path::new(&job.destination));
    let audio_bytes = tokio::fs::read(&audio_path).await.unwrap();
    let mut expected_audio = audio_init.clone();
    expected_audio.extend_from_slice(&audio_seg1);
    expected_audio.extend_from_slice(&audio_seg2);
    assert_eq!(audio_bytes, expected_audio);

    // Highest-bandwidth video representation should have been selected.
    let requests = server.received_requests().await.unwrap();
    assert!(requests.iter().any(|r| r.url.path() == "/video/seg-1.m4s"));
}
