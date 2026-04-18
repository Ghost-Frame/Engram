#!/usr/bin/env sh
# Kleos installer -- detects OS/arch and downloads the correct binaries.
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/Ghost-Frame/Engram/main/dist/install.sh | sh
#
# Options (via environment variables):
#   KLEOS_VERSION   -- version to install (default: latest)
#   KLEOS_INSTALL   -- installation directory (default: ~/.local/bin)
#   KLEOS_BINARIES  -- space-separated list of binaries (default: kleos-server kleos-cli kleos-mcp)

set -eu

REPO="Ghost-Frame/Engram"
VERSION="${KLEOS_VERSION:-}"
INSTALL_DIR="${KLEOS_INSTALL:-$HOME/.local/bin}"
BINARIES="${KLEOS_BINARIES:-kleos-server kleos-cli kleos-mcp}"

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

# ─── Main ────────────────────────────────────────────────────────────────────

main() {
    detect_platform
    resolve_version

    echo "Installing Kleos v${VERSION} (${SUFFIX})"
    echo "  Binaries: ${BINARIES}"
    echo "  Target:   ${INSTALL_DIR}"
    echo ""

    mkdir -p "$INSTALL_DIR"

    base_url="https://github.com/${REPO}/releases/download/v${VERSION}"
    failed=""

    for bin in $BINARIES; do
        url="${base_url}/${bin}-${SUFFIX}"
        dest="${INSTALL_DIR}/${bin}"

        printf "  %-20s" "$bin"
        if download "$url" "$dest" 2>/dev/null; then
            chmod +x "$dest"
            echo "OK"
        else
            echo "FAILED"
            failed="${failed} ${bin}"
        fi
    done

    echo ""

    if [ -n "$failed" ]; then
        echo "Warning: failed to download:${failed}" >&2
        echo "Check https://github.com/${REPO}/releases/tag/v${VERSION}" >&2
    fi

    # Check if install dir is on PATH
    case ":${PATH}:" in
        *":${INSTALL_DIR}:"*) ;;
        *)
            echo "Add to your shell profile:"
            echo "  export PATH=\"${INSTALL_DIR}:\$PATH\""
            echo ""
            ;;
    esac

    echo "Done. Verify with: kleos-server --version"
}

main
