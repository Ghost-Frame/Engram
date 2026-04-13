# Security Policy

## Supported Versions

| Version | Supported          |
| ------- | ------------------ |
| 0.2.x   | Yes                |
| < 0.2   | No                 |

## Reporting a Vulnerability

**Do not open a public issue for security vulnerabilities.**

Email security reports to: **security@syntheos.dev**

Include:
- Description of the vulnerability
- Steps to reproduce
- Affected versions
- Any potential mitigations you've identified

You can expect an initial response within 72 hours. We'll work with you to understand the issue and coordinate disclosure.

## Security Model

### Authentication

- All API endpoints require `Authorization: Bearer eg_...` tokens
- Tokens are scoped per-user with role-based access control
- Bootstrap endpoint is one-time only (returns 403 after first use)
- Rate limiting is applied after auth resolution to prevent tenant enumeration

### Data Isolation

- Every query is scoped by `user_id` -- no cross-tenant data access
- Parent lookups during versioning are also scoped
- Spaces provide additional isolation within a tenant

### Input Validation

- FTS5 queries are sanitized against injection and DoS patterns
- SQL uses parameterized queries exclusively
- Webhook URLs are validated against SSRF (blocks localhost, link-local, metadata endpoints)
- All user input is bounded (max lengths enforced)

### Credential Management

- `engram-credd` handles secrets with ChaCha20-Poly1305 encryption
- Master password never stored -- used to derive encryption key
- Agent keys are scoped and rotatable
- `allow_raw` flag is explicitly opt-in and logged

### Audit Trail

- Every mutation is logged: who, what, when, from where
- Audit log is append-only
- Rate limit violations are logged with resolved tenant ID

### Network Security

- Default bind is `127.0.0.1` (localhost only)
- Webhook callbacks block private IP ranges:
  - `127.0.0.0/8` (loopback)
  - `10.0.0.0/8`, `172.16.0.0/12`, `192.168.0.0/16` (RFC1918)
  - `169.254.0.0/16` (link-local, including AWS metadata)
  - `0.0.0.0/8` (invalid)

### Dependencies

- ONNX models run locally via `ort` -- no external API calls for embeddings
- Optional LLM integration requires explicit configuration
- HuggingFace model downloads can be disabled with `ENGRAM_EMBEDDING_OFFLINE_ONLY=1`

## Security Hardening Checklist

For production deployments:

- [ ] Set `ENGRAM_API_KEY` to a strong, unique value
- [ ] Bind to localhost and use a reverse proxy (nginx, Caddy) for TLS
- [ ] Set `ENGRAM_EMBEDDING_OFFLINE_ONLY=1` if running air-gapped
- [ ] Configure `ENGRAM_EIDOLON_GATE_BLOCKED_PATTERNS` for dangerous commands
- [ ] Set up log rotation for audit logs
- [ ] Run as a non-root user
- [ ] Use separate API keys per agent/integration
- [ ] Rotate keys periodically via `/keys/rotate`

## Known Limitations

- GUI password (`ENGRAM_GUI_PASSWORD`) is checked via timing-safe comparison but transmitted in plaintext without TLS
- Session tokens in URLs may appear in server logs
- No built-in TLS -- use a reverse proxy

## Audit Log (April 2026)

### Fixed findings

| Finding | Severity | Commit | Description |
|---------|----------|--------|-------------|
| SSRF in POST /fetch | HIGH | aa61cd8 | Replaced IP blocklist with DNS-rebind-safe URL validation + 10 MiB stream limit |
| Passport verify timing attack | LOW | d06444c | Switched `==` to `subtle::ConstantTimeEq` in `verify_signed_value()` |
| MCP HTTP rate limit spoofing | LOW | 63a4c00 | Rate limit now uses `ConnectInfo<SocketAddr>` instead of `x-forwarded-for` |
| credd lacks pre-auth throttle | LOW | 0f3d685 | Added per-IP rate limiter (10 req/60s) before auth middleware |

### Dependency advisories (blocked on upstream)

These are transitive dependencies that cannot be upgraded without upstream releases.

**engram-migrate only** (via libsql 0.6.0 -- does not affect server, CLI, or credd):

| Advisory | Crate | Status |
|----------|-------|--------|
| RUSTSEC-2026-0049 | rustls-webpki 0.102.8 | libsql pins rustls 0.22.4; needs rustls 0.23+ |
| RUSTSEC-2025-0141 | bincode 1.3.3 | libsql depends on bincode 1.x |
| RUSTSEC-2025-0134 | rustls-pemfile 2.2.0 | libsql via hyper-rustls 0.25 |

**engram-lib** (via lancedb -> tantivy 0.24.2, tokenizers):

| Advisory | Crate | Status |
|----------|-------|--------|
| RUSTSEC-2026-0002 | lru 0.12.5 | lancedb 0.27 still pins tantivy 0.24; tantivy 0.26 exists |
| RUSTSEC-2026-0097 | rand 0.8.5 | tantivy (rand_distr), libsql (tower 0.4). Advisory targets `rand::rng()` which is 0.9+ only; not callable in 0.8 |
| RUSTSEC-2024-0436 | paste 1.0.15 | tokenizers, lance-bitpacking. Unmaintained but no known vulnerability |

None of these advisories are exploitable in engram's usage patterns.

### Build issue: engram-migrate

`engram-migrate` has a linker conflict between libsqlite3-sys (SQLCipher) and libsql-ffi (bundled sqlite3). Both define the same sqlite3 symbols. This is a one-shot ETL utility and does not affect the server, CLI, or credential daemon.

## Acknowledgments

We appreciate responsible disclosure. Contributors who report valid security issues will be acknowledged here (with permission).
