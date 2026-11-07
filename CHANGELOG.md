# Changelog

## 0.1.0 — 2026-11-07

First public release of the independent Rust client for GSK Ethernet file transfer.

- Upload, download, directory listing, metadata and SHA-256 read-back verification.
- Original three-argument upload shorthand: `gsk IP FILE O1234.CNC`.
- Direct UDP protocol implementation; no MiniSock.dll, GSKComm or .NET dependency.
- Windows x64 executable with static CRT and a static Linux x64 musl executable.
- Offset-aware retries, response validation and explicit errors for ambiguous timeouts.
- Remote no-clobber preflight, staged local downloads and Unicode local paths.
- Live controller tests of 40, 80 and 132,557-byte programs; Windows and Linux/WSL.
- Local UDP fault-injection suite, protocol notes and captured read-only packet examples.

Known scope: file exchange with the tested GSK controller. Other firmware, macOS,
ARM, RS-232, DNC and remote program execution have not been validated or implemented.
