# Feature Checklist

Every feature from the original product spec, unabridged, as a checkbox tracked
against a delivery phase. Update this file's checkboxes as features land —
it is the single source of truth for "is X done yet."

Phases referenced here are defined in `ROADMAP.md`.

## 1. Core Download Engine — Phase 1
- [x] Multi-thread downloading, 1–128 connections
- [x] Dynamic thread allocation / automatic thread optimization
- [x] Resume individual thread
- [x] Split large files into chunks / merge chunks
- [ ] Adaptive chunk sizes
- [ ] Per-server connection limits
- [x] Smart resume (crash, shutdown, reboot, disconnect, VPN reconnect, proxy change, weeks later)
- [x] Intelligent retry (classify timeout/DNS/SSL/server-busy/bandwidth/HTTP errors)
- [x] Mirror support (auto switch, compare speed, continue from another mirror)
- [x] Dynamic segment allocation ("segment stealing")
- [x] Download verification (SHA1/SHA256/SHA512/CRC32/MD5)
- [x] Corruption detection with per-chunk re-download
- [x] Automatic file renaming on conflict
- [x] Duplicate detection (URL/hash/filename/content) with overwrite/rename/skip

## 2. Universal Protocol Support — Phase 1–3
- [ ] HTTP/1.1, HTTP/2, HTTP/3 — Phase 1
- [ ] HTTPS, TLS 1.2/1.3, certificate validation, pinned certs — Phase 1
- [x] FTP (download/upload/resume/folder listing) — Phase 1
- [x] FTPS — Phase 1
- [x] SFTP — Phase 2
- [x] SCP — Phase 2
- [x] WebDAV — Phase 2
- [ ] SMB — Phase 3
- [ ] NFS — Phase 3
- [x] Magnet links / BitTorrent / DHT / peer exchange / trackers — Phase 2
- [ ] Sequential download / torrent streaming — Phase 3
- [x] Piece verification — Phase 2
- [ ] Metalink (mirrors + hash verification) — Phase 2
- [ ] HLS (m3u8) — Phase 2
- [ ] MPEG-DASH (mpd) — Phase 2
- [ ] RTMP — Phase 3
- [ ] MMS — Phase 3
- [ ] RTSP — Phase 3

## 3. Browser Integration — Phase 2
- [ ] Chrome / Firefox / Edge / Brave / Opera / Vivaldi extensions (WebExtensions, shared codebase)
- [ ] Auto-detect downloads/video/audio/PDF/ZIP
- [ ] Clipboard monitoring with "Download?" popup
- [ ] Drag & drop (URL/HTML/file/text)
- [ ] Batch URL detection from pasted text

## 4. Video Downloading — Phase 2
- [ ] yt-dlp + FFmpeg integration (thousands of sites)
- [ ] Video/audio/subtitles/thumbnails/chapters/comments/metadata download
- [ ] Livestream / playlist / channel / album / podcast detection
- [ ] Quality selection 144p–8K
- [ ] Codec selection (AV1/VP9/H264/HEVC)
- [ ] Subtitle download + conversion (SRT/ASS/VTT)
- [ ] Automatic audio+video merge

## 5. Scheduling — Phase 3
- [ ] Schedule by date/time/recurring
- [ ] Post-download actions (shutdown/sleep/hibernate/logout/restart/run script/play sound)
- [ ] Time-window downloading (e.g. 2AM–5AM)
- [ ] Bandwidth schedule (different limits by time of day)

## 6. Bandwidth Manager — Phase 1
- [ ] Limit total / per-download / per-domain bandwidth
- [ ] Priority levels (High/Normal/Low)
- [ ] Auto-detect network congestion and back off

## 7. Download Queue — Phase 1
- [ ] Pause/resume queue
- [ ] Priority queue
- [ ] Nested queues
- [ ] Categories (Movies/Games/Images/Linux ISOs/Documents/...)

## 8. File Management — Phase 3
- [ ] Automatic folder routing by file type
- [ ] Custom rules (e.g. `*.pdf` → Documents)
- [ ] Automatic extraction (ZIP/RAR/7z/TAR)
- [ ] Virus scan integration via installed AV

## 9. Advanced Network Features — Phase 2
- [ ] Proxy: SOCKS4/SOCKS5/HTTP/HTTPS with auth
- [ ] VPN detection
- [ ] DNS over HTTPS
- [ ] IPv4 + IPv6, Happy Eyeballs
- [ ] Connection pooling, TLS session reuse

## 10. Authentication — Phase 2
- [ ] Login forms, cookies, sessions
- [ ] Bearer tokens, API keys
- [ ] OAuth
- [ ] Browser cookie import

## 11. Security — Phase 2
- [ ] Encrypted download history
- [ ] Encrypted credential storage
- [ ] Certificate pinning
- [ ] Download sandboxing
- [ ] File reputation / hash verification
- [ ] Digital signature verification

## 12. Automation — Phase 1–4
- [ ] CLI (`sdm`) — Phase 1
- [ ] REST API — Phase 1
- [ ] WebSocket events API — Phase 1
- [ ] Python SDK — Phase 4
- [ ] Plugin system — Phase 4
- [ ] Event hooks — Phase 3
- [ ] Automation rules engine — Phase 3

## 13. Search — Phase 2
- [ ] Search history/downloads/URLs
- [ ] Filter, sort, regex search

## 14. User Interface — Phase 1–2
- [x] Dark/light mode — Phase 1
- [ ] Tabs, docking panels — Phase 2
- [ ] Mini mode / floating monitor — Phase 2
- [x] Live speed graphs / download charts — Phase 1
- [x] Notifications, animations — Phase 1
- [ ] Keyboard shortcuts — Phase 2
- [ ] Touch support — Phase 3

## 15. Analytics — Phase 3
- [ ] Average/peak speed, server response time
- [ ] Download history, bandwidth charts
- [ ] Failure reasons, retry counts, statistics

## 16. Cross Platform — Phase 1
- [ ] Windows / macOS / Linux
- [ ] Android (future)

## 17. AI Features (Offline) — Phase 5
- [ ] Predict fastest mirror
- [ ] Predict best thread count
- [ ] Detect fake downloads
- [ ] Categorize files
- [ ] Suggest schedules
- [ ] Predict failures
- [ ] Bandwidth optimization
- [ ] Duplicate detection (content-based)
- [ ] Malware probability heuristic
- [ ] Natural-language search/commands

## 18. Plugin Marketplace — Phase 4
- [ ] Plugin install/manage UI
- [ ] Cloud storage plugins, custom extractors, website downloaders, themes

## 19. Cloud Features (Self-Hosted) — Phase 4
- [ ] Sync settings/queues/history/favorites across devices via self-hosted server

## 20. Developer Features — Phase 4
- [ ] REST API, WebSocket API, CLI (Phase 1 baseline) + Plugin/Theme/Extension/Automation SDKs (Phase 4)

## 21. Power User Features — Phase 3
- [ ] Regular expressions / filters / rules engine
- [ ] Download templates, macros
- [ ] Custom headers, cookie editor
- [ ] Packet logging, request/response inspector

## 22. Accessibility — Phase 3 (continuous)
- [ ] Screen reader support
- [ ] High contrast mode
- [ ] Keyboard navigation
- [ ] Large fonts
- [ ] Localization, RTL languages

## 23. Recovery Features — Phase 1
- [x] Recover interrupted downloads
- [x] Recover corrupted databases
- [x] Repair queues
- [x] Recover temporary files
- [x] Automatic backups, session restore

## 24. Enterprise Features — Phase 5
- [ ] Policy management, audit logs
- [ ] Centralized configuration
- [ ] User roles, download quotas
- [ ] Organization-wide settings

## 25. Differentiators — Phase 5
- [ ] AI-assisted mirror selection from historical performance
- [ ] Website-wide downloadable-asset extraction with type filters
- [ ] Built-in website crawler for asset discovery
- [ ] Download dependency graphs for installers
- [ ] Smart hash-based duplicate detection
- [ ] Automatic recovery from changed URLs via known mirrors
- [ ] Local OCR on downloaded docs/images
- [ ] Content-based organization via local AI
- [ ] Built-in checksum DB for popular Linux distros/OSS
- [ ] Extensible workflow automation (download → verify → extract → move → notify → run script)
