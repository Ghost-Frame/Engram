#!/usr/bin/env sh
# Kleos installer -- detects OS/arch and downloads the correct binaries.
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/Ghost-Frame/Engram/main/dist/install.sh | sh
#
# Options (via environment variables):
#   KLEOS_VERSION   -- version to install (default: latest)
#   KLEOS_INSTALL   -- installation directory (default: ~/.local/bin)
#   KLEOS_PROFILE   -- "server" (default), "agent-host", or "full"
#   KLEOS_BINARIES  -- space-separated list (overrides KLEOS_PROFILE if set)

set -eu

REPO="Ghost-Frame/Engram"
VERSION="${KLEOS_VERSION:-}"
INSTALL_DIR="${KLEOS_INSTALL:-$HOME/.local/bin}"
PROFILE="${KLEOS_PROFILE:-server}"

# Resolve KLEOS_BINARIES from profile if not set explicitly
if [ -n "${KLEOS_BINARIES:-}" ]; then
    BINARIES="$KLEOS_BINARIES"
else
    case "$PROFILE" in
        agent-host|agent)
            BINARIES="kleos-cli kleos-sh kr kw ke agent-forge eidolon-supervisor kleos-cred kleos-credd"
            ;;
        full)
            BINARIES="kleos-server kleos-cli kleos-sidecar kleos-credd kleos-cred kleos-mcp kleos-sh kr kw ke agent-forge eidolon-supervisor"
            ;;
        *)
            BINARIES="kleos-server kleos-cli kleos-mcp"
            ;;
    esac
fi

# ─── Detect platform ────────────────────────────────────────────────────────

detect_platform() {
    os="$(uname -s)"
    arch="$(uname -m)"

    case "$os" in
        Linux)  os_suffix="linux" ;;
        Darwin) os_suffix="darwin" ;;
        *)
            echo "error: unsupported OS: $os" >&2
            echo "       use WSL on Windows, or see dist/install.ps1 for PowerShell" >&2
            exit 1
            ;;
    esac

    case "$arch" in
        x86_64|amd64)   arch_suffix="x64" ;;
        aarch64|arm64)  arch_suffix="arm64" ;;
        *)
            echo "error: unsupported architecture: $arch" >&2
            exit 1
            ;;
    esac

    SUFFIX="${os_suffix}-${arch_suffix}"
}

# ─── Resolve latest version ─────────────────────────────────────────────────

resolve_version() {
    if [ -n "$VERSION" ]; then
        return
    fi

    echo "Fetching latest release..."
    if command -v curl >/dev/null 2>&1; then
        VERSION="$(curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" | grep '"tag_name"' | head -1 | sed 's/.*"v\([^"]*\)".*/\1/')"
    elif command -v wget >/dev/null 2>&1; then
        VERSION="$(wget -qO- "https://api.github.com/repos/${REPO}/releases/latest" | grep '"tag_name"' | head -1 | sed 's/.*"v\([^"]*\)".*/\1/')"
    else
        echo "error: need curl or wget to detect latest version" >&2
        echo "       set KLEOS_VERSION manually to bypass" >&2
        exit 1
    fi

    if [ -z "$VERSION" ]; then
        echo "error: could not determine latest version" >&2
        exit 1
    fi
}

# ─── Download ────────────────────────────────────────────────────────────────

download() {
    url="$1"
    dest="$2"

    if command -v curl >/dev/null 2>&1; then
        curl -fsSL -o "$dest" "$url"
    elif command -v wget >/dev/null 2>&1; then
        wget -qO "$dest" "$url"
    fi
}

# ─── SHA-256 verification ────────────────────────────────────────────────────
#
# Every release publishes a SHASUMS256.txt file alongside the binaries.  Each
# line is `<sha256>  <filename>`.  We download that manifest once, then verify
# each binary after it lands.  A verification failure deletes the partial
# download and aborts the installer so a MITM or tampered mirror cannot leave
# a compromised executable on disk.

pick_sha256_tool() {
    if command -v sha256sum >/dev/null 2>&1; then
        SHA256_CMD="sha256sum"
    elif command -v shasum >/dev/null 2>&1; then
        SHA256_CMD="shasum -a 256"
    else
        SHA256_CMD=""
    fi
}

compute_sha256() {
    # Prints only the hex digest on stdout.
    # shellcheck disable=SC2086
    $SHA256_CMD "$1" | awk '{print $1}'
}

verify_sha256() {
    file="$1"
    expected="$2"
    if [ -z "$SHA256_CMD" ] || [ -z "$expected" ]; then
        return 0
    fi
    actual="$(compute_sha256 "$file")"
    if [ "$actual" != "$expected" ]; then
        rm -f "$file"
        echo ""
        echo "error: SHA-256 mismatch for $(basename "$file")" >&2
        echo "  expected: $expected" >&2
        echo "  actual:   $actual" >&2
        return 1
    fi
    return 0
}

lookup_expected_sha256() {
    # $1 = filename as it appears in SHASUMS256.txt
    file="$1"
    [ -n "${SHASUMS_FILE:-}" ] && [ -f "$SHASUMS_FILE" ] || return 0
    awk -v f="$file" '$2 == f || $2 == "*"f {print $1; exit}' "$SHASUMS_FILE"
}

# ─── Main ────────────────────────────────────────────────────────────────────

main() {
    detect_platform
    resolve_version
    pick_sha256_tool

    echo "Installing Kleos v${VERSION} (${SUFFIX})"
    echo "  Binaries: ${BINARIES}"
    echo "  Target:   ${INSTALL_DIR}"
    echo ""

    mkdir -p "$INSTALL_DIR"

    base_url="https://github.com/${REPO}/releases/download/v${VERSION}"
    failed=""

    # Fetch the release's SHASUMS256.txt once.  We deliberately do NOT make
    # the overall install depend on reaching it (a release may predate the
    # SHA manifest), but if we have it we enforce strict verification below.
    SHASUMS_FILE="$(mktemp -t kleos-shasums.XXXXXX)"
    if download "${base_url}/SHASUMS256.txt" "$SHASUMS_FILE" 2>/dev/null \
            && [ -s "$SHASUMS_FILE" ]; then
        if [ -z "$SHA256_CMD" ]; then
            echo "warn: no sha256sum/shasum available; skipping integrity check" >&2
            rm -f "$SHASUMS_FILE"
            SHASUMS_FILE=""
        else
            echo "  manifest: SHASUMS256.txt fetched; integrity will be verified"
        fi
    else
        rm -f "$SHASUMS_FILE"
        SHASUMS_FILE=""
        echo "warn: SHASUMS256.txt not found for this release; integrity NOT verified" >&2
    fi
    echo ""

    for bin in $BINARIES; do
        fname="${bin}-${SUFFIX}"
        url="${base_url}/${fname}"
        dest="${INSTALL_DIR}/${bin}"

        printf "  %-20s" "$bin"
        if ! download "$url" "$dest" 2>/dev/null; then
            echo "FAILED (download)"
            failed="${failed} ${bin}"
            continue
        fi
        expected="$(lookup_expected_sha256 "$fname")"
        if [ -n "$SHASUMS_FILE" ] && [ -z "$expected" ]; then
            echo "FAILED (no checksum entry)"
            rm -f "$dest"
            failed="${failed} ${bin}"
            continue
        fi
        if ! verify_sha256 "$dest" "$expected"; then
            echo "FAILED (sha256 mismatch)"
            failed="${failed} ${bin}"
            continue
        fi
        chmod +x "$dest"
        if [ -n "$expected" ]; then
            echo "OK (verified)"
        else
            echo "OK (unverified)"
        fi
    done

    [ -n "$SHASUMS_FILE" ] && rm -f "$SHASUMS_FILE"
    echo ""

    if [ -n "$failed" ]; then
        echo "Warning: failed to install:${failed}" >&2
        echo "Check https://github.com/${REPO}/releases/tag/v${VERSION}" >&2
        exit 1
    fi

    # Agent-host profile: drop sample supervisor config if none exists
    case "$PROFILE" in
        agent-host|agent|full)
            supervisor_config_dir="$HOME/.config/eidolon"
            if [ ! -f "$supervisor_config_dir/supervisor.json" ]; then
                mkdir -p "$supervisor_config_dir"
                if [ -f "$(dirname "$0")/supervisor.example.json" ]; then
                    cp "$(dirname "$0")/supervisor.example.json" "$supervisor_config_dir/supervisor.json"
                else
                    # Inline minimal default when running from curl pipe
                    cat > "$supervisor_config_dir/supervisor.json" <<'RULES'
[
  {"id":"no-force-push","check_type":"rule_match","pattern":"git\\s+push\\s+.*--force","severity":"critical","cooldown_secs":300,"message":"Force push detected"},
  {"id":"no-reboot","check_type":"rule_match","pattern":"reboot|shutdown|systemctl\\s+(reboot|poweroff)","severity":"critical","cooldown_secs":600,"message":"Reboot or shutdown command detected"},
  {"id":"retry-loop","check_type":"retry_loop","pattern":"","severity":"warning","cooldown_secs":120,"message":"Agent stuck in retry loop (3+ identical failing commands)"}
]
RULES
                fi
                echo "  Dropped sample config: $supervisor_config_dir/supervisor.json"
            fi

            # Drop supervisor env file template if none exists
            if [ ! -f "$supervisor_config_dir/supervisor.env" ]; then
                cat > "$supervisor_config_dir/supervisor.env" <<'ENV'
# Eidolon supervisor environment
# KLEOS_SERVER_URL=http://172.30.0.201:4200
# KLEOS_API_KEY=  (prefer cred/credd over plaintext)
# CLAUDE_SESSIONS_DIR=~/.claude/projects
ENV
                echo "  Dropped template: $supervisor_config_dir/supervisor.env"
            fi
            echo ""
            ;;
    esac

    # Check if install dir is on PATH
    case ":${PATH}:" in
        *":${INSTALL_DIR}:"*) ;;
        *)
            echo "Add to your shell profile:"
            echo "  export PATH=\"${INSTALL_DIR}:\$PATH\""
            echo ""
            ;;
    esac

    verify_cmd="kleos-cli --version"
    case "$PROFILE" in
        server) verify_cmd="kleos-server --version" ;;
    esac
    echo "Done. Verify with: $verify_cmd"
}

main
