#!/usr/bin/env bash
set -euo pipefail
LOCK_DIR="/tmp/kleos-cargo-locks"
mkdir -p "$LOCK_DIR"
exec flock -x "$LOCK_DIR/global.lock" \
  env CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-8}" \
  cargo "$@"
