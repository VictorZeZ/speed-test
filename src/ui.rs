use crate::app::{App, RunState, SPINNER};
use crate::net::{ConnectionInfo, Phase, Profile};
use crate::history::TestRecord;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::symbols;
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, BorderType, Borders, Gauge, List, ListItem, ListState, Paragraph, Sparkline, Tabs,
};
use ratatui::Frame;

pub fn draw(f: &mut Frame, app: &mut App) {
    let outer = Layout::vertical([
        Constraint::Length(3),
        Constraint::Length(3),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .split(f.area());

    draw_info_strip(f, app, outer[0]);
    draw_tabs(f, app, outer[1]);

    match app.tab {
        crate::app::Tab::Test => draw_test(f, app, outer[2]),
        crate::app::Tab::History => draw_history(f, app, outer[2]),
        crate::app::Tab::Help => draw_help(f, outer[2]),
    }

    draw_footer(f, app, outer[3]);
}

pub fn speed_color(mbps: f64) -> Color {
    match mbps {
        x if x >= 300.0 => Color::LightGreen,
        x if x >= 100.0 => Color::Green,
        x if x >= 50.0 => Color::Yellow,
        x if x >= 25.0 => Color::LightYellow,
        x if x >= 10.0 => Color::LightRed,
        _ => Color::Red,
    }
}

fn draw_info_strip(f: &mut Frame, app: &App, area: Rect) {
    let cols = Layout::horizontal([
        Constraint::Percentage(24),
        Constraint::Percentage(44),
        Constraint::Min(12),
    ])
    .split(area);

    let (ip, isp, location) = if let Some(ref c) = app.connection {
        (c.client_ip.clone(), Some(isp_label(c)), location_label(c))
    } else {
        ("…".to_string(), None, "resolving…".to_string())
    };

    f.render_widget(info_cell("IP", &ip, Color::Cyan), cols[0]);

    match isp {
        Some(isp) => f.render_widget(info_cell("ISP", &isp, Color::LightMagenta), cols[1]),
        None => f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "▌ ISP   resolving…",
                Style::default().fg(Color::DarkGray),
            ))),
            cols[1],
        ),
    }

    f.render_widget(info_cell("LOCATION", &location, Color::LightGreen), cols[2]);
}

fn info_cell(label: &str, value: &str, color: Color) -> Paragraph<'static> {
    let label = format!("{label:<9}");
    Paragraph::new(vec![
        Line::from(vec![
            Span::styled("▌ ".to_string(), Style::default().fg(color)),
            Span::styled(
                label,
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(Span::styled(
            format!("  {value}"),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        )),
    ])
}

fn isp_label(c: &ConnectionInfo) -> String {
    match c.asn {
        Some(asn) => format!("{} (AS{})", c.as_organization, asn),
        None => c.as_organization.clone(),
    }
}

fn location_label(c: &ConnectionInfo) -> String {
    match (&c.city, &c.country) {
        (Some(city), Some(country)) if !city.is_empty() && !country.is_empty() => {
            format!("{city} | {country}")
        }
        (Some(city), _) if !city.is_empty() => city.clone(),
        (_, Some(country)) if !country.is_empty() => country.clone(),
        _ => c
            .colo
            .clone()
            .unwrap_or_else(|| "unknown".to_string()),
    }
}

fn draw_tabs(f: &mut Frame, app: &App, area: Rect) {
    let titles = vec![" 🚀 Test ", " 📜 History ", " ❓ Help "];
    let idx = app.tab.index();
    let tabs = Tabs::new(titles)
        .select(idx)
        .block(Block::new().borders(Borders::ALL).border_type(BorderType::Rounded))
        .style(Style::default().fg(Color::DarkGray))
        .highlight_style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD | Modifier::REVERSED),
        )
        .divider(symbols::line::VERTICAL);
    f.render_widget(tabs, area);
}

fn draw_footer(f: &mut Frame, app: &App, area: Rect) {
    let parts: Vec<Vec<Span>> = match app.tab {
        crate::app::Tab::Test => vec![
            key("ENTER", "start"),
            gap(),
            key("↑/↓", "profile"),
            gap(),
            key("ESC", "cancel/quit"),
            gap(),
            key("TAB", "switch tab"),
            gap(),
            key("Q", "quit"),
        ],
        crate::app::Tab::History => vec![
            key("↑/↓", "navigate"),
            gap(),
            key("D", "delete entry"),
            gap(),
            key("DEL", "clear all"),
            gap(),
            key("TAB", "switch tab"),
            gap(),
            key("Q", "quit"),
        ],
        crate::app::Tab::Help => vec![key("TAB", "back"), gap(), key("Q", "quit")],
    };
    let spans: Vec<Span> = parts.into_iter().flatten().collect();
    f.render_widget(Line::from(spans), area);
}

fn key(name: &str, action: &str) -> Vec<Span<'static>> {
    vec![
        Span::styled(
            format!(" {name} "),
            Style::default()
                .fg(Color::Black)
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!(" {action} "), Style::default().fg(Color::Gray)),
    ]
}

fn gap() -> Vec<Span<'static>> {
    vec![Span::raw("   ")]
}

fn draw_test(f: &mut Frame, app: &mut App, area: Rect) {
    let cols = Layout::horizontal([Constraint::Percentage(38), Constraint::Min(20)]).split(area);

    let left = Layout::vertical([Constraint::Min(12), Constraint::Length(9)]).split(cols[0]);
    let right = Layout::vertical([Constraint::Length(9), Constraint::Min(6)]).split(cols[1]);

    draw_latency_panel(f, app, left[0]);
    draw_profile_panel(f, app, left[1]);
    draw_status_panel(f, app, right[0]);
    draw_graph_panel(f, app, right[1]);
}

fn draw_latency_panel(f: &mut Frame, app: &mut App, area: Rect) {
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .title(Span::styled(
            " PING / JITTER ",
            Style::default().fg(Color::Magenta).add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(area);
    f.render_widget(block, area);

    if app.pings.is_empty() && !matches!(app.run_state, RunState::Finished) {
        let msg = match app.run_state {
            RunState::Running => Paragraph::new(SPINNER[app.spinner_frame]).alignment(Alignment::Center),
            _ => Paragraph::new(Line::from(Span::styled(
                "no data — run a test",
                Style::default().fg(Color::DarkGray),
            )))
            .alignment(Alignment::Center),
        };
        f.render_widget(msg, inner);
        return;
    }

    if let Some(lat) = &app.latency {
        let rows = Layout::vertical([Constraint::Length(4), Constraint::Min(3)]).split(inner);
        let ping_color = latency_color(lat.avg_ms);
        let lines = vec![
            Line::from(vec![
                Span::styled(" avg ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    format!("{:.1} ms", lat.avg_ms),
                    Style::default()
                        .fg(ping_color)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled("   min ", Style::default().fg(Color::DarkGray)),
                Span::styled(format!("{:.1}", lat.min_ms), Style::default().fg(ping_color)),
                Span::styled(" ms", Style::default().fg(Color::DarkGray)),
            ]),
            Line::from(vec![
                Span::styled(" max ", Style::default().fg(Color::DarkGray)),
                Span::styled(format!("{:.1}", lat.max_ms), Style::default().fg(ping_color)),
                Span::styled(" ms   ", Style::default().fg(Color::DarkGray)),
                Span::styled("jitter ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    format!("{:.1} ms", lat.jitter_ms),
                    Style::default()
                        .fg(jitter_color(lat.jitter_ms))
                        .add_modifier(Modifier::BOLD),
                ),
            ]),
            Line::from(""),
        ];
        f.render_widget(Paragraph::new(lines), rows[0]);
    }

    let data_start = app.latency.as_ref().map(|_| 0).unwrap_or(0);
    let spark_area = if app.latency.is_some() {
        let rows = Layout::vertical([Constraint::Length(4), Constraint::Min(3)]).split(inner);
        rows[1]
    } else {
        inner
    };
    let pings: Vec<u64> = app.pings[data_start.min(app.pings.len())..]
        .iter()
        .map(|p| p.round() as u64)
        .collect();
    if !pings.is_empty() {
        let spark = Sparkline::default()
            .data(&pings)
            .style(Style::default().fg(Color::Magenta));
        f.render_widget(spark, spark_area);
    }
}

fn latency_color(ms: f64) -> Color {
    match ms {
        x if x < 20.0 => Color::Green,
        x if x < 60.0 => Color::Yellow,
        _ => Color::Red,
    }
}

fn jitter_color(ms: f64) -> Color {
    match ms {
        x if x < 5.0 => Color::Green,
        x if x < 20.0 => Color::Yellow,
        _ => Color::Red,
    }
}

fn draw_profile_panel(f: &mut Frame, app: &mut App, area: Rect) {
    let running = app.run_state == RunState::Running;
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .title(Span::styled(
            " PROFILE ",
            Style::default().fg(Color::Blue).add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let items: Vec<ListItem> = Profile::ALL
        .iter()
        .map(|p| {
            let selected = *p == app.profile;
            let marker = if selected { "●" } else { "○" };
            let secs = p.phase_seconds();
            let style = if selected {
                Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::Gray)
            };
            let line = Line::from(vec![
                Span::styled(format!(" {marker} "), style),
                Span::styled(format!("{:<9}", p.name()), style),
                Span::styled(
                    format!("{}s ↓ / {}s ↑", secs.0, secs.1),
                    Style::default().fg(Color::DarkGray),
                ),
            ]);
            ListItem::new(line)
        })
        .collect();

    let list = List::new(items);
    let mut state = ListState::default();
    state.select(Some(app.profile.index()));
    if running {
        // dim while running
    }
    f.render_stateful_widget(list, inner, &mut state);
}

fn draw_status_panel(f: &mut Frame, app: &mut App, area: Rect) {
    let (title_color, title_text) = match app.run_state {
        RunState::Idle => (Color::DarkGray, " STATUS ".to_string()),
        RunState::Running => (Color::Yellow, format!(" {} {} ", SPINNER[app.spinner_frame], app.phase.label())),
        RunState::Finished => (Color::Green, " RESULTS ".to_string()),
    };

    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .title(Span::styled(
            title_text,
            Style::default().fg(title_color).add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(area);
    f.render_widget(block, area);

    match app.run_state {
        RunState::Idle => {
            let lines = vec![
                Line::from(""),
                Line::from(Span::styled(
                    "Press ENTER to start the test",
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ))
                .alignment(Alignment::Center),
                Line::from(Span::styled(
                    "powered by the Cloudflare speed network",
                    Style::default().fg(Color::DarkGray),
                ))
                .alignment(Alignment::Center),
            ];
            f.render_widget(Paragraph::new(lines), inner);
        }
        RunState::Running => {
            let is_down = app.phase == Phase::Download;
            let is_up = app.phase == Phase::Upload;

            if is_down || is_up || app.phase == Phase::Latency {
                let (instant, avg, label, color) = if is_down {
                    (app.down_instant, app.down_avg, "▼ DOWNLOAD", speed_color(app.down_instant))
                } else if is_up {
                    (app.up_instant, app.up_avg, "▲ UPLOAD", speed_color(app.up_instant))
                } else {
                    (0.0, 0.0, "• LATENCY", Color::Magenta)
                };

                let ratio = if is_down || is_up {
                    ((if is_down { app.down_avg } else { app.up_avg }) / 1000.0).clamp(0.02, 1.0)
                } else {
                    (app.pings.len() as f64 / 12.0).clamp(0.02, 1.0)
                };

                let rows = Layout::vertical([
                    Constraint::Length(2),
                    Constraint::Length(1),
                    Constraint::Length(1),
                    Constraint::Length(2),
                ])
                .split(inner);

                let big = Line::from(vec![
                    Span::styled(
                        format!("{:>8.1}", instant),
                        Style::default().fg(color).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(" Mbps", Style::default().fg(Color::DarkGray)),
                    Span::styled(
                        format!("   avg {:.1}", avg),
                        Style::default().fg(Color::Gray),
                    ),
                ]);
                f.render_widget(big.alignment(Alignment::Center), rows[0]);

                let gauge = Gauge::default()
                    .ratio(ratio)
                    .label(Span::styled(label, Style::default().fg(Color::Black)))
                    .gauge_style(Style::default().fg(color).bg(Color::Rgb(30, 30, 46)));
                f.render_widget(gauge, rows[2]);
            }
        }
        RunState::Finished => {
            if let Some(m) = &app.metrics {
                let (grade_char, grade_color) = App::grade(m.down_mbps);
                let rows = Layout::vertical([
                    Constraint::Length(1),
                    Constraint::Length(1),
                    Constraint::Length(1),
                ])
                .split(inner);

                let line = Line::from(vec![
                    Span::styled(
                        format!(" {grade_char} "),
                        Style::default()
                            .fg(Color::Black)
                            .bg(grade_color)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled("  ", Style::default()),
                    Span::styled("▼ ", Style::default().fg(speed_color(m.down_mbps))),
                    Span::styled(
                        format!("{:.1} Mbps", m.down_mbps),
                        Style::default()
                            .fg(speed_color(m.down_mbps))
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled("   ▲ ", Style::default().fg(speed_color(m.up_mbps))),
                    Span::styled(
                        format!("{:.1} Mbps", m.up_mbps),
                        Style::default()
                            .fg(speed_color(m.up_mbps))
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled("   ● ", Style::default().fg(latency_color(m.latency.avg_ms))),
                    Span::styled(
                        format!("{:.0} ms", m.latency.avg_ms),
                        Style::default().fg(Color::Gray),
                    ),
                ]);
                f.render_widget(line.alignment(Alignment::Center), rows[1]);
            }
        }
    }

    if let Some(err) = &app.error {
        let err_area = Rect {
            y: area.bottom().saturating_sub(2),
            height: 1,
            x: area.x + 1,
            width: area.width.saturating_sub(2),
        };
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                format!(" ✖ {err} "),
                Style::default().fg(Color::Black).bg(Color::Red),
            ))),
            err_area,
        );
    }
}

fn draw_graph_panel(f: &mut Frame, app: &mut App, area: Rect) {
    let (title, samples, color) = match app.run_state {
        RunState::Running if app.phase == Phase::Upload => {
            (" LIVE THROUGHPUT — UPLOAD ▲ ", &app.up_samples, Color::LightMagenta)
        }
        RunState::Running if app.phase == Phase::Download => {
            (" LIVE THROUGHPUT — DOWNLOAD ▼ ", &app.down_samples, Color::LightBlue)
        }
        RunState::Running => (" THROUGHPUT ", &Vec::new(), Color::DarkGray),
        RunState::Finished => (" THROUGHPUT — LAST RUN ", &app.down_samples, Color::Blue),
        RunState::Idle => (" THROUGHPUT ", &Vec::new(), Color::DarkGray),
    };

    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .title(Span::styled(title, Style::default().fg(color)));
    let inner = block.inner(area);
    f.render_widget(block, area);

    if samples.is_empty() {
        let hint = match app.run_state {
            RunState::Idle => "throughput graph will appear here during the test",
            RunState::Running => "warming up…",
            _ => "",
        };
        f.render_widget(
            Paragraph::new(Span::styled(hint, Style::default().fg(Color::DarkGray)))
                .alignment(Alignment::Center),
            inner,
        );
        return;
    }

    let spark = Sparkline::default()
        .data(samples)
        .style(Style::default().fg(color));
    f.render_widget(spark, inner);
}

fn draw_history(f: &mut Frame, app: &mut App, area: Rect) {
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .title(Span::styled(
            format!(" HISTORY ({} runs) ", app.history.len()),
            Style::default().fg(Color::Green).add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(area);
    f.render_widget(block, area);

    if app.history.is_empty() {
        f.render_widget(
            Paragraph::new(Span::styled(
                "no runs yet — complete a test to record history",
                Style::default().fg(Color::DarkGray),
            ))
            .alignment(Alignment::Center),
            inner,
        );
        return;
    }

    app.history_selected = app.history_selected.min(app.history.len() - 1);

    let header = history_header_row();
    let rows: Vec<ratatui::widgets::Row> = app
        .history
        .iter()
        .enumerate()
        .map(|(i, r)| history_row(i, r, i == app.history_selected))
        .collect();

    let widths = [
        Constraint::Length(4),
        Constraint::Length(22),
        Constraint::Length(10),
        Constraint::Length(16),
        Constraint::Length(14),
        Constraint::Length(10),
        Constraint::Length(8),
        Constraint::Min(4),
    ];

    let table = ratatui::widgets::Table::new(rows, widths)
        .header(header)
        .row_highlight_style(
            Style::default()
                .bg(Color::Rgb(50, 50, 80))
                .add_modifier(Modifier::BOLD),
        );

    f.render_stateful_widget(
        table,
        inner,
        &mut ratatui::widgets::TableState::default().with_selected(Some(app.history_selected)),
    );
}

use ratatui::widgets::{Cell, Row};

fn history_header_row() -> Row<'static> {
    Row::new(vec![
        Cell::from("#").style(Style::default().fg(Color::DarkGray)),
        Cell::from("when").style(Style::default().fg(Color::DarkGray)),
        Cell::from("grade").style(Style::default().fg(Color::DarkGray)),
        Cell::from("download").style(Style::default().fg(Color::DarkGray)),
        Cell::from("upload").style(Style::default().fg(Color::DarkGray)),
        Cell::from("ping").style(Style::default().fg(Color::DarkGray)),
        Cell::from("jitter").style(Style::default().fg(Color::DarkGray)),
        Cell::from("profile").style(Style::default().fg(Color::DarkGray)),
    ])
}

fn history_row(idx: usize, r: &TestRecord, _selected: bool) -> Row<'static> {
    let (_, grade_color) = App::grade(r.down_mbps);
    let when = r.timestamp.with_timezone(&chrono::Local);
    Row::new(vec![
        Cell::from(format!("{}", idx + 1)).style(Style::default().fg(Color::DarkGray)),
        Cell::from(when.format("%Y-%m-%d %H:%M").to_string())
            .style(Style::default().fg(Color::Gray)),
        Cell::from(format!(" {} ", r.grade))
            .style(Style::default().fg(Color::Black).bg(grade_color)),
        Cell::from(format!("{:>7.1} Mbps", r.down_mbps))
            .style(Style::default().fg(speed_color(r.down_mbps))),
        Cell::from(format!("{:>7.1} Mbps", r.up_mbps))
            .style(Style::default().fg(speed_color(r.up_mbps))),
        Cell::from(format!("{:>5.0} ms", r.ping_ms))
            .style(Style::default().fg(latency_color(r.ping_ms))),
        Cell::from(format!("{:>5.1} ms", r.jitter_ms)).style(Style::default().fg(Color::Gray)),
        Cell::from(r.profile.clone()).style(Style::default().fg(Color::Cyan)),
    ])
}

fn draw_help(f: &mut Frame, area: Rect) {
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .title(Span::styled(
            " HELP ",
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
        ));
    let text = vec![
        Line::from(""),
        Line::from(vec![
            key_span("ENTER"),
            plain(" start a new speed test"),
        ]),
        Line::from(vec![
            key_span("↑ / ↓"),
            plain(" cycle test profiles (Quick / Standard / Maximum)"),
        ]),
        Line::from(vec![
            key_span("ESC"),
            plain(" cancel a running test / quit from idle"),
        ]),
        Line::from(vec![key_span("TAB"), plain(" next tab   "), key_span("SHIFT+TAB"), plain(" previous tab")]),
        Line::from(vec![
            key_span("D"),
            plain(" delete selected history entry   "),
            key_span("DEL"),
            plain(" clear history"),
        ]),
        Line::from(vec![
            key_span("Q"),
            plain(" or "),
            key_span("CTRL+C"),
            plain(" quit"),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            "Grades are based on download speed:",
            Style::default().fg(Color::DarkGray),
        )),
        Line::from(vec![
            grade_span('S', "≥ 300 Mbps", Color::LightGreen),
            grade_span('A', "≥ 100", Color::Green),
            grade_span('B', "≥ 50", Color::Yellow),
            grade_span('C', "≥ 25", Color::LightYellow),
            grade_span('D', "≥ 10", Color::LightRed),
            grade_span('F', "< 10", Color::Red),
        ]),
    ];
    f.render_widget(Paragraph::new(text).block(block), area);
}

fn key_span(s: &str) -> Span<'static> {
    Span::styled(
        format!(" {s} "),
        Style::default()
            .fg(Color::Black)
            .bg(Color::DarkGray)
            .add_modifier(Modifier::BOLD),
    )
}

fn plain(s: &str) -> Span<'static> {
    Span::raw(format!(" {s}"))
}

fn grade_span(grade: char, range: &str, color: Color) -> Span<'static> {
    Span::styled(
        format!("  {grade}: {range} "),
        Style::default().fg(color).add_modifier(Modifier::BOLD),
    )
}
