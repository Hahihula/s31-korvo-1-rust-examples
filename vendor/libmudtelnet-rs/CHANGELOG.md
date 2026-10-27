# Changelog

All notable changes to this project will be documented in this file.

The format is inspired by Keep a Changelog, and this project adheres to
Semantic Versioning.

## [2.0.10] - 2025-09-15

### Changed
- GMCP and MSDP are now treated as bidirectional once the server sends `IAC WILL <opt>` and the client responds `IAC DO <opt>`. After this handshake:
  - Incoming subnegotiations for GMCP/MSDP are accepted when either side is active (remote||local state).
  - Outgoing subnegotiations for GMCP/MSDP are permitted when the remote side is active, even if the client never sent `WILL`.
  - Rationale: Matches real‑world MUD server behavior and the GMCP/MSDP guidance, avoiding deadlocks where servers don’t echo `DO` to client `WILL`.

### Added
- Tests covering GMCP/MSDP bidirectional behavior after `WILL/DO` handshake.
- Documentation notes in `src/lib.rs` explaining the GMCP/MSDP policy.

### Compatibility
- No public API changes. Behavior remains compatible with libtelnet‑rs for general options; only GMCP/MSDP gating was relaxed to improve interoperability.

### Performance
- No measurable changes in hot paths. Parsing/dispatch logic changes are conditional checks only.

---

## [2.0.9] - 2025-xx-xx
- Previous entries omitted here; see git history for details.

