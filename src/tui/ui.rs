//! ratatui による描画。

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{
    Block, BorderType, Borders, Cell, Clear, Gauge, Padding, Paragraph, Row, Table, TableState,
    Tabs,
};
use ratatui::Frame;

use crate::model::Outcome;
use crate::store::DbRow;
use crate::util::{fmt_date, fmt_time};

use super::{App, Tab};

pub fn render(f: &mut Frame, app: &mut App) {
    let chunks = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(3),
        Constraint::Length(1),
    ])
    .split(f.area());

    // タブ
    let titles = vec![
        format!(" Live ({}) ", app.filtered_len()),
        format!(" Stats [{}] ", app.period.label()),
        " Daily ".to_string(),
    ];
    let sel = match app.tab {
        Tab::Live => 0,
        Tab::Stats => 1,
        Tab::Daily => 2,
    };
    let tabs = Tabs::new(titles)
        .select(sel)
        .highlight_style(
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
        .divider("|");
    f.render_widget(tabs, chunks[0]);

    match app.tab {
        Tab::Live => render_live(f, app, chunks[1]),
        Tab::Stats => render_stats(f, app, chunks[1]),
        Tab::Daily => render_daily(f, app, chunks[1]),
    }

    // ステータス行
    let status = Paragraph::new(app.status_line()).style(Style::default().fg(Color::DarkGray));
    f.render_widget(status, chunks[2]);

    if app.confirm.is_some() {
        render_confirm(f, app);
    }
}

fn outcome_style(o: &Outcome) -> Style {
    let color = match o {
        Outcome::Blocked => Color::Red,
        Outcome::Forwarded => Color::Green,
        Outcome::Cached => Color::Yellow,
        Outcome::Reply => Color::Blue,
        Outcome::Hosts => Color::Magenta,
        Outcome::Unknown => Color::DarkGray,
        Outcome::Other(_) => Color::Cyan,
    };
    Style::default().fg(color)
}

fn render_live(f: &mut Frame, app: &mut App, area: Rect) {
    let idxs = app.visible_indices();
    let len = idxs.len();
    app.view_height = area.height.saturating_sub(1) as usize;
    app.clamp_view(len);

    let end = (app.scroll + app.view_height).min(len);
    let window: Vec<&DbRow> = idxs[app.scroll.min(len)..end]
        .iter()
        .filter_map(|i| app.events.get(*i))
        .collect();

    let rows: Vec<Row> = window
        .iter()
        .map(|r| {
            let style = outcome_style(&r.ev.outcome);
            Row::new(vec![
                Cell::from(fmt_time(r.ev.ts)),
                Cell::from(r.ev.client.clone()),
                Cell::from(r.ev.domain.clone()),
                Cell::from(r.ev.qtype.clone().unwrap_or_default()),
                Cell::from(r.ev.outcome.as_str().to_string()),
                Cell::from(r.ev.answer.clone().unwrap_or_default()),
            ])
            .style(style)
        })
        .collect();

    let header = Row::new(vec!["time", "client", "domain", "type", "result", "answer"])
        .style(Style::default().fg(Color::Gray).add_modifier(Modifier::BOLD));

    let title = format!(
        " queries  follow:{}  {} ",
        if app.follow { "on" } else { "off" },
        app.cfg.db
    );
    let table = Table::new(
        rows,
        [
            Constraint::Length(8),
            Constraint::Length(16),
            Constraint::Min(24),
            Constraint::Length(6),
            Constraint::Length(9),
            Constraint::Min(12),
        ],
    )
    .header(header)
    .block(
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .title(title),
    )
    .row_highlight_style(
        Style::default()
            .bg(Color::Indexed(236))
            .add_modifier(Modifier::BOLD),
    );

    let mut state = TableState::default();
    if len > 0 {
        state = state.with_selected(Some(app.cursor.saturating_sub(app.scroll)));
    }
    f.render_stateful_widget(table, area, &mut state);

    if len == 0 {
        let msg = Paragraph::new("(no queries yet)")
            .style(Style::default().fg(Color::DarkGray))
            .block(Block::default().borders(Borders::NONE));
        let inner = Rect {
            x: area.x + 2,
            y: area.y + 1,
            width: area.width.saturating_sub(4),
            height: 1,
        };
        f.render_widget(msg, inner);
    }
}

fn render_stats(f: &mut Frame, app: &App, area: Rect) {
    let s = &app.summary;
    let rate = if s.total > 0 {
        s.blocked as f64 / s.total as f64
    } else {
        0.0
    };

    let rows = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(6),
    ])
    .split(area);

    // サマリ + ブロック率ゲージ
    let summary_line = format!(
        "queries {}   blocked {}   block rate {:.1}%   period {}",
        s.total,
        s.blocked,
        rate * 100.0,
        app.period.label()
    );
    let gauge = Gauge::default()
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .title(format!(" summary ({} window) ", app.period.label())),
        )
        .gauge_style(Style::default().fg(Color::Red))
        .ratio(rate.clamp(0.0, 1.0))
        .label(Span::styled(
            summary_line,
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ));
    f.render_widget(gauge, rows[0]);

    // 3 段: domains 系 / clients・qtypes / outcomes (全幅)
    let body = Layout::vertical([
        Constraint::Percentage(38),
        Constraint::Percentage(32),
        Constraint::Percentage(30),
    ])
    .split(rows[1]);
    let row1 = Layout::horizontal([
        Constraint::Percentage(50),
        Constraint::Percentage(50),
    ])
    .split(body[0]);
    let row2 = Layout::horizontal([
        Constraint::Percentage(50),
        Constraint::Percentage(50),
    ])
    .split(body[1]);

    f.render_widget(
        counts_block(
            " Top domains ",
            &s.domains
                .iter()
                .map(|c| (c.key.clone(), c.count, c.blocked))
                .collect::<Vec<_>>(),
            (s.total - s.blocked).max(0),
            Color::Cyan,
            false,
        ),
        row1[0],
    );
    f.render_widget(
        counts_block(
            " Top blocked domains ",
            &s.blocked_domains
                .iter()
                .map(|c| (c.key.clone(), c.count, c.blocked))
                .collect::<Vec<_>>(),
            s.blocked,
            Color::Red,
            false,
        ),
        row1[1],
    );
    f.render_widget(
        counts_block(
            " Top clients ",
            &s.clients
                .iter()
                .map(|c| (c.key.clone(), c.count, c.blocked))
                .collect::<Vec<_>>(),
            s.total,
            Color::Green,
            true,
        ),
        row2[0],
    );
    f.render_widget(
        counts_block(
            " Query types ",
            &s.qtypes
                .iter()
                .map(|c| (c.key.clone(), c.count, c.blocked))
                .collect::<Vec<_>>(),
            s.total,
            Color::Magenta,
            true,
        ),
        row2[1],
    );
    f.render_widget(
        outcomes_stacked_block(
            " Outcomes (excl. blocked) ",
            &s.outcomes
                .iter()
                .map(|c| (c.key.clone(), c.count))
                .collect::<Vec<_>>(),
        ),
        body[2],
    );
}

/// 日別統計: 上にクエリ数 (cached/reply/forwarded)、下にブロック数を
/// 1 日 1 本の積み上げ棒で表示する。
fn render_daily(f: &mut Frame, app: &App, area: Rect) {
    let rows = Layout::vertical([
        Constraint::Percentage(55),
        Constraint::Percentage(45),
    ])
    .split(area);

    let color_q = |o: &str| match o {
        "cached" => Color::Yellow,
        "reply" => Color::Blue,
        _ => Color::Green, // forwarded
    };

    // 直近の日を左→右 (古い→新しい) に並べる。
    let labels: Vec<String> = app
        .daily
        .iter()
        .map(|d| fmt_date(d.day_ms).get(5..).unwrap_or("").to_string())
        .collect();

    // クエリ数のパネル (棒グラフ | 数値リスト)
    let q_title = Line::from(vec![
        Span::raw(" Query counts by day  "),
        Span::styled("■ cached", Style::default().fg(color_q("cached"))),
        Span::raw(" "),
        Span::styled("■ reply", Style::default().fg(color_q("reply"))),
        Span::raw(" "),
        Span::styled("■ forwarded", Style::default().fg(color_q("forwarded"))),
        Span::raw(" "),
    ]);
    let q_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .title(q_title);
    let q_inner = q_block.inner(rows[0]);
    f.render_widget(q_block, rows[0]);

    let q_items: Vec<Vec<(i64, Color)>> = app
        .daily
        .iter()
        .map(|d| {
            vec![
                (d.cached, color_q("cached")),
                (d.reply, color_q("reply")),
                (d.forwarded, color_q("forwarded")),
            ]
        })
        .collect();
    // 数値リストは最新が上。データの無い日は出さない。
    let q_lines: Vec<Line> = app
        .daily
        .iter()
        .rev()
        .filter(|d| d.cached + d.reply + d.forwarded > 0)
        .map(|d| {
            Line::from(vec![
                Span::styled(
                    fmt_date(d.day_ms).get(5..).unwrap_or("").to_string(),
                    Style::default().fg(Color::Gray),
                ),
                Span::raw("  "),
                Span::styled(
                    format!("cached:{:>5}", d.cached),
                    Style::default().fg(color_q("cached")),
                ),
                Span::raw("  "),
                Span::styled(
                    format!("reply:{:>5}", d.reply),
                    Style::default().fg(color_q("reply")),
                ),
                Span::raw("  "),
                Span::styled(
                    format!("forwarded:{:>5}", d.forwarded),
                    Style::default().fg(color_q("forwarded")),
                ),
            ])
        })
        .collect();
    let q_cols = Layout::horizontal([
        Constraint::Min(10),
        Constraint::Length(54.min(q_inner.width.saturating_sub(20))),
    ])
    .split(q_inner);
    render_stacked_bars(f, q_cols[0], &labels, &q_items);
    render_numbers(f, q_cols[1], q_lines);

    // ブロック数のパネル (棒グラフ | 数値リスト)
    let b_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .title(Line::from(Span::styled(
            " Blocked by day ",
            Style::default().fg(Color::Red),
        )));
    let b_inner = b_block.inner(rows[1]);
    f.render_widget(b_block, rows[1]);

    let b_items: Vec<Vec<(i64, Color)>> = app
        .daily
        .iter()
        .map(|d| vec![(d.blocked, Color::Red)])
        .collect();
    let b_lines: Vec<Line> = app
        .daily
        .iter()
        .rev()
        .filter(|d| d.blocked > 0)
        .map(|d| {
            Line::from(vec![
                Span::styled(
                    fmt_date(d.day_ms).get(5..).unwrap_or("").to_string(),
                    Style::default().fg(Color::Gray),
                ),
                Span::raw("  "),
                Span::styled(
                    format!("blocked:{:>5}", d.blocked),
                    Style::default().fg(Color::Red),
                ),
            ])
        })
        .collect();
    let b_cols = Layout::horizontal([
        Constraint::Min(10),
        Constraint::Length(26.min(b_inner.width.saturating_sub(20))),
    ])
    .split(b_inner);
    render_stacked_bars(f, b_cols[0], &labels, &b_items);
    render_numbers(f, b_cols[1], b_lines);
}

/// 数値リストを左罫線付きで描く。
fn render_numbers(f: &mut Frame, area: Rect, lines: Vec<Line<'static>>) {
    let p = Paragraph::new(Text::from(lines)).block(
        Block::default()
            .borders(Borders::LEFT)
            .border_type(BorderType::Rounded)
            .padding(Padding::horizontal(1)),
    );
    f.render_widget(p, area);
}

/// 日ごとに 1 本の積み上げ棒を描く。各棒はセグメント (値, 色) を下から積む。
fn render_stacked_bars(
    f: &mut Frame,
    area: Rect,
    labels: &[String],
    items: &[Vec<(i64, Color)>],
) {
    if items.is_empty() || area.width == 0 || area.height < 2 {
        return;
    }
    // 最下段はラベル行に使う。
    let chart_h = (area.height - 1) as usize;
    // 最上段は余白にして、最大の棒でも枠に接しないようにする。
    let scale_h = chart_h.saturating_sub(1).max(1);
    // データが無くてもスロット (余白) は確保する。
    let max_total = items
        .iter()
        .map(|segs| segs.iter().map(|(v, _)| *v).sum::<i64>())
        .max()
        .unwrap_or(0)
        .max(1);

    let n = items.len() as u16;
    let slot = area.width / n;
    if slot == 0 {
        return;
    }
    // 棒は太くなりすぎないよう上限を設け、スロット内で中央に置く。
    let bar_w = slot.saturating_sub(2).min(8).max(1);
    let buf = f.buffer_mut();

    for (i, segs) in items.iter().enumerate() {
        let x0 = area.x + i as u16 * slot + (slot - bar_w) / 2;

        // ラベルは値が 0 の日でも描く。
        if let Some(label) = labels.get(i) {
            let ly = area.y + area.height - 1;
            let text = truncate(label, slot as usize);
            let lx = area.x + i as u16 * slot + slot.saturating_sub(text.len() as u16) / 2;
            buf.set_string(lx, ly, &text, Style::default().fg(Color::DarkGray));
        }

        let total: i64 = segs.iter().map(|(v, _)| *v).sum();
        if total <= 0 {
            continue;
        }
        // 高さは最大の棒を基準に相対計算する (最上段は余白)。
        let cells = (((total as f64 / max_total as f64) * scale_h as f64).round() as usize)
            .clamp(1, scale_h);

        let mut filled = 0usize;
        let mut acc = 0f64;
        let mut y = area.y + chart_h as u16;
        for (si, (v, color)) in segs.iter().enumerate() {
            let seg = if si + 1 == segs.len() {
                cells - filled
            } else {
                acc += *v as f64 / total as f64 * cells as f64;
                (acc.round() as usize).saturating_sub(filled)
            };
            for _ in 0..seg {
                if y <= area.y {
                    break;
                }
                y -= 1;
                for dx in 0..bar_w {
                    buf.set_string(x0 + dx, y, "█", Style::default().fg(*color));
                }
            }
            filled = (filled + seg).min(cells);
        }
    }
}

fn outcome_color(name: &str) -> Color {
    match name {
        "blocked" => Color::Red,
        "forwarded" => Color::Green,
        "cached" => Color::Yellow,
        "reply" => Color::Blue,
        "hosts" => Color::Magenta,
        "unknown" => Color::DarkGray,
        _ => Color::Cyan,
    }
}

/// Outcomes を 1 本の積み上げバーで表示する (blocked は除外)。
fn outcomes_stacked_block(title: &str, items: &[(String, i64)]) -> Paragraph<'static> {
    const BAR_WIDTH: usize = 48;
    let items: Vec<(&str, i64)> = items
        .iter()
        .filter(|(k, _)| *k != "blocked")
        .map(|(k, c)| (k.as_str(), *c))
        .collect();
    let total: i64 = items.iter().map(|(_, c)| *c).sum();
    if items.is_empty() || total <= 0 {
        return Paragraph::new(Text::from(Line::from(Span::styled(
            "(no data)",
            Style::default().fg(Color::DarkGray),
        ))))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .title(title.to_string()),
        );
    }

    let mut spans: Vec<Span<'static>> = Vec::new();
    for (i, (name, count)) in items.iter().enumerate() {
        let w = (((*count as f64 / total as f64) * BAR_WIDTH as f64).round() as usize)
            .max(1)
            .min(BAR_WIDTH);
        if i > 0 {
            spans.push(Span::raw("|"));
        }
        spans.push(Span::styled(
            "█".repeat(w),
            Style::default().fg(outcome_color(name)),
        ));
    }
    let mut lines = vec![Line::from(spans)];

    for (name, count) in &items {
        let pct = *count as f64 / total as f64 * 100.0;
        lines.push(Line::from(vec![
            Span::styled("■ ".to_string(), Style::default().fg(outcome_color(name))),
            Span::styled(
                format!("{name:<12} {count:>5} {pct:>5.1}%"),
                Style::default().fg(Color::White),
            ),
        ]));
    }

    Paragraph::new(Text::from(lines)).block(
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .title(title.to_string()),
    )
}

fn counts_block(
    title: &str,
    items: &[(String, i64, i64)],
    total: i64,
    color: Color,
    show_blocked: bool,
) -> Paragraph<'static> {
    let max = items.iter().map(|(_, c, _)| *c).max().unwrap_or(1).max(1);
    let mut lines: Vec<Line> = Vec::new();
    for (key, count, blocked) in items {
        let bar = bar(*count, max, 12);
        let pct = if total > 0 {
            *count as f64 / total as f64 * 100.0
        } else {
            0.0
        };
        lines.push(Line::from(vec![
            Span::styled(format!("{:<20}", truncate(key, 20)), Style::default().fg(color)),
            Span::raw(" "),
            Span::styled(bar, Style::default().fg(color)),
            Span::styled(
                format!(" {count:>5} {pct:>5.1}%"),
                Style::default().fg(Color::White),
            ),
            if show_blocked && *blocked > 0 {
                Span::styled(
                    format!(" ({blocked} blk)"),
                    Style::default().fg(Color::Red),
                )
            } else {
                Span::raw("")
            },
        ]));
    }
    if lines.is_empty() {
        lines.push(Line::from(Span::styled(
            "(no data)",
            Style::default().fg(Color::DarkGray),
        )));
    }
    Paragraph::new(Text::from(lines)).block(
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .title(title.to_string()),
    )
}

fn bar(value: i64, max: i64, width: usize) -> String {
    if max <= 0 {
        return String::new();
    }
    let n = ((value as f64 / max as f64) * width as f64).round() as usize;
    "█".repeat(n.min(width))
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(n.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

fn render_confirm(f: &mut Frame, app: &App) {
    let msg = match &app.confirm {
        Some(super::Action::Whitelist(d)) => format!("whitelist \"{d}\" and reload dnsmasq?  (y/n)"),
        Some(super::Action::Reload) => "reload dnsmasq?  (y/n)".to_string(),
        None => return,
    };
    let area = centered(f.area(), 60.min(f.area().width), 3);
    f.render_widget(Clear, area);
    let p = Paragraph::new(msg)
        .style(Style::default().fg(Color::Yellow))
        .alignment(ratatui::layout::Alignment::Center)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Double)
                .title(" confirm "),
        );
    f.render_widget(p, area);
}

fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let v = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Fill(1),
            Constraint::Length(height),
            Constraint::Fill(1),
        ])
        .split(area);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Fill(1),
            Constraint::Length(width),
            Constraint::Fill(1),
        ])
        .split(v[1])[1]
}
