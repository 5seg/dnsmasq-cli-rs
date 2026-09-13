//! SQLite ストア: スキーマ、バッチ挿入、tail 状態の永続化、集計。

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OpenFlags, OptionalExtension};
use std::path::Path;

use crate::model::{Outcome, QueryEvent};

const SCHEMA_VERSION: &str = "1";

pub struct Store {
    conn: Connection,
}

/// TUI が表示する 1 行 (DB の id 付き)。
#[derive(Clone, Debug)]
pub struct DbRow {
    pub id: i64,
    pub ev: QueryEvent,
}

/// 集計結果 (期間内)。
#[derive(Clone, Debug, Default)]
pub struct Summary {
    pub total: i64,
    pub blocked: i64,
    pub domains: Vec<CountRow>,
    pub clients: Vec<CountRow>,
    pub qtypes: Vec<CountRow>,
}

/// 集計の 1 行 (key と件数、任意でブロック数)。
#[derive(Clone, Debug)]
pub struct CountRow {
    pub key: String,
    pub count: i64,
    pub blocked: i64,
}

impl Store {
    /// 書き込み用に開く (ingest 用)。親ディレクトリが無ければ作る。
    pub fn open_rw(path: &str) -> Result<Self> {
        if let Some(parent) = Path::new(path).parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("mkdir -p {}", parent.display()))?;
            }
        }
        let conn = Connection::open(path).with_context(|| format!("open {path}"))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        let s = Store { conn };
        s.migrate()?;
        Ok(s)
    }

    /// 読み取り専用で開く (TUI 用)。失敗したら書き込み可 + query_only で再試行。
    pub fn open_ro(path: &str) -> Result<Self> {
        match Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY) {
            Ok(conn) => {
                conn.busy_timeout(std::time::Duration::from_secs(5))?;
                conn.pragma_update(None, "query_only", true)?;
                Ok(Store { conn })
            }
            Err(_) => {
                let conn = Connection::open(path)?;
                conn.busy_timeout(std::time::Duration::from_secs(5))?;
                conn.pragma_update(None, "query_only", true)?;
                Ok(Store { conn })
            }
        }
    }

    pub fn migrate(&self) -> Result<()> {
        self.conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS queries (
                id      INTEGER PRIMARY KEY AUTOINCREMENT,
                ts      INTEGER NOT NULL,
                pid     INTEGER,
                seq     INTEGER,
                client  TEXT    NOT NULL,
                domain  TEXT    NOT NULL,
                qtype   TEXT,
                outcome TEXT    NOT NULL,
                answer  TEXT,
                raw     TEXT
            );
            CREATE INDEX IF NOT EXISTS idx_queries_ts      ON queries(ts);
            CREATE INDEX IF NOT EXISTS idx_queries_domain  ON queries(domain);
            CREATE INDEX IF NOT EXISTS idx_queries_client  ON queries(client);
            CREATE INDEX IF NOT EXISTS idx_queries_outcome ON queries(outcome);
            CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT);
            "#,
        )?;
        self.meta_set("schema_version", SCHEMA_VERSION)?;
        Ok(())
    }

    // ---- meta ----

    pub fn meta_get(&self, key: &str) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row("SELECT value FROM meta WHERE key = ?1", params![key], |r| {
                r.get::<_, String>(0)
            })
            .optional()?)
    }

    pub fn meta_set(&self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO meta(key, value) VALUES(?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    // ---- ingest ----

    /// イベント群の挿入と tail 状態の更新を 1 トランザクションで行う。
    /// これによりクラッシュ時も「重複なし・欠落なし」を保証する。
    pub fn commit_batch(
        &mut self,
        evs: &[QueryEvent],
        tail_inode: Option<u64>,
        tail_offset: Option<u64>,
    ) -> Result<usize> {
        let tx = self.conn.transaction()?;
        {
            let mut stmt = tx.prepare_cached(
                "INSERT INTO queries(ts, pid, seq, client, domain, qtype, outcome, answer, raw)
                 VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            )?;
            for ev in evs {
                stmt.execute(params![
                    ev.ts,
                    ev.pid,
                    ev.seq,
                    ev.client,
                    ev.domain,
                    ev.qtype,
                    ev.outcome.as_str(),
                    ev.answer,
                    ev.raw,
                ])?;
            }
        }
        if let Some(ino) = tail_inode {
            tx.execute(
                "INSERT INTO meta(key, value) VALUES('tail_inode', ?1)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![ino.to_string()],
            )?;
        }
        if let Some(off) = tail_offset {
            tx.execute(
                "INSERT INTO meta(key, value) VALUES('tail_offset', ?1)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![off.to_string()],
            )?;
        }
        tx.commit()?;
        Ok(evs.len())
    }

    /// 保持期間を超えた行を削除する。削除件数を返す。
    pub fn prune(&mut self, older_than_ms: i64, vacuum: bool) -> Result<usize> {
        let cutoff = now_millis() - older_than_ms;
        let n = self
            .conn
            .execute("DELETE FROM queries WHERE ts < ?1", params![cutoff])?;
        if vacuum {
            self.conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
            self.conn.execute_batch("VACUUM;")?;
        }
        Ok(n)
    }

    // ---- query (TUI / info) ----

    pub fn count(&self) -> Result<i64> {
        Ok(self
            .conn
            .query_row("SELECT COUNT(*) FROM queries", [], |r| r.get(0))?)
    }

    pub fn time_range(&self) -> Result<Option<(i64, i64)>> {
        Ok(self
            .conn
            .query_row("SELECT MIN(ts), MAX(ts) FROM queries", [], |r| {
                Ok((r.get::<_, Option<i64>>(0)?, r.get::<_, Option<i64>>(1)?))
            })
            .optional()?
            .and_then(|(a, b)| match (a, b) {
                (Some(a), Some(b)) => Some((a, b)),
                _ => None,
            }))
    }

    pub fn outcome_breakdown(&self) -> Result<Vec<(String, i64)>> {
        let mut stmt = self
            .conn
            .prepare("SELECT outcome, COUNT(*) FROM queries GROUP BY outcome ORDER BY 2 DESC")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    const COLS: &'static str =
        "id, ts, pid, seq, client, domain, qtype, outcome, answer, raw";

    /// `id > after` の行を昇順で最大 `limit` 件取得する。
    pub fn rows_after(&self, after: i64, limit: i64) -> Result<Vec<DbRow>> {
        let sql = format!(
            "SELECT {} FROM queries WHERE id > ?1 ORDER BY id ASC LIMIT ?2",
            Self::COLS
        );
        let mut stmt = self.conn.prepare_cached(&sql)?;
        let rows = stmt.query_map(params![after, limit], map_row)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// 最新 `limit` 件を古い順で取得する (起動時の初期ロード用)。
    pub fn rows_recent(&self, limit: i64) -> Result<Vec<DbRow>> {
        let sql = format!(
            "SELECT {} FROM (SELECT * FROM queries ORDER BY id DESC LIMIT ?1) ORDER BY id ASC",
            Self::COLS
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params![limit], map_row)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// `ts < cutoff` の行数 (prune の dry-run 用)。
    pub fn count_older(&self, cutoff_ms: i64) -> Result<i64> {
        Ok(self.conn.query_row(
            "SELECT COUNT(*) FROM queries WHERE ts < ?1",
            params![cutoff_ms],
            |r| r.get(0),
        )?)
    }

    /// 期間内 (ts >= from) の集計をまとめて返す。
    pub fn summary(&self, from_ms: i64, top: i64) -> Result<Summary> {
        let total: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM queries WHERE ts >= ?1",
            params![from_ms],
            |r| r.get(0),
        )?;
        let blocked: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM queries WHERE ts >= ?1 AND outcome = 'blocked'",
            params![from_ms],
            |r| r.get(0),
        )?;

        let domains = self.top(
            "SELECT domain, COUNT(*) c, SUM(outcome = 'blocked') b
             FROM queries WHERE ts >= ?1 GROUP BY domain ORDER BY c DESC LIMIT ?2",
            from_ms,
            top,
        )?;
        let clients = self.top(
            "SELECT client, COUNT(*) c, SUM(outcome = 'blocked') b
             FROM queries WHERE ts >= ?1 GROUP BY client ORDER BY c DESC LIMIT ?2",
            from_ms,
            top,
        )?;
        let qtypes = self.top(
            "SELECT COALESCE(qtype, '?'), COUNT(*) c, SUM(outcome = 'blocked') b
             FROM queries WHERE ts >= ?1 GROUP BY 1 ORDER BY c DESC LIMIT ?2",
            from_ms,
            top,
        )?;

        Ok(Summary {
            total,
            blocked,
            domains,
            clients,
            qtypes,
        })
    }

    fn top(&self, sql: &str, from_ms: i64, limit: i64) -> Result<Vec<CountRow>> {
        let mut stmt = self.conn.prepare(sql)?;
        let rows = stmt.query_map(params![from_ms, limit], |r| {
            Ok(CountRow {
                key: r.get(0)?,
                count: r.get(1)?,
                blocked: r.get::<_, Option<i64>>(2)?.unwrap_or(0),
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }
}

fn map_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<DbRow> {
    Ok(DbRow {
        id: r.get(0)?,
        ev: QueryEvent {
            ts: r.get(1)?,
            pid: r.get(2)?,
            seq: r.get(3)?,
            client: r.get(4)?,
            domain: r.get(5)?,
            qtype: r.get(6)?,
            outcome: Outcome::from_db(&r.get::<_, String>(7)?),
            answer: r.get(8)?,
            raw: r.get(9)?,
        },
    })
}

impl Outcome {
    fn from_db(s: &str) -> Outcome {
        match s {
            "blocked" => Outcome::Blocked,
            "forwarded" => Outcome::Forwarded,
            "cached" => Outcome::Cached,
            "reply" => Outcome::Reply,
            "hosts" => Outcome::Hosts,
            "unknown" => Outcome::Unknown,
            other => Outcome::Other(other.to_string()),
        }
    }
}

pub fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
