#!/bin/sh
# dnsmasq-cli-rs の VPS (Alpine/OpenRC) 側セットアップ。冪等。root で実行する。
#
#   sudo sh setup.sh
#
# 前提: バイナリが /usr/local/bin/dnsmasq-cli-rs に配置済みであること
#       (build.sh --deploy で配置される)。
set -eu

SVC_USER="${SVC_USER:-user}"
BIN="/usr/local/bin/dnsmasq-cli-rs"
WL_DIR="/etc/dnsmasq-cli-rs"
WL_FILE="$WL_DIR/whitelist.txt"
DB_DIR="/var/lib/dnsmasq-cli-rs"
DSQ_LOG="/var/log/dnsmasq.log"
GEN="/etc/periodic/weekly/update-adblock.sh"

say() { printf '[setup] %s\n' "$*"; }

# ---- 0. 前提確認 -----------------------------------------------------------
if [ ! -x "$BIN" ]; then
    echo "error: $BIN がありません。先に build.sh --deploy を実行してください。" >&2
    exit 1
fi
if ! id "$SVC_USER" >/dev/null 2>&1; then
    echo "error: ユーザー '$SVC_USER' が存在しません (SVC_USER=... で指定可)。" >&2
    exit 1
fi
if ! command -v rc-service >/dev/null 2>&1; then
    echo "error: OpenRC (rc-service) が見つかりません。" >&2
    exit 1
fi

# ---- 1. dnsmasq: クエリログを専用ファイルへ --------------------------------
say "dnsmasq のログ設定を書き込みます"
cat > /etc/dnsmasq.d/logging.conf <<'CONF'
# dnsmasq クエリログを専用ファイルへ出力する (dnsmasq-cli-rs ingest が取り込む)
log-queries=extra
log-facility=/var/log/dnsmasq.log
CONF
chmod 0644 /etc/dnsmasq.d/logging.conf

# ---- 2. logrotate ----------------------------------------------------------
say "logrotate 設定を書き込みます"
cat > /etc/logrotate.d/dnsmasq <<CONF
$DSQ_LOG {
    daily
    rotate 7
    size 10M
    missingok
    notifempty
    copytruncate
    create 0644 root root
}
CONF
chmod 0644 /etc/logrotate.d/dnsmasq

# ---- 3. whitelist / DB ディレクトリ ---------------------------------------
say "whitelist と DB ディレクトリを用意します"
install -d -m0755 "$WL_DIR"
if [ ! -f "$WL_FILE" ]; then
    printf 'click.discord.com\n' > "$WL_FILE"
fi
chmod 0644 "$WL_FILE"
install -d -m0755 -o "$SVC_USER" -g "$SVC_USER" "$DB_DIR"

# ---- 4. 取り込みサービス (OpenRC) -----------------------------------------
say "OpenRC サービスを書き込みます"
cat > /etc/init.d/dnsmasq-cli-rs <<CONF
#!/sbin/openrc-run

name="dnsmasq-cli-rs ingest"
description="dnsmasq query log -> SQLite ingester"

command="$BIN"
command_args="ingest --log $DSQ_LOG --db $DB_DIR/logs.db --retention-days 30"
command_user="$SVC_USER"
command_background="yes"
pidfile="/run/dnsmasq-cli-rs.pid"
# 出力先は command_user が書き込める場所にする (/var/log は root 所有のため不可)
output_log="$DB_DIR/ingest.log"
error_log="$DB_DIR/ingest.log"

depend() {
    need dnsmasq
    after dnsmasq
}
CONF
chmod 0755 /etc/init.d/dnsmasq-cli-rs
rc-update add dnsmasq-cli-rs default >/dev/null 2>&1 || true

# ---- 5. adblock ジェネレータを whitelist 対応に ---------------------------
marker='dnsmasq-cli-rs/whitelist.txt'
if [ -f "$GEN" ] && ! grep -qF "$marker" "$GEN"; then
    say "$GEN を whitelist 対応に書き換えます (backup: $GEN.bak)"
    [ -f "$GEN.bak" ] || cp "$GEN" "$GEN.bak"
    cat > "$GEN" <<'GENEOF'
#!/bin/sh
# Weekly update of StevenBlack ad-block list for dnsmasq.
# /etc/dnsmasq-cli-rs/whitelist.txt に載っているドメインは除外する。
RAW="/tmp/adblock.raw"
OUT="/etc/dnsmasq.d/adblock.conf"
WHITELIST="/etc/dnsmasq-cli-rs/whitelist.txt"

if ! curl -fsSL -o "$RAW" https://raw.githubusercontent.com/StevenBlack/hosts/master/hosts; then
    logger -t adblock "failed to fetch StevenBlack hosts"
    exit 1
fi

[ -f "$WHITELIST" ] || : > "$WHITELIST"

# 1 ファイル目 (whitelist) を skip 集合に、2 ファイル目 (hosts) を本体として処理
awk '
    FNR == NR { if ($0 != "") skip[$0] = 1; next }
    $2 != "0.0.0.0" && /^0.0.0.0[ \t]/ && !($2 in skip) {
        print "address=/"$2"/0.0.0.0"
        print "address=/"$2"/::"
    }
' "$WHITELIST" "$RAW" > "$RAW.new"

if [ ! -s "$RAW.new" ]; then
    logger -t adblock "generated empty list, aborting"
    rm -f "$RAW" "$RAW.new"
    exit 1
fi

mv "$RAW.new" "$OUT"
rm -f "$RAW"

if /usr/sbin/dnsmasq --test --conf-file=/etc/dnsmasq.conf 2>/dev/null; then
    rc-service dnsmasq restart
    logger -t adblock "adblock list updated and reloaded"
else
    logger -t adblock "syntax check failed, config NOT reloaded"
    exit 1
fi
GENEOF
    chmod 0755 "$GEN"
fi

# ---- 6. dnsmasq を再起動してログを出させる --------------------------------
say "dnsmasq を検証して再起動します"
/usr/sbin/dnsmasq --test --conf-file=/etc/dnsmasq.conf
rc-service dnsmasq restart
sleep 1
[ -f "$DSQ_LOG" ] && chmod 0644 "$DSQ_LOG" || true

# ---- 7. 取り込みサービスを起動 --------------------------------------------
say "取り込みサービスを起動します"
rc-service dnsmasq-cli-rs restart
sleep 1
rc-status default | grep dnsmasq-cli-rs || true

say "完了。TUI は次で起動できます:  dnsmasq-cli-rs"
say "  (VPS に ssh して実行。キー: Tab=切替 /=検索 b=ブロックのみ w=whitelist R=リロード q=終了)"
