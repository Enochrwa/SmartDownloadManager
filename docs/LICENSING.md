# Licensing

SmartDownloadManager's own code is licensed under the **MIT License** (see
`LICENSE`). This document tracks the licenses of the open-source components we
depend on or shell out to, so we stay honest about distribution obligations.

| Component | License | Usage | Note |
|---|---|---|---|
| Rust crates (`reqwest`, `tokio`, `axum`, `clap`, `sqlx`, `rustls`, ...) | MIT/Apache-2.0 (dual) | Statically linked | No copyleft obligations |
| `suppaftp` | MIT | Linked | — |
| `russh` (SFTP/SCP) | Apache-2.0 | Linked | — |
| BitTorrent crate (`librqbit` or similar) | MIT/Apache-2.0 | Linked | Confirm exact crate before Sprint 7 |
| `yt-dlp` | Unlicense (public domain) | Invoked as external subprocess, not linked | No restrictions, but verify the bundled binary's own checksum at install time |
| `FFmpeg` | LGPL 2.1+ or GPL 2+/3+ depending on build flags | Invoked as external subprocess, not linked | **Must ship the LGPL build** (no GPL-only codecs enabled) unless we're prepared for our distributed binary to be GPL — decision needed before Sprint 6 packaging |
| SQLite | Public domain | Embedded | — |
| Tauri | MIT/Apache-2.0 (dual) | Framework | — |
| React | MIT | Frontend | — |

## Action items before any public binary release
1. Confirm the exact FFmpeg build we bundle is LGPL-compliant (no `--enable-gpl`,
   no nonfree codecs), or explicitly decide to GPL-license our own distributed
   binaries if we want GPL-only codecs.
2. Confirm the BitTorrent crate chosen in Sprint 7 has no GPL dependencies.
3. Add a `THIRD_PARTY_NOTICES.md` generated from `cargo license` + `pnpm licenses`
   to every release artifact.
