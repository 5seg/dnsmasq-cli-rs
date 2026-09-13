//! ratatui による描画。

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{
    Block, BorderType, Borders, Cell, Clear, Gauge, Paragraph, Row, Table, TableState, Tabs,
};
use ratatui::Frame;

use crate::model::Outcome;
use crate::store::DbRow;
use crate::util::fmt_time;

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
    ];
    let sel = match app.tab {
        Tab::Live => 0,
        Tab::Stats => 1,
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

    // 下段を左右 2 カラム + qtype
    let cols = Layout::horizontal([
        Constraint::Percentage(50),
        Constraint::Percentage(50),
    ])
    .split(rows[1]);

    let left = Layout::vertical([Constraint::Percentage(60), Constraint::Percentage(40)])
        .split(cols[0]);

    f.render_widget(
        counts_block(
            " Top domains ",
            &s.domains
                .iter()
                .map(|c| (c.key.clone(), c.count, c.blocked))
                .collect::<Vec<_>>(),
            Color::Cyan,
        ),
        left[0],
    );
    f.render_widget(
        counts_block(
            " Top clients ",
            &s.clients
                .iter()
                .map(|c| (c.key.clone(), c.count, c.blocked))
                .collect::<Vec<_>>(),
            Color::Green,
        ),
        cols[1],
    );
    f.render_widget(
        counts_block(
            " qtypes ",
            &s.qtypes
                .iter()
                .map(|c| (c.key.clone(), c.count, c.blocked))
                .collect::<Vec<_>>(),
            Color::Magenta,
        ),
        left[1],
    );
}

fn counts_block(title: &str, items: &[(String, i64, i64)], color: Color) -> Paragraph<'static> {
    let max = items.iter().map(|(_, c, _)| *c).max().unwrap_or(1).max(1);
    let mut lines: Vec<Line> = Vec::new();
    for (key, count, blocked) in items {
        let bar = bar(*count, max, 14);
        lines.push(Line::from(vec![
            Span::styled(format!("{:<32}", truncate(key, 32)), Style::default().fg(color)),
            Span::raw(" "),
            Span::styled(bar, Style::default().fg(color)),
            Span::styled(format!(" {count}"), Style::default().fg(Color::White)),
            if *blocked > 0 {
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
