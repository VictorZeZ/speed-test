use crate::app::{App, Incident, MonitorStats, RunState, SPINNER};
use crate::history::TestRecord;
use crate::keys;
use crate::net::{ConnectionInfo, Phase, Profile};
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::symbols;
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, BorderType, Borders, Gauge, List, ListItem, ListState, Paragraph, Sparkline, Tabs,
};
use ratatui::Frame;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

pub fn draw(f: &mut Frame, app: &mut App) {
    let area = f.area();
    if area.width < 50 || area.height < 12 {
        // Degenerate terminal sizes: render a friendly notice instead of
        // letting the layout produce overlapping garbage.
        f.render_widget(
            Paragraph::new(Span::styled(
                "terminal too small — resize to at least 50×12",
                Style::default().fg(Color::Yellow),
            ))
            .alignment(Alignment::Center),
            area,
        );
        return;
    }
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
        crate::app::Tab::Monitor => draw_monitor(f, app, outer[2]),
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

    let refreshing = app.connection_refreshing;
    let (ip, isp, location, note) = if let Some(ref c) = app.connection {
        (
            c.client_ip.clone(),
            Some(isp_label(c)),
            location_label(c),
            if refreshing { "  [F5]" } else { "" },
        )
    } else {
        ("…".to_string(), None, "resolving…".to_string(), "")
    };

    f.render_widget(
        info_cell("IP", &format!("{ip}{note}"), Color::Cyan),
        cols[0],
    );

    match isp {
        Some(isp) => f.render_widget(
            info_cell("ISP", &format!("{isp}{note}"), Color::LightMagenta),
            cols[1],
        ),
        None => f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "▌ ISP   resolving…",
                Style::default().fg(Color::DarkGray),
            ))),
            cols[1],
        ),
    }

    f.render_widget(
        info_cell(
            "LOCATION",
            &format!("{location}{note}"),
            Color::LightGreen,
        ),
        cols[2],
    );
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
    let titles = vec![" Test ", " Monitor ", " History ", " Help "];
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

fn draw_footer(f: &mut Frame, app: &mut App, area: Rect) {
    // Generated from the central KEYMAP: every binding whose scope is Global
    // or matches the active tab. When the line is wider than the terminal it
    // scrolls horizontally in a smooth ping-pong motion so every shortcut
    // becomes visible without requiring any input.
    let scope = match app.tab {
        crate::app::Tab::Test => keys::Scope::Test,
        crate::app::Tab::Monitor => keys::Scope::Monitor,
        crate::app::Tab::History | crate::app::Tab::Help => keys::Scope::History,
    };

    let mut spans: Vec<Span> = Vec::new();
    let mut first = true;
    let ordered: Vec<&keys::KeyDef> = keys::KEYMAP
        .iter()
        .filter(|d| d.scope == scope)
        .chain(keys::KEYMAP.iter().filter(|d| d.scope == keys::Scope::Global))
        .collect();
    for def in ordered {
        let desc = match (def.action, app.tab, app.run_state, app.monitor.running) {
            (keys::Action::StartStopTest, _, RunState::Running, _) => "stop loop",
            (keys::Action::StartStopTest, _, _, _) => "start test",
            (keys::Action::MonitorToggle, crate::app::Tab::Monitor, _, true) => "stop ping",
            (keys::Action::MonitorToggle, crate::app::Tab::Monitor, _, false) => "start ping",
            _ => def.desc,
        };
        if !first {
            spans.push(Span::styled(
                " | ".to_string(),
                Style::default().fg(Color::Rgb(70, 70, 100)),
            ));
        }
        spans.extend(key(def.labels[0], desc));
        first = false;
    }

    let total: usize = spans.iter().map(|s| s.width()).sum();
    let viewport = area.width as usize;

    if total <= viewport {
        // Everything fits: render statically and reset the marquee.
        app.footer_scroll = 0;
        app.footer_dir = 1;
        app.footer_next_step = None;
        f.render_widget(Line::from(spans), area);
        return;
    }

    advance_footer_marquee(app, total as u16, viewport as u16);
    let offset = app.footer_scroll as usize;
    let visible = slice_line(&spans, offset, viewport);
    f.render_widget(Line::from(visible), area);
}

/// Advance the ping-pong scroll offset by one column per step, pausing
/// briefly at both edges so entries can actually be read before the
/// direction flips.
fn advance_footer_marquee(app: &mut App, total: u16, viewport: u16) {
    const STEP_MS: u64 = 110; // ~9 columns/second: readable, not distracting
    const EDGE_PAUSE_MS: u64 = 1500;

    let max_offset = total.saturating_sub(viewport);
    if app.footer_scroll > max_offset {
        app.footer_scroll = max_offset;
    }

    let now = Instant::now();
    if let Some(next) = app.footer_next_step {
        if now < next {
            return; // between steps (or paused at an edge)
        }
    }

    if app.footer_dir < 0 {
        if app.footer_scroll == 0 {
            app.footer_dir = 1;
            app.footer_next_step = Some(now + Duration::from_millis(EDGE_PAUSE_MS));
        } else {
            app.footer_scroll -= 1;
            app.footer_next_step = Some(now + Duration::from_millis(STEP_MS));
        }
    } else {
        if app.footer_scroll >= max_offset {
            app.footer_dir = -1;
            app.footer_next_step = Some(now + Duration::from_millis(EDGE_PAUSE_MS));
        } else {
            app.footer_scroll += 1;
            app.footer_next_step = Some(now + Duration::from_millis(STEP_MS));
        }
    }
}

/// Extract a `[start, start+width)` window from styled spans, preserving each
/// span's style and clipping content at both boundaries. Used by the
/// horizontally scrollable footer.
pub(crate) fn slice_line(spans: &[Span<'_>], start: usize, width: usize) -> Vec<Span<'static>> {
    let mut out: Vec<Span<'static>> = Vec::new();
    if width == 0 {
        return out;
    }
    let end_at = start + width;
    let mut pos = 0usize;
    for span in spans {
        let span_end = pos + span.width();
        pos = span_end;
        if span_end <= start || pos - span.width() >= end_at {
            continue;
        }
        let rel_start = start.saturating_sub(pos - span.width());
        let rel_end = (end_at.min(span_end)) - (pos - span.width());
        let content: String = span.content.chars().skip(rel_start).take(rel_end - rel_start).collect();
        out.push(Span::styled(content, span.style));
    }
    out
}

/// A footer entry: the key as a bright badge, its description in a dimmer,
/// clearly different color so the eye separates "what to press" from
/// "what it does".
fn key(name: &str, action: &str) -> Vec<Span<'static>> {
    vec![
        Span::styled(
            format!(" {} ", name),
            Style::default()
                .fg(Color::Cyan)
                .bg(Color::Rgb(40, 40, 66))
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(" {} ", action),
            Style::default().fg(Color::Gray),
        ),
    ]
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
            let marker = if selected { ">" } else { " " };
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
                    format!("{}s down / {}s up", secs.0, secs.1),
                    Style::default().fg(Color::DarkGray),
                ),
            ]);
            ListItem::new(line)
        })
        .collect();

    let list = List::new(items);
    let mut state = ListState::default();
    state.select(Some(app.profile.index()));
    f.render_stateful_widget(list, inner, &mut state);

    let mode_area = Rect {
        y: inner.y + 4,
        height: 1,
        ..inner
    };
    if app.run_state != RunState::Running {
        let (marker, style) = if app.continuous_mode {
            (">", Style::default().fg(Color::LightMagenta).add_modifier(Modifier::BOLD))
        } else {
            (" ", Style::default().fg(Color::DarkGray))
        };
        let mode_line = Line::from(vec![
            Span::raw(" "),
            Span::styled(marker, style),
            Span::styled(
                format!(" {:<9}", if app.continuous_mode { "Continuous" } else { "Single" }),
                if app.continuous_mode {
                    Style::default().fg(Color::LightMagenta).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::Gray)
                },
            ),
            Span::styled("[Ins]", Style::default().fg(Color::DarkGray)),
        ]);
        f.render_widget(mode_line, mode_area);
    } else if app.runs_completed > 0 {
        // While a continuous session runs, show session progress instead.
        let line = Line::from(vec![
            Span::raw(" "),
            Span::styled(
                format!(
                    "runs: {}   avg dl {:.0} / up {:.0} Mbps",
                    app.runs_completed,
                    if app.runs_completed > 0 { app.agg_down_sum / app.runs_completed as f64 } else { 0.0 },
                    if app.runs_completed > 0 { app.agg_up_sum / app.runs_completed as f64 } else { 0.0 },
                ),
                Style::default().fg(Color::LightMagenta),
            ),
        ]);
        f.render_widget(line, mode_area);
    }
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
            let mode_hint = if app.continuous_mode {
                "continuous mode — tests repeat until you press ENTER again"
            } else {
                "press M for continuous mode · powered by the Cloudflare network"
            };
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
                    mode_hint,
                    Style::default().fg(Color::DarkGray),
                ))
                .alignment(Alignment::Center),
            ];
            f.render_widget(Paragraph::new(lines), inner);
        }
        RunState::Running => {
            let is_down = app.phase == Phase::Download;
            let is_up = app.phase == Phase::Upload;

            let title_suffix = if app.continuous_mode {
                format!("  · loop run #{}", app.runs_completed + 1)
            } else {
                String::new()
            };

            if is_down || is_up || app.phase == Phase::Latency {
                let (instant, avg, label, color) = if is_down {
                    (app.down_instant, app.down_avg, "DOWNLOAD", speed_color(app.down_instant))
                } else if is_up {
                    (app.up_instant, app.up_avg, "UPLOAD", speed_color(app.up_instant))
                } else {
                    (0.0, 0.0, "LATENCY", Color::Magenta)
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
                    .label(Span::styled(
                        format!("{label}{title_suffix}"),
                        Style::default().fg(Color::Black),
                    ))
                    .gauge_style(Style::default().fg(color).bg(Color::Rgb(30, 30, 46)));
                f.render_widget(gauge, rows[2]);

                if app.continuous_mode && inner.height > 5 {
                    let hint_area = Rect { y: rows[3].y + 1, height: 1, ..rows[3] };
                    f.render_widget(
                        Line::from(Span::styled(
                            "ENTER stops after this run",
                            Style::default().fg(Color::DarkGray),
                        ))
                        .alignment(Alignment::Center),
                        hint_area,
                    );
                }
            }
        }
        RunState::Finished => {
            if let Some(m) = &app.metrics {
                let (grade_char, grade_color) = App::grade(m.down_mbps);
                let lines_area = Layout::vertical([
                    Constraint::Length(1),
                    Constraint::Length(1),
                    Constraint::Length(1),
                    Constraint::Min(1),
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
                    Span::styled("down ", Style::default().fg(speed_color(m.down_mbps))),
                    Span::styled(
                        format!("{:.1} Mbps", m.down_mbps),
                        Style::default()
                            .fg(speed_color(m.down_mbps))
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled("   up ", Style::default().fg(speed_color(m.up_mbps))),
                    Span::styled(
                        format!("{:.1} Mbps", m.up_mbps),
                        Style::default()
                            .fg(speed_color(m.up_mbps))
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled("   ping ", Style::default().fg(latency_color(m.latency.avg_ms))),
                    Span::styled(
                        format!("{:.0} ms", m.latency.avg_ms),
                        Style::default().fg(Color::Gray),
                    ),
                ]);
                f.render_widget(line.alignment(Alignment::Center), lines_area[1]);

                if app.runs_completed > 1 {
                    let n = app.runs_completed as f64;
                    let session = Line::from(vec![
                        Span::styled(
                            format!(
                                "session: {} runs · avg down {:.0} / up {:.0} Mbps · best {:.0} · avg ping {:.0} ms",
                                app.runs_completed,
                                app.agg_down_sum / n,
                                app.agg_up_sum / n,
                                app.best_down,
                                app.agg_ping_sum / n,
                            ),
                            Style::default().fg(Color::LightMagenta),
                        ),
                    ]);
                    f.render_widget(session.alignment(Alignment::Center), lines_area[3]);
                } else if app.runs_completed == 1 {
                    let session = Line::from(Span::styled(
                        "1 completed run — results saved to history",
                        Style::default().fg(Color::DarkGray),
                    ));
                    f.render_widget(session.alignment(Alignment::Center), lines_area[3]);
                }
            } else {
                // Continuous session stopped before any run could complete.
                f.render_widget(
                    Line::from(Span::styled(
                        "stopped — no completed runs in this session",
                        Style::default().fg(Color::Yellow),
                    ))
                    .alignment(Alignment::Center),
                    inner,
                );
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
                format!(" ERROR: {err} "),
                Style::default().fg(Color::Black).bg(Color::Red),
            ))),
            err_area,
        );
    }
}

fn draw_graph_panel(f: &mut Frame, app: &mut App, area: Rect) {
    let (title, samples, color) = match app.run_state {
        RunState::Running if app.phase == Phase::Upload => {
            (" LIVE THROUGHPUT - UPLOAD ", &app.up_samples, Color::LightMagenta)
        }
        RunState::Running if app.phase == Phase::Download => {
            (" LIVE THROUGHPUT - DOWNLOAD ", &app.down_samples, Color::LightBlue)
        }
        RunState::Running => (" THROUGHPUT ", &Vec::new(), Color::DarkGray),
        RunState::Finished => (" THROUGHPUT - LAST RUN ", &app.down_samples, Color::Blue),
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

fn ping_quality_color(rtt: f64, gaming: bool) -> Color {
    let (good, playable) = if gaming { (60.0, 100.0) } else { (100.0, 160.0) };
    if rtt <= good {
        Color::Green
    } else if rtt <= playable {
        Color::Yellow
    } else {
        Color::Red
    }
}

fn stability_color(pct: f64) -> Color {
    match pct {
        x if x >= 95.0 => Color::LightGreen,
        x if x >= 85.0 => Color::Green,
        x if x >= 65.0 => Color::Yellow,
        _ => Color::Red,
    }
}

fn draw_monitor(f: &mut Frame, app: &mut App, area: Rect) {
    let stats = app.monitor.stats();

    let rows = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(10),
        Constraint::Length(9),
    ])
    .split(area);

    draw_monitor_banner(f, app, &stats, rows[0]);
    draw_monitor_main(f, app, &stats, rows[1]);
    draw_incident_log(f, app, rows[2]);

    if app.monitor.editing_target {
        draw_target_editor(f, app, area);
    }
}

fn draw_monitor_banner(f: &mut Frame, app: &App, stats: &MonitorStats, area: Rect) {
    let spans = if app.monitor.paused {
        vec![
            Span::styled("[paused] ", Style::default().fg(Color::Yellow)),
            Span::styled(
                "monitor paused during the speed test - it resumes automatically",
                Style::default().fg(Color::DarkGray),
            ),
        ]
    } else if !app.monitor.running {
        vec![
            Span::styled("[idle] ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                "press ENTER to start continuous ping",
                Style::default().fg(Color::DarkGray),
            ),
        ]
    } else if stats.in_trouble {
        let detail = app
            .monitor
            .incidents
            .last()
            .map(|i| i.detail.clone())
            .unwrap_or_default();
        vec![
            Span::styled(
                format!(" !! NETWORK TROUBLE DETECTED - {detail} "),
                Style::default()
                    .fg(Color::White)
                    .bg(Color::Red)
                    .add_modifier(Modifier::BOLD),
            ),
        ]
    } else {
        vec![
            Span::styled("LIVE ", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
            Span::styled(
                "monitoring",
                Style::default().fg(Color::Gray),
            ),
            Span::styled(
                format!(
                    " · stable for {}s",
                    stats.stable_for_secs
                ),
                Style::default().fg(Color::DarkGray),
            ),
        ]
    };

    let verdict_span = Span::styled(
        format!("{} ", stats.verdict.0),
        Style::default()
            .fg(stats.verdict.1)
            .add_modifier(Modifier::BOLD),
    );

    let mut line_spans = spans;
    // right-align the verdict by padding to width
    let used: usize = line_spans.iter().map(|s| s.width()).sum();
    let total = area.width as usize;
    let pad = total.saturating_sub(used + stats.verdict.0.len() + 2);
    line_spans.push(Span::raw(" ".repeat(pad)));
    line_spans.push(verdict_span);

    f.render_widget(Paragraph::new(Line::from(line_spans)), area);
}

fn draw_monitor_main(f: &mut Frame, app: &mut App, stats: &MonitorStats, area: Rect) {
    let cols = Layout::horizontal([Constraint::Percentage(46), Constraint::Min(20)]).split(area);
    let gaming = app.monitor.gaming;

    // ----- left: latency readout (aligned two-column stat grid) -----
    let accent = if gaming { Color::LightGreen } else { Color::Cyan };
    let left_block = Block::bordered()
        .border_type(BorderType::Rounded)
        .title(Span::styled(
            if gaming { " GAMING LATENCY " } else { " LATENCY " },
            Style::default().fg(accent).add_modifier(Modifier::BOLD),
        ));
    let inner = left_block.inner(cols[0]);
    f.render_widget(left_block, cols[0]);

    // Grid geometry: two label+value columns, fixed widths so values line up
    // perfectly regardless of content length.
    // | label(9) value(9) gap(3) label(9) value(9) |
    let cell = |label: &str, value: &str, color: Color| -> [Span<'static>; 2] {
        [
            Span::styled(format!(" {:<9}", label), Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!("{:>8} ", value),
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            ),
        ]
    };

    let ms = |v: f64| format!("{:.1} ms", v);
    let loss_color = if stats.loss_pct == 0.0 {
        Color::Green
    } else if stats.loss_pct < 3.0 {
        Color::Yellow
    } else {
        Color::Red
    };

    let mut lines: Vec<Line> = Vec::new();

    // Big current ping, centered.
    lines.push(Line::from(""));
    match stats.cur {
        Some(rtt) => lines.push(
            Line::from(vec![
                Span::styled(
                    format!("{:>5.1}", rtt),
                    Style::default()
                        .fg(ping_quality_color(rtt, gaming))
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(" ms  current", Style::default().fg(Color::DarkGray)),
            ])
            .alignment(Alignment::Center),
        ),
        None => lines.push(
            Line::from(Span::styled("—  no samples yet", Style::default().fg(Color::DarkGray)))
                .alignment(Alignment::Center),
        ),
    }
    lines.push(Line::from(Span::styled(
        "─────────────",
        Style::default().fg(Color::Rgb(60, 60, 90)),
    ))
    .alignment(Alignment::Center));
    lines.push(Line::from(""));

    // Two-column grid; every row uses identical widths.
    let rows: [([Span; 2], [Span; 2]); 3] = [
        (
            cell("avg", &ms(stats.avg), ping_quality_color(stats.avg, gaming)),
            cell("min", &ms(stats.min), Color::Gray),
        ),
        (
            cell("max", &ms(stats.max), Color::Gray),
            cell("jitter", &ms(stats.jitter), jitter_color(stats.jitter)),
        ),
        (
            cell("loss", &format!("{:.2} %", stats.loss_pct), loss_color),
            cell(
                "stability",
                &format!("{:.1} %", stats.stability),
                stability_color(stats.stability),
            ),
        ),
    ];
    for (left_cells, right_cells) in rows {
        let mut spans = Vec::with_capacity(4);
        spans.extend(left_cells);
        spans.push(Span::raw(" "));
        spans.extend(right_cells);
        lines.push(Line::from(spans));
    }

    // Probes row spans the full width beneath the grid.
    lines.push(Line::from(vec![Span::styled(
        format!(
            " {:<9}{:>8} ",
            "probes",
            format!("{}/{}", stats.received, stats.sent)
        ),
        Style::default().fg(Color::DarkGray),
    )]));

    if gaming && inner.height as usize > lines.len() + 2 {
        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::styled(" thresholds ", Style::default().fg(Color::DarkGray)),
            Span::styled("<60", Style::default().fg(Color::Green)),
            Span::raw("  "),
            Span::styled("<=100", Style::default().fg(Color::Yellow)),
            Span::raw("  "),
            Span::styled(">100 ms", Style::default().fg(Color::Red)),
        ]));
    }

    // Stability gauge pinned to the bottom in gaming mode.
    if gaming && inner.height > 14 {
        let gauge_area = Rect {
            y: inner.y + inner.height - 2,
            height: 1,
            ..inner
        };
        let ratio = (stats.stability / 100.0).clamp(0.0, 1.0);
        let gauge = Gauge::default()
            .ratio(ratio)
            .label(Span::styled(
                format!("STABILITY {:.0}%", stats.stability),
                Style::default().fg(Color::Black),
            ))
            .gauge_style(Style::default().fg(stability_color(stats.stability)));
        f.render_widget(gauge, gauge_area);
    }

    f.render_widget(Paragraph::new(lines), inner);

    // ----- right: live graph -----
    let graph_color = if stats.in_trouble {
        Color::Red
    } else if gaming {
        Color::LightGreen
    } else {
        Color::Cyan
    };
    let interval_ms = app.monitor.interval_ms.load(Ordering::Relaxed);
    let title = format!(
        " LIVE PING — {} @ {}ms ",
        app.monitor.target, interval_ms
    );
    let right_block = Block::bordered()
        .border_type(BorderType::Rounded)
        .title(Span::styled(title, Style::default().fg(graph_color)));
    let inner = right_block.inner(cols[1]);
    f.render_widget(right_block, cols[1]);

    if app.monitor.recent.is_empty() {
        f.render_widget(
            Paragraph::new(Span::styled(
                if app.monitor.running { "waiting for first probe…" } else { "graph appears here once the monitor runs" },
                Style::default().fg(Color::DarkGray),
            ))
            .alignment(Alignment::Center),
            inner,
        );
        return;
    }

    let data: Vec<u64> = app
        .monitor
        .recent
        .iter()
        .map(|s| s.map(|r| r.round() as u64).unwrap_or(0))
        .collect();
    let spark = Sparkline::default()
        .data(&data)
        .style(Style::default().fg(graph_color));
    f.render_widget(spark, inner);

    // loss markers row under the graph
    if inner.height > 2 {
        let marker_area = Rect {
            y: inner.y + inner.height - 1,
            height: 1,
            ..inner
        };
        let lost_count = app.monitor.lost;
        let marker = if lost_count > 0 {
            Span::styled(format!(" [{}] lost probes shown as dips ", lost_count), Style::default().fg(Color::DarkGray))
        } else {
            Span::styled(" no loss recorded", Style::default().fg(Color::DarkGray))
        };
        f.render_widget(Line::from(marker), marker_area);
    }
}

fn draw_incident_log(f: &mut Frame, app: &mut App, area: Rect) {    let trouble = app
        .monitor
        .trouble_until
        .map(|t| std::time::Instant::now() < t)
        .unwrap_or(false);
    let title_color = if trouble { Color::Red } else { Color::Yellow };
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .title(Span::styled(
            format!(" EVENTS / TROUBLE LOG ({} ) ", app.monitor.incidents.len()),
            Style::default().fg(title_color).add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(area);
    f.render_widget(block, area);

    if app.monitor.incidents.is_empty() {
        f.render_widget(
            Paragraph::new(Span::styled(
                "no network trouble detected — spikes, loss and jitter bursts will be logged here",
                Style::default().fg(Color::DarkGray),
            ))
            .alignment(Alignment::Center),
            inner,
        );
        return;
    }

    let visible = inner.height as usize;
    let start = app.monitor.incidents.len().saturating_sub(visible);
    let lines: Vec<Line> = app.monitor.incidents[start..]
        .iter()
        .rev()
        .map(|i: &Incident| incident_line(i))
        .collect();
    f.render_widget(Paragraph::new(lines), inner);
}

fn incident_line(i: &Incident) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!(" {} ", i.at.format("%H:%M:%S")),
            Style::default().fg(Color::DarkGray),
        ),
        Span::styled(
            format!(" {} ", i.kind.label()),
            Style::default()
                .fg(Color::Black)
                .bg(i.kind.color())
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!(" {}", i.detail), Style::default().fg(Color::Gray)),
    ])
}

fn draw_target_editor(f: &mut Frame, app: &App, area: Rect) {
    let w = 62.min(area.width.saturating_sub(4));
    let h = 5;
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + (area.height.saturating_sub(h)) / 2;
    let popup = Rect { x, y, width: w, height: h };

    f.render_widget(ratatui::widgets::Clear, popup);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .title(Span::styled(
            " TARGET HOST ",
            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        ))
        .style(Style::default().bg(Color::Rgb(25, 25, 40)));
    let inner = block.inner(popup);
    f.render_widget(block, popup);

    let cursor = if app.spinner_frame % 2 == 0 { "█" } else { " " };
    let shown: String = app.monitor.input_buf.chars().take(inner.width as usize - 6).collect();
    let input = Paragraph::new(Line::from(vec![
        Span::styled(" > ", Style::default().fg(Color::Cyan)),
        Span::styled(shown, Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
        Span::styled(cursor, Style::default().fg(Color::Cyan)),
        Span::styled("   ENTER apply · ESC cancel", Style::default().fg(Color::DarkGray)),
    ]));
    f.render_widget(input, Rect { x: inner.x, y: inner.y + 1, height: 1, ..inner });
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
            " HELP — every shortcut works on any keyboard layout ",
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
        ));

    // The entire shortcut list is generated from the central KEYMAP table,
    // so it can never drift out of sync with actual behavior.
    let mut text: Vec<Line> = vec![Line::from("")];
    for scope in [
        keys::Scope::Global,
        keys::Scope::Test,
        keys::Scope::Monitor,
        keys::Scope::History,
    ] {
        text.push(Line::from(Span::styled(
            format!(" {} ", scope.label()),
            Style::default()
                .fg(scope.color())
                .add_modifier(Modifier::BOLD),
        )));
        for def in keys::KEYMAP.iter().filter(|d| d.scope == scope) {
            let mut spans = Vec::new();
            for (i, label) in def.labels.iter().enumerate() {
                if i > 0 {
                    spans.push(Span::styled("/", Style::default().fg(Color::DarkGray)));
                }
                spans.push(key_span(label));
            }
            spans.push(plain(def.desc));
            // Universal-key hint when the first binding is a letter alias.
            if !def.labels.is_empty() {
                let universal: Vec<&str> = def
                    .labels
                    .iter()
                    .skip_while(|l| l.len() == 1)
                    .copied()
                    .collect();
                if universal.is_empty() {
                    spans.push(Span::styled(
                        "  (universal key)",
                        Style::default().fg(Color::DarkGray),
                    ));
                } else if def.labels.len() > 1 && universal.len() < def.labels.len() {
                    spans.push(Span::styled(
                        format!("  · universal: {}", universal.join("/")),
                        Style::default().fg(Color::DarkGray),
                    ));
                }
            }
            text.push(Line::from(spans));
        }
        text.push(Line::from(""));
    }

    text.push(Line::from(Span::styled(
        "NOTES",
        Style::default().fg(Color::Magenta).add_modifier(Modifier::BOLD),
    )));
    for note in [
        "Trouble detection logs spikes, packet loss, outages and jitter bursts.",
        "Probes are tiny (~100 bytes): monitoring never loads your connection,",
        "and it auto-pauses during speed tests so results stay clean.",
        "If a test server stops responding, the app aborts with an error",
        "message instead of hanging.",
        "The shortcut bar scrolls by itself when there is not enough room",
        "for every entry on screen.",
    ] {
        text.push(Line::from(Span::styled(
            format!(" {note}"),
            Style::default().fg(Color::DarkGray),
        )));
    }
    text.push(Line::from(""));
    text.push(Line::from(Span::styled(
        "Speed grades are based on download speed:",
        Style::default().fg(Color::DarkGray),
    )));
    text.push(Line::from(vec![
        grade_span('S', "≥ 300 Mbps", Color::LightGreen),
        grade_span('A', "≥ 100", Color::Green),
        grade_span('B', "≥ 50", Color::Yellow),
        grade_span('C', "≥ 25", Color::LightYellow),
        grade_span('D', "≥ 10", Color::LightRed),
        grade_span('F', "< 10", Color::Red),
    ]));
    f.render_widget(Paragraph::new(text).block(block), area);
}

fn key_span(s: &str) -> Span<'static> {
    Span::styled(
        format!(" {s} "),
        Style::default()
            .fg(Color::Cyan)
            .bg(Color::Rgb(40, 40, 66))
            .add_modifier(Modifier::BOLD),
    )
}

fn plain(s: &str) -> Span<'static> {
    Span::styled(format!(" {s}"), Style::default().fg(Color::Gray))
}

fn grade_span(grade: char, range: &str, color: Color) -> Span<'static> {
    Span::styled(
        format!("  {grade}: {range} "),
        Style::default().fg(color).add_modifier(Modifier::BOLD),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::{Color, Style};

    fn span(s: &str) -> Span<'static> {
        Span::styled(s.to_string(), Style::default().fg(Color::Cyan))
    }

    #[test]
    fn slice_returns_everything_when_it_fits() {
        let spans = vec![span("abc"), span("def")];
        let out = slice_line(&spans, 0, 10);
        let joined: String = out.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(joined, "abcdef");
    }

    #[test]
    fn slice_clips_both_sides_and_keeps_styles() {
        let spans = vec![span("aaaa"), span("bb"), span("cccc")];
        // Window [3, 8): tail of first span, whole second, head of third.
        let out = slice_line(&spans, 3, 5);
        let joined: String = out.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(joined, "abbcc");
        assert!(out.iter().all(|s| s.style.fg == Some(Color::Cyan)));
    }

    #[test]
    fn slice_beyond_content_is_empty() {
        let spans = vec![span("ab")];
        assert!(slice_line(&spans, 5, 4).is_empty());
    }

    #[test]
    fn marquee_pings_pong_between_bounds_with_edge_pauses() {
        set_history_env();
        let mut app = App::new();
        let total: u16 = 200;
        let viewport: u16 = 100;
        let max = total - viewport;

        // Scroll right to the edge.
        for _ in 0..max + 5 {
            advance_footer_marquee(&mut app, total, viewport);
            app.footer_next_step = Some(Instant::now()); // force each step
            if app.footer_scroll == max {
                break;
            }
        }
        assert_eq!(app.footer_scroll, max);
        // One more step must flip direction and pause at the edge.
        app.footer_next_step = Some(Instant::now());
        advance_footer_marquee(&mut app, total, viewport);
        assert_eq!(app.footer_dir, -1);

        // And back down to zero, flipping again.
        for _ in 0..max + 5 {
            app.footer_next_step = Some(Instant::now());
            advance_footer_marquee(&mut app, total, viewport);
            if app.footer_scroll == 0 {
                break;
            }
        }
        assert_eq!(app.footer_scroll, 0);
        app.footer_next_step = Some(Instant::now());
        advance_footer_marquee(&mut app, total, viewport);
        assert_eq!(app.footer_dir, 1);
    }

    fn set_history_env() {
        // SAFETY: test-only; env is read once per history call in-process.
        unsafe {
            std::env::set_var(
                "SPEED_TEST_HISTORY_FILE",
                std::env::temp_dir().join("speed-test-ui-test.json"),
            );
        }
    }
}