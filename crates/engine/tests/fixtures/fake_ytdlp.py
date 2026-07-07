#!/usr/bin/env python3
"""A deliberately minimal stand-in for the real `yt-dlp` CLI, used only by
`crates/engine/tests/sprint10_flow.rs` to test playlist expansion (parent
Job + N child Jobs) deterministically -- there is no local equivalent of
a real "playlist" site the way a raw HTTP file server can stand in for a
single direct-URL video (see `crates/media/tests/support/tiny_http.rs`
and its doc comment for that latter, real-subprocess approach), so this
script fakes just enough of yt-dlp's own CLI contract (the same
`--dump-json`/`--flat-playlist`/progress-template/`[download]
Destination:` surface `crates/media::ytdlp` actually parses) for one
specific fake playlist URL, and otherwise behaves like a single-video
generic extraction.

This intentionally does NOT replace `crates/media`'s own integration
tests, which run against the real `yt-dlp` binary -- it only lets
`crates/engine::media`'s playlist-expansion *orchestration logic* (does
it create 1 parent + N children, does it link them, does per-child
failure not abort the rest) be tested without depending on a real
multi-video site being reachable/stable in CI.
"""
import json
import sys
import os
import shutil

PLAYLIST_URL = "https://fake.invalid/playlist?id=sprint10"
MULTIFORMAT_URL = "https://fake.invalid/multiformat"

FIXTURES_DIR = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "..", "media", "tests", "fixtures")


def child_url(n):
    return f"https://fake.invalid/watch?v={n}"


def main():
    args = sys.argv[1:]

    if args == ["--version"]:
        print("2026.07.01-fake")
        return 0

    if "--flat-playlist" in args and "--dump-json" in args:
        url = args[-1]
        if url == PLAYLIST_URL:
            for n in range(1, 4):
                print(json.dumps({"id": f"child{n}", "url": child_url(n), "title": f"Child {n}"}))
        else:
            # A single video probed the same way (yt-dlp's flat-playlist
            # listing of a lone video is one entry describing itself).
            print(json.dumps({"id": "solo", "url": url, "title": "Solo video"}))
        return 0

    if "--dump-json" in args and "--no-playlist" in args:
        url = args[-1]
        if url == MULTIFORMAT_URL:
            # A source whose best quality has genuinely separate
            # video-only and audio-only formats -- exactly the case
            # `crates/engine::media`'s merge path exists for.
            print(json.dumps({
                "id": "multi",
                "title": "Multiformat fake video",
                "duration": 2.0,
                "is_live": False,
                "formats": [
                    {"format_id": "vid", "ext": "mp4", "vcodec": "avc1", "acodec": "none", "height": 240},
                    {"format_id": "aud", "ext": "m4a", "vcodec": "none", "acodec": "aac"},
                ],
            }))
        else:
            print(json.dumps({
                "id": "solo",
                "title": f"Fake video for {url}",
                "duration": 42.0,
                "is_live": False,
                "formats": [
                    {"format_id": "18", "ext": "mp4", "vcodec": "avc1", "acodec": "aac", "height": 360},
                ],
            }))
        return 0

    # Fetch invocation: -f <id> -o <template> --newline ... <url>
    fmt_id = args[args.index("-f") + 1]
    out_template = args[args.index("-o") + 1]
    url = args[-1]

    if fmt_id == "vid":
        output_path = out_template.replace("%(ext)s", "mp4")
        shutil.copyfile(os.path.join(FIXTURES_DIR, "sample_video_only.mp4"), output_path)
    elif fmt_id == "aud":
        output_path = out_template.replace("%(ext)s", "m4a")
        shutil.copyfile(os.path.join(FIXTURES_DIR, "sample_audio_only.m4a"), output_path)
    else:
        output_path = out_template.replace("%(ext)s", "mp4")
        shutil.copyfile(os.path.join(FIXTURES_DIR, "sample.mp4"), output_path)

    stem = out_template.rsplit(".", 1)[0]
    if "--write-subs" in args:
        idx = args.index("--sub-langs")
        langs = args[idx + 1].split(",")
        for lang in langs:
            shutil.copyfile(os.path.join(FIXTURES_DIR, "sample.srt"), f"{stem}.{lang}.srt")
    if "--write-thumbnail" in args:
        shutil.copyfile(os.path.join(FIXTURES_DIR, "sample_thumb.jpg"), f"{stem}.jpg")

    print(f"[download] Destination: {output_path}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
