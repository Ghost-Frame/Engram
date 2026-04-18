# GUI authentication

## Decision: bearer tokens + API keys only. No Google OAuth.

The original TypeScript engram shipped a Google OAuth login flow for the web
GUI (`src/auth/google-auth.ts`). engram-rust deliberately does not port this.

## Why

1. **Reduced dependency surface.** OAuth adds a Google client library, a
   callback route, cookie/session state, and a secret-rotation story. Bearer
   + API keys need none of that.
2. **Self-host posture.** engram is deployed by a single operator (or a
   small team) against their own data. A Google account as the gatekeeper
   is strictly worse than a token the operator controls directly.
3. **No third-party trust path.** With bearer-only, a compromised Google
   account cannot read memory. The only credential is the one the operator
   issues and revokes.
4. **Key rotation already exists.** `/auth/keys` supports multi-key per
   user with TTL and grace windows (plan 3.14); that's the rotation story.
5. **Browser flow works.** The GUI reads the bearer from a cookie set by
   the `/login` endpoint against a locally-issued API key. No redirect, no
   consent screen.

## Threat model

- **Lost laptop with cookie:** cookie expires on a TTL; revoke the API key
  via `/auth/keys/:id` to kill all sessions instantly.
- **Phished API key:** revoke and rotate; the grace window lets the legit
  client roll without downtime.
- **Server compromise:** same blast radius as OAuth (both expose memory).
  Mitigations are SQLCipher at rest, per-user BOLA checks, and audit log.

## If OAuth is ever needed

Add a separate auth provider trait (`trait AuthProvider`) with OAuth as
one impl. Keep bearer as the default. Do not replace bearer; augment it.

## References

- `engram-server/src/extractors.rs` -- bearer extraction
- `engram-server/src/routes/auth_keys/` -- key lifecycle
- `engram-server/src/routes/gui/` -- cookie-based session
- `SECURITY.md` -- full security posture
