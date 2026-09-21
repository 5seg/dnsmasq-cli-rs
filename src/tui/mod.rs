//! TUI: 状態、イベントループ、キー処理。

mod manage;
mod ui;

use std::collections::VecDeque;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::DefaultTerminal;

use crate::cli::Config;
use crate::store::{now_millis, DbRow, DayRow, Store, Summary};

enum BgResult {
    Summary {
        period: Period,
        result: Result<Summary>,
    },
    Daily(Result<Vec<DayRow>>),
}

/// メモリ上に保持する最大行数。
const MAX_BUFFER: usize = 20_000;
/// 新規行を取り込む間隔。
const POLL_INTERVAL: Duration = Duration::from_millis(500);
/// 統計を再集計する間隔。
const STATS_INTERVAL: Duration = Duration::from_secs(10);
/// Daily タブで表示する日数。
const DAILY_DAYS: i64 = 7;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Live,
    Stats,
    Daily,
}

impl Tab {
    fn next(self) -> Tab {
        match self {
            Tab::Live => Tab::Stats,
            Tab::Stats => Tab::Daily,
            Tab::Daily => Tab::Live,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Period {
    H1,
    H24,
    D7,
}

impl Period {
    pub fn ms(self) -> i64 {
        match self {
            Period::H1 => 3_600_000,
            Period::H24 => 86_400_000,
            Period::D7 => 604_800_000,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Period::H1 => "1h",
            Period::H24 => "24h",
            Period::D7 => "7d",
        }
    }
    fn cycle(self) -> Period {
        match self {
            Period::H1 => Period::H24,
            Period::H24 => Period::D7,
            Period::D7 => Period::H1,
        }
    }
}

/// ブロック行の表示フィルタ (`b` キーで巡回)。
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum BlockFilter {
    /// 全件表示。
    Default,
    /// ブロック行のみ。
    Only,
    /// ブロック行を除外。
    Exclude,
}

impl BlockFilter {
    fn next(self) -> Self {
        match self {
            BlockFilter::Default => BlockFilter::Only,
            BlockFilter::Only => BlockFilter::Exclude,
            BlockFilter::Exclude => BlockFilter::Default,
        }
    }

    fn label(self) -> &'static str {
        match self {
            BlockFilter::Default => "blocked: all",
            BlockFilter::Only => "blocked-only",
            BlockFilter::Exclude => "blocked excluded",
        }
    }

    fn allows(self, blocked: bool) -> bool {
        match self {
            BlockFilter::Default => true,
            BlockFilter::Only => blocked,
            BlockFilter::Exclude => !blocked,
        }
    }
}

pub enum InputMode {
    Search,
    Client,
}

pub enum Action {
    Whitelist(String),
    Reload,
}

pub struct App {
    pub store: Store,
    pub cfg: Config,

    pub events: VecDeque<DbRow>,
    last_id: i64,
    last_poll: Instant,
    last_stats: Instant,

    pub tab: Tab,
    pub period: Period,
    pub summary: Summary,
    /// `summary` が対応する期間。現在の `period` と一致しない間は未確定。
    pub summary_period: Option<Period>,
    pub daily: Vec<DayRow>,

    pub summary_loading: bool,
    pub daily_loading: bool,
    /// バックグラウンド集計 (summary/daily) の直近エラー。
    pub stats_error: Option<String>,
    /// 行ポーリングの直近エラー。
    pub poll_error: Option<String>,
    bg_tx: Sender<BgResult>,
    bg_rx: Receiver<BgResult>,

    pub search: String,
    pub client_filter: String,
    pub block_filter: BlockFilter,
    pub paused: bool,

    /// フィルタ後の並びにおける先頭表示位置。
    pub scroll: usize,
    /// フィルタ後の並びにおける選択位置。
    pub cursor: usize,
    /// 直近の描画で判明した表示行数 (カーソル可視化用)。
    pub view_height: usize,
    pub follow: bool,

    pub input: Option<InputMode>,
    pub confirm: Option<Action>,
    pub status: String,
    pub should_quit: bool,
}

impl App {
    pub fn new(store: Store, cfg: Config) -> Self {
        let (bg_tx, bg_rx) = channel();
        App {
            store,
            cfg,
            events: VecDeque::new(),
            last_id: 0,
            last_poll: Instant::now(),
            last_stats: Instant::now(),
            tab: Tab::Live,
            period: Period::H24,
            summary: Summary::default(),
            summary_period: None,
            daily: Vec::new(),
            summary_loading: false,
            daily_loading: false,
            stats_error: None,
            poll_error: None,
            bg_tx,
            bg_rx,
            search: String::new(),
            client_filter: String::new(),
            block_filter: BlockFilter::Default,
            paused: false,
            scroll: 0,
            cursor: 0,
            view_height: 1,
            follow: true,
            input: None,
            confirm: None,
            status: String::new(),
            should_quit: false,
        }
    }

    pub fn bootstrap(&mut self) -> Result<()> {
        // 起動時に直近 MAX_BUFFER 行をメモリへ載せる。以降 poll_rows() が新規行を追記する
        // (新規行だけでは過去分は埋まらないため、ここで履歴を確保する)。
        let rows = self.store.rows_recent(MAX_BUFFER as i64)?;
        self.last_id = rows.last().map(|r| r.id).unwrap_or(0);
        self.events.extend(rows);
        // 起動時にバックグラウンドで集計を先行開始 (プリフェッチ)
        self.trigger_refresh_summary();
        self.trigger_refresh_daily();
        self.jump_to_end();
        Ok(())
    }

    fn poll_rows(&mut self) -> Result<()> {
        if self.paused {
            return Ok(());
        }
        let rows = self.store.rows_after(self.last_id, 2000)?;
        if rows.is_empty() {
            return Ok(());
        }
        if let Some(last) = rows.last() {
            self.last_id = last.id;
        }
        for r in rows {
            self.events.push_back(r);
        }
        while self.events.len() > MAX_BUFFER {
            self.events.pop_front();
        }
        if self.follow {
            self.jump_to_end();
        }
        Ok(())
    }

    pub fn trigger_refresh_summary(&mut self) {
        if self.summary_loading {
            return;
        }
        self.summary_loading = true;
        let tx = self.bg_tx.clone();
        let db_path = self.cfg.db.clone();
        let period = self.period;
        let from = now_millis() - period.ms();
        let top = self.cfg.top;
        std::thread::spawn(move || {
            let res = Store::open_ro(&db_path).and_then(|s| s.summary(from, top));
            let _ = tx.send(BgResult::Summary {
                period,
                result: res,
            });
        });
    }

    pub fn trigger_refresh_daily(&mut self) {
        if self.daily_loading {
            return;
        }
        self.daily_loading = true;
        let tx = self.bg_tx.clone();
        let db_path = self.cfg.db.clone();
        let off = crate::util::tz_offset_ms();
        let today = (now_millis() + off) / 86_400_000;
        let start = today - (DAILY_DAYS - 1);
        std::thread::spawn(move || {
            let res = Store::open_ro(&db_path).and_then(|s| s.daily(start, DAILY_DAYS));
            let _ = tx.send(BgResult::Daily(res));
        });
    }

    pub fn tick(&mut self) {
        while let Ok(msg) = self.bg_rx.try_recv() {
            match msg {
                BgResult::Summary { period, result } => {
                    self.summary_loading = false;
                    match result {
                        Ok(s) if period == self.period => {
                            self.summary = s;
                            self.summary_period = Some(period);
                            self.stats_error = None;
                        }
                        // 取得中に期間が変わった。現在の期間で取り直す。
                        Ok(_) => self.trigger_refresh_summary(),
                        Err(e) => self.stats_error = Some(format!("summary error: {e}")),
                    }
                }
                BgResult::Daily(result) => {
                    self.daily_loading = false;
                    match result {
                        Ok(rows) => {
                            let off = crate::util::tz_offset_ms();
                            let today = (now_millis() + off) / 86_400_000;
                            let start = today - (DAILY_DAYS - 1);
                            self.daily = (0..DAILY_DAYS)
                                .map(|i| {
                                    let ms = (start + i) * 86_400_000 - off;
                                    rows.iter()
                                        .find(|r| r.day_ms == ms)
                                        .cloned()
                                        .unwrap_or_else(|| DayRow::zeros(ms))
                                })
                                .collect();
                            self.stats_error = None;
                        }
                        Err(e) => self.stats_error = Some(format!("daily error: {e}")),
                    }
                }
            }
        }

        if self.last_poll.elapsed() >= POLL_INTERVAL {
            self.last_poll = Instant::now();
            if let Err(e) = self.poll_rows() {
                self.poll_error = Some(format!("db error: {e}"));
            } else {
                self.poll_error = None;
            }
        }
        if self.last_stats.elapsed() >= STATS_INTERVAL {
            // 表示中のタブだけを再集計する。Live では間隔を進めないので、
            // タブを戻した時に即座に更新される。
            match self.tab {
                Tab::Live => {}
                Tab::Stats => {
                    self.last_stats = Instant::now();
                    self.trigger_refresh_summary();
                }
                Tab::Daily => {
                    self.last_stats = Instant::now();
                    self.trigger_refresh_daily();
                }
            }
        }
    }

    // ---- フィルタ/並び ----

    pub fn visible_indices(&self) -> Vec<usize> {
        let q = self.search.to_lowercase();
        let c = self.client_filter.to_lowercase();
        self.events
            .iter()
            .enumerate()
            .filter(|(_, r)| {
                self.block_filter.allows(r.ev.outcome.is_blocked())
                    && (q.is_empty() || r.ev.domain.to_lowercase().contains(&q))
                    && (c.is_empty() || r.ev.client.to_lowercase().contains(&c))
            })
            .map(|(i, _)| i)
            .collect()
    }

    fn jump_to_end(&mut self) {
        self.follow = true;
    }

    /// フィルタ後の件数を返す (描画補助)。
    pub fn filtered_len(&self) -> usize {
        self.visible_indices().len()
    }

    /// 選択中の行 (フィルタ後の並びで cursor)。
    pub fn selected(&self) -> Option<&DbRow> {
        let idx = self.visible_indices();
        idx.get(self.cursor).and_then(|i| self.events.get(*i))
    }

    /// cursor/scroll を可視範囲に収める。
    pub fn clamp_view(&mut self, len: usize) {
        if len == 0 {
            self.scroll = 0;
            self.cursor = 0;
            return;
        }
        if self.cursor >= len {
            self.cursor = len - 1;
        }
        let h = self.view_height.max(1);
        if self.cursor < self.scroll {
            self.scroll = self.cursor;
        }
        if self.cursor >= self.scroll + h {
            self.scroll = self.cursor + 1 - h;
        }
        let max_scroll = len.saturating_sub(h);
        if self.scroll > max_scroll {
            self.scroll = max_scroll;
        }
    }

    // ---- キー処理 ----

    pub fn handle_key(&mut self, key: KeyEvent) {
        if key.kind == KeyEventKind::Release {
            return;
        }
        // 確認ダイアログ
        if self.confirm.is_some() {
            match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => {
                    let act = self.confirm.take().unwrap();
                    let msg = manage::execute(&act);
                    self.status = msg;
                    // 管理操作後はバックグラウンドで統計を取り直す
                    self.trigger_refresh_summary();
                    self.trigger_refresh_daily();
                }
                _ => {
                    self.confirm = None;
                    self.status = "cancelled".to_string();
                }
            }
            return;
        }
        // 入力モード
        if let Some(mode) = &self.input {
            match key.code {
                KeyCode::Esc => {
                    self.input = None;
                }
                KeyCode::Enter => {
                    let target = match mode {
                        InputMode::Search => self.search.clone(),
                        InputMode::Client => self.client_filter.clone(),
                    };
                    self.status = format!("filter: {target:?}");
                    self.input = None;
                    self.jump_to_end();
                }
                KeyCode::Backspace => {
                    match mode {
                        InputMode::Search => {
                            self.search.pop();
                        }
                        InputMode::Client => {
                            self.client_filter.pop();
                        }
                    }
                }
                KeyCode::Char(c) => match mode {
                    InputMode::Search => self.search.push(c),
                    InputMode::Client => self.client_filter.push(c),
                },
                _ => {}
            }
            return;
        }

        match key.code {
            KeyCode::Char('q') => self.should_quit = true,
            KeyCode::Esc => {
                // フィルタをリセット
                self.search.clear();
                self.client_filter.clear();
                self.block_filter = BlockFilter::Default;
            }
            KeyCode::Tab => {
                self.tab = self.tab.next();
                // 表示対象のデータが未取得、または現在の期間と不一致なら即座に非同期取得
                match self.tab {
                    Tab::Live => {}
                    Tab::Stats => {
                        if self.summary_period != Some(self.period) {
                            self.trigger_refresh_summary();
                        }
                    }
                    Tab::Daily => {
                        if self.daily.is_empty() {
                            self.trigger_refresh_daily();
                        }
                    }
                }
            }
            KeyCode::Char('1') => {
                self.period = Period::H1;
                if self.tab == Tab::Stats {
                    self.trigger_refresh_summary();
                }
            }
            KeyCode::Char('2') => {
                self.period = Period::H24;
                if self.tab == Tab::Stats {
                    self.trigger_refresh_summary();
                }
            }
            KeyCode::Char('3') => {
                self.period = Period::D7;
                if self.tab == Tab::Stats {
                    self.trigger_refresh_summary();
                }
            }
            KeyCode::Char('b') => {
                self.block_filter = self.block_filter.next();
                self.status = self.block_filter.label().to_string();
                self.jump_to_end();
            }
            KeyCode::Char('p') => {
                self.paused = !self.paused;
                self.status = format!("paused: {}", self.paused);
            }
            KeyCode::Char('/') => {
                self.input = Some(InputMode::Search);
                self.status = "search domain:".to_string();
            }
            KeyCode::Char('c') => {
                self.input = Some(InputMode::Client);
                self.status = "filter client:".to_string();
            }
            KeyCode::Char('T') => {
                self.period = self.period.cycle();
                if self.tab == Tab::Stats {
                    self.trigger_refresh_summary();
                }
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.follow = false;
                self.cursor = self.cursor.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.follow = false;
                self.cursor = self.cursor.saturating_add(1);
            }
            KeyCode::PageUp => {
                self.follow = false;
                self.cursor = self.cursor.saturating_sub(self.view_height.max(1));
            }
            KeyCode::PageDown => {
                self.follow = false;
                self.cursor = self.cursor.saturating_add(self.view_height.max(1));
            }
            KeyCode::Home | KeyCode::Char('g') => {
                self.follow = false;
                self.cursor = 0;
            }
            KeyCode::End | KeyCode::Char('G') => {
                let len = self.filtered_len();
                self.cursor = len.saturating_sub(1);
                self.follow = true;
            }
            KeyCode::Char('w') => {
                if let Some(row) = self.selected() {
                    let d = row.ev.domain.clone();
                    self.confirm = Some(Action::Whitelist(d.clone()));
                    self.status = format!("whitelist {d}? (y/n)");
                } else {
                    self.status = "no row selected".to_string();
                }
            }
            KeyCode::Char('R') => {
                self.confirm = Some(Action::Reload);
                self.status = "reload dnsmasq? (y/n)".to_string();
            }
            _ => {}
        }
    }

    pub fn status_line(&self) -> String {
        let mut parts = vec![format!("{}/{}", self.cursor + 1, self.filtered_len())];
        if !self.search.is_empty() {
            parts.push(format!("search:{:?}", self.search));
        }
        if !self.client_filter.is_empty() {
            parts.push(format!("client:{:?}", self.client_filter));
        }
        if self.block_filter != BlockFilter::Default {
            parts.push(self.block_filter.label().to_string());
        }
        if self.paused {
            parts.push("PAUSED".to_string());
        }
        if !self.status.is_empty() {
            parts.push(self.status.clone());
        }
        parts.join("  ")
    }

    /// バックグラウンドの直近エラー (ポーリング/集計)。ユーザー向け status とは別枠で表示する。
    pub fn error_line(&self) -> Option<String> {
        let mut parts = Vec::new();
        if let Some(e) = &self.poll_error {
            parts.push(e.clone());
        }
        if let Some(e) = &self.stats_error {
            parts.push(e.clone());
        }
        if parts.is_empty() {
            None
        } else {
            Some(parts.join("  "))
        }
    }
}

/// TUI を起動する。
pub fn run(cfg: Config) -> Result<()> {
    let store = Store::open_ro(&cfg.db)?;
    let mut app = App::new(store, cfg);
    app.bootstrap()?;

    let mut terminal = ratatui::init();
    let result = event_loop(&mut terminal, &mut app);
    ratatui::restore();
    result
}

fn event_loop(terminal: &mut DefaultTerminal, app: &mut App) -> Result<()> {
    loop {
        terminal.draw(|f| ui::render(f, app))?;
        if event::poll(Duration::from_millis(200))? {
            if let Event::Key(k) = event::read()? {
                if k.modifiers.contains(KeyModifiers::CONTROL) && k.code == KeyCode::Char('c') {
                    app.should_quit = true;
                } else {
                    app.handle_key(k);
                }
            }
        }
        app.tick();
        if app.should_quit {
            break;
        }
    }
    Ok(())
}
