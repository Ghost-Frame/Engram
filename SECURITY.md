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

## Acknowledgments

We appreciate responsible disclosure. Contributors who report valid security issues will be acknowledged here (with permission).
