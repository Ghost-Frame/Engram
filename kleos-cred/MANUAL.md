# cred(1) -- YubiKey-encrypted credential vault and broker for Kleos bootstrap

Cross-platform manual for the `cred` CLI. Mirrors `man cred` on Linux/macOS;
this is the canonical reference on Windows and any host without `man`.

## Synopsis

```
cred COMMAND [OPTIONS] [ARGS]
```

## Description

`cred` is the user-facing CLI for the canonical credential system in
`engram-rust`. It manages an AES-256-GCM encrypted vault of secrets unlocked
by a YubiKey HMAC-SHA1 challenge-response on slot 2, talks to the local
`credd` daemon over a Unix socket (or TCP on Windows) to broker per-agent
Kleos bearers, and produces / consumes the YubiKey-sealed `bootstrap.enc`
blob that gives credd its privileged Kleos master key.

The vault stores both local secrets (kept in the cred SQLite DB) and
`[CRED:v3]` entries (encrypted secrets stored as memory rows in Kleos,
fetched by credd during bootstrap). The two stores serve different purposes;
see Files below.

## Commands

### Vault management

#### `cred init`
First-time setup. Generates a 20-byte HMAC secret, programs YubiKey slot 2
with it, derives the Argon2id master key, writes the persisted challenge to
`~/.config/cred/challenge`, and creates a recovery kit at
`~/.config/cred/recovery.enc`. Set `CRED_ALLOW_INIT=1` to bypass the safety
guard that prevents accidental re-init.

#### `cred store SERVICE KEY [-t TYPE]`
Store a secret interactively. `TYPE` is one of api-key, login, oauth-app,
ssh-key, note, environment (default: api-key).

#### `cred get SERVICE KEY [-f FIELD] [-r]`
Retrieve a secret. With `-f` pulls a single field (e.g. password, username).
With `-r` prints the raw value, no JSON wrapping. Triggers a YubiKey unlock.

#### `cred list`
List all stored secrets with values redacted.

#### `cred delete SERVICE KEY [-y]`
Delete a secret. `-y` skips confirmation.

#### `cred import [-n]`
Bulk import from stdin in `service<TAB>key<TAB>value` format. `-n` is
dry-run.

#### `cred export`
Dump all secrets as JSON for backup or migration.

#### `cred recover [-f FILE]`
Decrypt `recovery.enc` (default `~/.config/cred/recovery.enc`) and program a
fresh YubiKey with the recovered HMAC secret. Used when the primary YubiKey
is lost or replaced. The recovery file uses the `CRv2` format (4-iter
Argon2id, JSON payload of `{hmac_secret, challenge}`).

### Bootstrap blob (YubiKey-sealed credd master key)

#### `cred bootstrap wrap SERVICE KEY [--out PATH]`
Read an existing cred entry (typically the privileged Kleos bearer for this
host), wrap it with the YubiKey-derived master key plus a header, and write
`~/.config/cred/bootstrap.enc`. The on-disk format is:

- 4-byte magic `CBv1`
- AES-256-GCM ciphertext of `(header_json || 0x1E || bare_key_bytes)`, where
  `header_json` captures the source service/key and timestamp.

Run once per host during initial provisioning. credd reads this file at
startup; the bare key never touches disk in plaintext.

#### `cred bootstrap unwrap [--from PATH] [--raw]`
Decrypt `bootstrap.enc` and print the bare key to stdout. Manual escape
hatch for debugging; credd is the normal consumer. `--raw` omits the
trailing newline.

### Agent keys (DB-backed and file-backed)

cred maintains two distinct agent-key stores, gated by scope.

#### `cred agent-key generate NAME [-d DESC] [--scope SCOPE ...]`
Without `--scope`, the key lands in the DB-backed `cred_agent_keys` table.
These keys gate the three-tier resolve handlers (`/resolve/text`,
`/resolve/proxy`, `/resolve/raw`) and `/secret/{cat}/{name}` endpoints with
category-based permissions.

With one or more `--scope bootstrap/<slot>` flags (or `bootstrap/*` or
`*`), the key lands in the file-backed store at
`~/.config/cred/agent-keys.json` (mode 0600) and gates the
`/bootstrap/kleos-bearer` endpoint. Mixing scope types in a single token is
rejected.

The token is printed once. Write it to
`~/.config/cred/credd-agent-key.token` for shell rc to pick up via
`CREDD_AGENT_KEY`.

#### `cred agent-key list`
Lists DB-backed keys only. For file-backed bootstrap tokens, read
`~/.config/cred/agent-keys.json` directly.

#### `cred agent-key revoke ID`
Revoke a DB-backed agent key.

### Interactive

#### `cred tui`
Launch the interactive TUI for browsing and managing secrets.

## Files

| Path | Purpose |
|------|---------|
| `~/.config/cred/challenge` | 32-byte challenge for YubiKey HMAC-SHA1 slot 2. Argon2id KDF derives the master key (salt `cred-yubikey-v1\0`, m=19MiB, t=2, p=1). |
| `~/.config/cred/cred.db` | SQLite vault for cred-managed secrets. |
| `~/.config/cred/bootstrap.enc` | YubiKey-sealed CBv1 blob containing credd's privileged Kleos master bearer. |
| `~/.config/cred/recovery.enc` | CRv2 file holding the HMAC secret + challenge, sealed by a recovery passphrase. |
| `~/.config/cred/agent-keys.json` | File-backed store for bootstrap-scoped agent tokens (mode 0600). |
| `~/.config/cred/credd-agent-key.token` | Plaintext file containing the active bootstrap-agent token, sourced by shell rc into `CREDD_AGENT_KEY`. |

On Windows, the equivalent paths live under `C:\Users\<you>\.config\cred\`
(the binary uses XDG resolution on every OS, so the same `.config\cred`
layout applies).

## Environment

| Variable | Default | Purpose |
|----------|---------|---------|
| `CRED_ALLOW_INIT` | unset | Required to be `1` for `cred init` to proceed. |
| `KLEOS_URL` (or `ENGRAM_URL`) | unset | Kleos API endpoint. |
| `KLEOS_API_KEY` (or `ENGRAM_API_KEY`) | unset | Bearer for Kleos. If unset, cred asks credd for the per-host bearer. |
| `CREDD_SOCKET` | `/run/user/$UID/credd.sock` (Linux) | Unix socket path that credd listens on. Unused on Windows; cred uses TCP via `CREDD_URL` instead. |
| `CREDD_URL` | `http://127.0.0.1:4400` | TCP endpoint for credd (Windows path). |

## Security model

- The 32-byte master key is never persisted. It is re-derived on every cred
  invocation by sending the persisted challenge to YubiKey slot 2
  (HMAC-SHA1, 20-byte response) and running the result through Argon2id.
- Vault entries are AES-256-GCM-encrypted with the master key; the nonce
  prefixes the ciphertext.
- `bootstrap.enc` uses the same master key with a CBv1 header so credd can
  self-bootstrap its Kleos bearer without prompting.
- The recovery file uses CRv2 (Argon2id with a passphrase, NOT the
  YubiKey). Print the passphrase, lock it in a safe.
- Bootstrap-agent tokens in `agent-keys.json` are the current weak link:
  plaintext on disk, scoped per agent slot, no rotation. Replacement
  design: see `~/projects/plans/2026-04-26-ecdh-bootstrap-auth-piv.md`
  (P-256 ECDH via YubiKey PIV slots 9D + 9A, eliminates the token file).

## Examples

### First-time setup on a new host

```bash
CRED_ALLOW_INIT=1 cred init
# (taps YubiKey, writes challenge + recovery.enc; print recovery passphrase)

cred store kleos credd-$(hostname) --secret-type api-key
# (paste bare bearer when prompted)

cred bootstrap wrap kleos credd-$(hostname)
# wraps the privileged Kleos master bearer for credd

cred agent-key generate shell-$(hostname) --scope "bootstrap/claude-code-${USER}-$(hostname)" \
    -d "shell+hooks credd auth"
# token prints once; save to ~/.config/cred/credd-agent-key.token
```

### Day-to-day

```bash
cred get authentik akadmin --field username
cred get forgejo rocky-push --raw         # taps YubiKey
cred list
```

### Recovering after losing the YubiKey

```bash
cred recover                              # prompts for passphrase, taps new YubiKey
```

## See also

- [credd(8)](../kleos-credd/MANUAL.md) -- the daemon backing this CLI
- `~/projects/plans/2026-04-26-ecdh-bootstrap-auth-piv.md` -- ECDH PIV
  replacement spec
