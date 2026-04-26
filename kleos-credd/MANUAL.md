# credd(8) -- credential daemon for Kleos bootstrap and three-tier secret resolution

Cross-platform manual for the `credd` daemon. Mirrors `man 8 credd` on
Linux/macOS; this is the canonical reference on Windows and any host
without `man`.

## Synopsis

```
credd [--listen ADDR] [--db-path PATH] [--auth-mode yubikey|password]
      [--master-password PASSWORD]
```

## Description

`credd` is the long-running daemon that backs the `cred` CLI and brokers
per-agent Kleos bearers to local clients (kleos-cli, kleos-sidecar, shell
hooks). It runs as a per-user systemd service on Linux and as a service on
Windows. It binds either a Unix socket (`CREDD_SOCKET`) or a TCP listener
(`CREDD_BIND`), or both.

### Startup sequence

1. Derives its 32-byte master key. With `--auth-mode=yubikey` (default) it
   sends the persisted challenge from `~/.config/cred/challenge` to YubiKey
   slot 2 (HMAC-SHA1) and runs the response through Argon2id (salt
   `cred-yubikey-v1\0`, m=19MiB, t=2, p=1). With `--auth-mode=password` it
   reads `CREDD_MASTER_PASSWORD` or stdin and uses the same Argon2id KDF.
2. Loads `~/.config/cred/bootstrap.enc` (the YubiKey-sealed CBv1 blob),
   decrypts it with the master key, splits header from key at the first
   0x1E byte, and holds the bare bearer in `Zeroizing<String>` in process
   memory. This is the privileged Kleos bearer used for `[CRED:v3]` fetches.
3. Loads the file-backed bootstrap-scoped agent keys from
   `~/.config/cred/agent-keys.json`.
4. Connects to the SQLite DB at `--db-path` (default `kleos.db`), runs
   migrations, and binds the configured listeners.

## Endpoints

### Bootstrap

#### `GET /bootstrap/kleos-bearer?agent=<slot>`

Auth: owner key OR file-backed agent key with `bootstrap/<slot>` scope (or
`bootstrap/*` or `*`).

Special case: if `agent` equals `credd-$hostname`, returns `bootstrap_master`
itself (owner-only).

Otherwise: credd uses `bootstrap_master` as a Kleos bearer to fetch
`[CRED:v3] engram-rust/<agent>` from `$KLEOS_URL/list?category=credential`,
hex-decodes the value, decrypts with the master key, returns the bare
per-agent bearer along with `expires_at` (RFC 3339) and `ttl_secs` (default
3600).

Response shape:

```json
{
  "key": "<bare-kleos-bearer>",
  "expires_at": "2026-04-26T10:00:00Z",
  "ttl_secs": 3600
}
```

### Three-tier resolve (DB-backed agent keys)

- `POST /resolve/text` -- substitution
- `POST /resolve/proxy` -- proxy
- `POST /resolve/raw` -- raw fetch
- `GET /secret/{category}/{name}` -- DB-backed secret lookup

All require category-permissioned DB-backed agent keys. Audit rows are
written to `cred_audit`.

### Agents (DB-backed only)

- `GET /agents` -- list
- `POST /agents` -- create
- `DELETE /agents/{id}` -- revoke

These manage DB-backed agent keys (NOT the file-backed bootstrap store).

### Misc

- `GET /health` -- liveness probe. Returns 200 if the master key is loaded
  and the DB is reachable.

## Auth tiers

Three auth tiers, in order of privilege:

1. **Master / owner.** `Authorization: Bearer <owner_key>` where `owner_key`
   is `hex::encode(master_key[..16])`. Full access. Used by `cred` itself
   for vault operations.
2. **DB-backed agent keys.** `Authorization: Bearer <key>` where the key
   matches a row in `cred_agent_keys`. Permissions are category-scoped.
   Validated for the resolve and secret endpoints.
3. **File-backed bootstrap-agent keys.** `Authorization: Bearer <key>`
   where the key matches an entry in `~/.config/cred/agent-keys.json`.
   Scope-checked against `bootstrap/<slot>` (or `bootstrap/*`, `*`).
   Validated only for `/bootstrap/kleos-bearer`.

The brute-force throttle in `auth_middleware` applies to all tiers;
Unix-socket connections skip the IP-based portion of the rate limiter.

## Environment

| Variable | Default | Purpose |
|----------|---------|---------|
| `CREDD_AUTH_MODE` | `yubikey` | `yubikey` or `password`. |
| `CREDD_MASTER_PASSWORD` | unset | Master password (only with `--auth-mode=password`). |
| `CREDD_SOCKET` | unset (Linux: `/run/user/$UID/credd.sock` if configured) | Unix socket path. |
| `CREDD_BIND` | unset | TCP bind address (e.g. `127.0.0.1:4400`). |
| `CREDD_LISTEN` | `127.0.0.1:4400` | Fallback `--listen` value. |
| `CREDD_DB_PATH` | `kleos.db` | SQLite DB path. |
| `CREDD_BOOTSTRAP_BLOB` | `~/.config/cred/bootstrap.enc` | Override path for `bootstrap.enc`. |
| `CREDD_AGENT_KEYS_FILE` | `~/.config/cred/agent-keys.json` | Override path for the bootstrap-scoped agent-keys store. |
| `KLEOS_URL` (or `ENGRAM_URL`) | unset | Kleos API endpoint. Required for `/bootstrap/kleos-bearer`. |

If neither `CREDD_SOCKET` nor `CREDD_BIND` is set, the `--listen` CLI flag
value is used as `CREDD_BIND`.

## Files

| Path | Purpose |
|------|---------|
| `~/.config/cred/challenge` | YubiKey challenge for master key derivation. |
| `~/.config/cred/bootstrap.enc` | CBv1-sealed Kleos master bearer. Loaded at startup. |
| `~/.config/cred/agent-keys.json` | File-backed bootstrap-scoped agent tokens. |
| `~/.config/systemd/user/credd.service` | Linux systemd unit. Restart with `systemctl --user restart credd`. |
| `$XDG_RUNTIME_DIR/credd.sock` | Default Unix socket path (chmod 0600). |

### Windows specifics

- credd runs as a Windows service under the user account (no systemd).
- Unix sockets are unsupported on Windows; credd binds TCP only.
  `CREDD_BIND=127.0.0.1:4400` is the standard.
- YubiKey access uses `ykchallenge.exe` (HID-direct) instead of `ykman` to
  avoid HID exclusive-access issues with Python wrappers.
- Config dir is `C:\Users\<you>\.config\cred\` (XDG resolution applies).

## Startup diagnostics

After starting credd, verify:

### Linux

```bash
systemctl --user is-active credd
journalctl --user -u credd -n 20 | grep -E "yubikey|bootstrap.enc|listening"
```

### Windows (PowerShell)

```powershell
Get-Service credd
Get-EventLog -LogName Application -Source credd -Newest 20
```

Expected log lines on either platform:

- `credd: master key derived from YubiKey` (or `from password` in password mode)
- `bootstrap.enc loaded`
- `credd listening on unix:<socket>` and/or `credd listening on tcp:<addr>`

If `challenge_response` fails in service context on Linux, credd falls back
to `sudo python3 -c "from ykman..."` (needed where the bare ykman wrapper
script can't take the HID).

## Bootstrap flow (end-to-end)

The flow that bootstraps an agent host with zero plaintext on disk:

1. Master taps YubiKey, runs `cred init`, `cred store kleos credd-<host>`,
   `cred bootstrap wrap kleos credd-<host>`. This produces `bootstrap.enc`.
2. Master runs `cred agent-key generate shell-<host> --scope
   bootstrap/<slot>` and writes the printed token to
   `~/.config/cred/credd-agent-key.token`.
3. Shell rc exports `CREDD_SOCKET` (or `CREDD_URL` on Windows),
   `CREDD_AGENT_KEY`, `KLEOS_AGENT_SLOT`, `KLEOS_URL`.
4. Agent (kleos-cli, sidecar, hook) calls
   `GET /bootstrap/kleos-bearer?agent=$KLEOS_AGENT_SLOT` with
   `Authorization: Bearer $CREDD_AGENT_KEY`.
5. credd validates the bootstrap-scoped agent key, fetches the `[CRED:v3]`
   entry from Kleos using `bootstrap_master`, decrypts with `master_key`,
   returns the bare per-agent bearer and a TTL hint.
6. Client caches the bearer until `expires_at` and uses it directly against
   Kleos endpoints.

The pre-shared `credd-agent-key.token` file is the current weak link in
this flow. Replacement design (P-256 ECDH via YubiKey PIV applet, slots 9D
+ 9A) is at `~/projects/plans/2026-04-26-ecdh-bootstrap-auth-piv.md`.

## See also

- [cred(1)](../kleos-cred/MANUAL.md) -- the CLI that talks to this daemon
- `~/projects/plans/2026-04-26-ecdh-bootstrap-auth-piv.md` -- ECDH PIV
  replacement spec
