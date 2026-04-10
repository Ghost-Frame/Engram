#!/usr/bin/env sh
set -eu

cargo_home="${CARGO_HOME:-$HOME/.cargo}"

for protoc in "$cargo_home"/registry/src/*/protoc-bin-vendored-linux-x86_64-*/bin/protoc; do
    if [ -x "$protoc" ]; then
        exec "$protoc" "$@"
    fi
done

echo "vendored protoc binary not found; run cargo fetch and retry" >&2
exit 127
