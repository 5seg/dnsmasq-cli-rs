//! データモデル: クエリイベントと outcome 分類。

/// クエリの最終的な扱い (dnsmasq のログ種別に対応)。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// adblock の `address=/.../0.0.0.0` 等でブロックされた。
    Blocked,
    /// 上流へ転送された (応答行が来る前の中間状態)。
    Forwarded,
    /// dnsmasq のキャッシュから返された。
    Cached,
    /// dnsmasq の期限切れキャッシュ (serve-stale / use-stale-cache) から返された。
    CachedStale,
    /// 上流からの応答 (reply)。
    Reply,
    /// /etc/hosts から返された。
    Hosts,
    /// その他 (config で実 IP、nameserver など)。種別名を保持。
    Other(String),
    /// 応答行が得られなかった。
    Unknown,
}

impl Outcome {
    pub fn as_str(&self) -> &str {
        match self {
            Outcome::Blocked => "blocked",
            Outcome::Forwarded => "forwarded",
            Outcome::Cached => "cached",
            Outcome::CachedStale => "cached-stale",
            Outcome::Reply => "reply",
            Outcome::Hosts => "hosts",
            Outcome::Unknown => "unknown",
            Outcome::Other(s) => s,
        }
    }

    pub fn is_blocked(&self) -> bool {
        matches!(self, Outcome::Blocked)
    }
}

/// 1 件のクエリ (query 行 + 対応する outcome 行を相関させたもの)。
#[derive(Clone, Debug)]
pub struct QueryEvent {
    /// 取り込み時刻 (unix millis)。dnsmasq の専用ログには時刻が無いため受信時刻を使う。
    pub ts: i64,
    /// dnsmasq の PID (ログ接頭辞 `dnsmasq[PID]:` から)。
    pub pid: Option<i64>,
    /// `log-queries=extra` のシーケンス番号。再起動で 1 に戻る。
    pub seq: Option<i64>,
    /// クライアント IP。
    pub client: String,
    pub domain: String,
    pub qtype: Option<String>,
    pub outcome: Outcome,
    /// 応答 IP / 転送先サーバ / NXDOMAIN などの本文。
    pub answer: Option<String>,
    pub raw: String,
}

impl QueryEvent {
    pub fn new(ts: i64, domain: String) -> Self {
        QueryEvent {
            ts,
            pid: None,
            seq: None,
            client: String::new(),
            domain,
            qtype: None,
            outcome: Outcome::Unknown,
            answer: None,
            raw: String::new(),
        }
    }
}
