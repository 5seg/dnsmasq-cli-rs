#!/bin/sh
# dnsmasq-cli-rs をリリースビルドする。`--deploy` で vlproxy へ配置する。
set -eu
cd "$(dirname "$0")"

HOST_TRIPLE="$(rustc -vV | awk '/^host:/{print $2}')"
TARGET="${TARGET:-$HOST_TRIPLE}"
BIN="target/$TARGET/release/dnsmasq-cli-rs"

# musl ターゲットは完全静的リンクにする (対象クレートだけに適用し、
# proc-macro のビルドを壊さないようターゲット個別の RUSTFLAGS を使う)。
TRIPLE_ENV="$(printf '%s' "$TARGET" | tr 'a-z-' 'A-Z_')"
if printf '%s' "$TARGET" | grep -q musl; then
    eval "export CARGO_TARGET_${TRIPLE_ENV}_RUSTFLAGS='-C target-feature=+crt-static'"
fi

echo "[build] target=$TARGET"
cargo build --release --target "$TARGET"

file "$BIN" || true
ls -lh "$BIN"

if [ "${1:-}" = "--deploy" ]; then
    HOST="${HOST:-vlproxy}"
    DEST="${DEST:-/usr/local/bin/dnsmasq-cli-rs}"
    echo "[deploy] $BIN -> $HOST:$DEST"
    scp "$BIN" "$HOST:/tmp/dnsmasq-cli-rs.new"
    ssh "$HOST" "sudo install -m0755 /tmp/dnsmasq-cli-rs.new '$DEST' \
        && rm -f /tmp/dnsmasq-cli-rs.new && echo '[deploy] installed'"
fi
