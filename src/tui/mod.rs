//! TUI: 状態、イベントループ、キー処理。

mod manage;
mod ui;

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::DefaultTerminal;

use crate::cli::Config;
use crate::store::{now_millis, DbRow, Store, Summary};

/// メモリ上に保持する最大行数。
const MAX_BUFFER: usize = 20_000;
/// 新規行を取り込む間隔。
const POLL_INTERVAL: Duration = Duration::from_millis(500);
/// 統計を再集計する間隔。
const STATS_INTERVAL: Duration = Duration::from_secs(3);

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Live,
    Stats,
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
        App {
            store,
            cfg,
            events: VecDeque::new(),
            last_id: 0,
            last_poll: Instant::now(),
            last_stats: Instant::now() - STATS_INTERVAL,
            tab: Tab::Live,
            period: Period::H24,
            summary: Summary::default(),
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
        let rows = self.store.rows_recent(MAX_BUFFER as i64)?;
        self.last_id = rows.last().map(|r| r.id).unwrap_or(0);
        self.events.extend(rows);
        self.refresh_stats()?;
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

    fn refresh_stats(&mut self) -> Result<()> {
        let from = now_millis() - self.period.ms();
        self.summary = self.store.summary(from, self.cfg.top)?;
        self.last_stats = Instant::now();
        Ok(())
    }

    pub fn tick(&mut self) {
        if self.last_poll.elapsed() >= POLL_INTERVAL {
            self.last_poll = Instant::now();
            if let Err(e) = self.poll_rows() {
                self.status = format!("db error: {e}");
            }
        }
        if self.last_stats.elapsed() >= STATS_INTERVAL {
            if let Err(e) = self.refresh_stats() {
                self.status = format!("stats error: {e}");
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
                    // 管理操作後は統計を取り直す
                    let _ = self.refresh_stats();
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
                self.tab = match self.tab {
                    Tab::Live => Tab::Stats,
                    Tab::Stats => Tab::Live,
                }
            }
            KeyCode::Char('1') => {
                self.period = Period::H1;
                let _ = self.refresh_stats();
            }
            KeyCode::Char('2') => {
                self.period = Period::H24;
                let _ = self.refresh_stats();
            }
            KeyCode::Char('3') => {
                self.period = Period::D7;
                let _ = self.refresh_stats();
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
                let _ = self.refresh_stats();
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
