use ratatui::{
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Cell, Clear, Paragraph, Row, Sparkline, Table},
    Frame,
};

use crate::app::{App, Entry, SortKey};
use crate::human;

const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

pub fn draw(f: &mut Frame, app: &mut App) {
    let area = f.area();
    if area.width < 70 || area.height < 12 {
        let msg = Paragraph::new("terminal too small (need at least 70x12)")
            .alignment(Alignment::Center);
        f.render_widget(msg, area);
        return;
    }
    let [header, table, details, footer] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(3),
        Constraint::Length(7),
        Constraint::Length(1),
    ])
    .areas(area);
    draw_header(f, header, app);
    draw_table(f, table, app);
    draw_details(f, details, app);
    draw_footer(f, footer, app);
    if app.help {
        draw_help(f, area);
    }
}

fn draw_header(f: &mut Frame, area: Rect, app: &App) {
    let shown = app.entries.len();
    let running = app.entries.iter().filter(|e| e.running()).count();
    let live: Vec<&Entry> = app.entries.iter().filter(|e| e.running()).collect();
    let total_cpu: f64 = live
        .iter()
        .filter_map(|e| e.snap.as_ref().map(|s| s.cpu_percent))
        .sum();
    // an empty f64 sum is -0.0, which would render as "-0.0%"
    let total_cpu = if total_cpu == 0.0 { 0.0 } else { total_cpu };
    let (mu, ml) = live
        .iter()
        .filter_map(|e| e.snap.as_ref().map(|s| (s.mem_usage, s.mem_limit)))
        .fold((0u64, Vec::new()), |(used, mut limits), (u, l)| {
            limits.push(l);
            (used + u, limits)
        });
    let (ml, mem_pct) = human::cluster_mem(mu, &ml);
    let spinner = SPINNER[((app.frame / 4) % SPINNER.len() as u64) as usize];

    let mut spans = vec![
        Span::styled(format!("{spinner} "), Style::new().fg(Color::Cyan)),
        Span::styled(
            format!("{shown} containers · {running} running"),
            Style::new().add_modifier(Modifier::BOLD),
        ),
        Span::raw("   "),
        Span::styled(format!("CPU {:.1}%", total_cpu), Style::new().fg(heat(total_cpu))),
        Span::raw("   "),
        Span::styled(
            format!("MEM {} / {} ({:.0}%)", human::bytes(mu), human::bytes(ml), mem_pct),
            Style::new().fg(Color::Blue),
        ),
    ];
    if app.paused {
        spans.push(Span::styled("   ⏸ PAUSED", Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD)));
    }
    if !app.daemon_ok {
        spans.push(Span::styled("   ⚠ daemon unreachable", Style::new().fg(Color::Red)));
    }

    let block = Block::bordered()
        .title(Span::styled(
            " DockerDash ",
            Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        ))
        .title(
            Line::from(format!(" engine {} ", app.engine))
                .right_aligned()
                .style(Style::new().fg(Color::DarkGray)),
        );
    let p = Paragraph::new(Line::from(spans)).block(block);
    f.render_widget(p, area);
}

fn draw_table(f: &mut Frame, area: Rect, app: &mut App) {
    let sel = app.selected_index();
    let arrow = |key: SortKey| {
        if app.sort == key {
            if key.ascending() { " ↑" } else { " ↓" }
        } else {
            ""
        }
    };
    let headers = vec![
        format!("NAME{}", arrow(SortKey::Name)),
        "ID".to_string(),
        "IMAGE".to_string(),
        "STATUS".to_string(),
        format!("CPU %{}", arrow(SortKey::Cpu)),
        "MEM USAGE / LIMIT".to_string(),
        format!("MEM %{}", arrow(SortKey::Mem)),
        format!("NET I/O{}", arrow(SortKey::Net)),
        "BLOCK I/O".to_string(),
        format!("PIDS{}", arrow(SortKey::Pids)),
    ];

    let rows: Vec<Row> = app
        .entries
        .iter()
        .enumerate()
        .map(|(i, e)| row_for(e, i == sel))
        .collect();

    let widths = [
        Constraint::Min(14),
        Constraint::Length(12),
        Constraint::Min(10),
        Constraint::Min(10),
        Constraint::Length(9),
        Constraint::Length(17),
        Constraint::Length(6),
        Constraint::Length(13),
        Constraint::Length(13),
        Constraint::Length(4),
    ];
    let title = format!(
        " containers ({}) — sorted by {} {} ",
        app.entries.len(),
        app.sort.label(),
        if app.sort.ascending() { "↑" } else { "↓" }
    );
    let table = Table::new(rows, widths)
        .header(Row::new(headers).style(Style::new().add_modifier(Modifier::BOLD)))
        .column_spacing(1)
        .block(Block::bordered().title(title));

    app.table_state.select(if app.entries.is_empty() { None } else { Some(sel) });
    f.render_stateful_widget(table, area, &mut app.table_state);

    if app.entries.is_empty() {
        let msg = Paragraph::new(format!(
            "no {} containers — press 'a' to include stopped ones",
            if app.show_all { "matching" } else { "running" }
        ))
        .style(Style::new().fg(Color::Yellow))
        .alignment(Alignment::Center);
        let [_, mid, _] = Layout::vertical([
            Constraint::Fill(1),
            Constraint::Length(1),
            Constraint::Fill(1),
        ])
        .areas(area);
        f.render_widget(msg, mid);
    }
}

fn row_for(e: &Entry, selected: bool) -> Row<'static> {
    let placeholder = |running: bool| {
        if running {
            Cell::from("…".to_string()).style(Style::new().fg(Color::DarkGray))
        } else {
            Cell::from("—".to_string()).style(Style::new().fg(Color::DarkGray))
        }
    };
    let (cpu_cell, mem_cell, mem_pct_cell, net_cell, block_cell, pids_cell) = match &e.snap {
        Some(s) => (
            Cell::from(format!("{:.2}", s.cpu_percent)).style(Style::new().fg(heat(s.cpu_percent))),
            Cell::from(format!("{} / {}", human::bytes(s.mem_usage), human::bytes(s.mem_limit))),
            Cell::from(format!("{:.1}", s.mem_percent)).style(Style::new().fg(heat_mem(s.mem_percent))),
            Cell::from(format!("{} / {}", human::bytes(s.net_rx), human::bytes(s.net_tx))),
            Cell::from(format!("{} / {}", human::bytes(s.block_read), human::bytes(s.block_write))),
            Cell::from(s.pids.to_string()),
        ),
        None => (placeholder(e.running()), placeholder(e.running()), placeholder(e.running()), placeholder(e.running()), placeholder(e.running()), placeholder(e.running())),
    };
    Row::new(vec![
        Cell::from(e.info.name.clone()),
        Cell::from(e.info.id.chars().take(12).collect::<String>()).style(Style::new().fg(Color::DarkGray)),
        Cell::from(e.info.image.clone()).style(Style::new().fg(Color::DarkGray)),
        Cell::from(e.info.status.clone()).style(Style::new().fg(state_color(&e.info.state))),
        cpu_cell,
        mem_cell,
        mem_pct_cell,
        net_cell,
        block_cell,
        pids_cell,
    ])
    .style(if selected {
        Style::new().bg(Color::DarkGray)
    } else {
        Style::new()
    })
}

fn draw_details(f: &mut Frame, area: Rect, app: &App) {
    let Some(e) = app.selected_entry() else {
        let p = Paragraph::new("")
            .block(Block::bordered().title(" details ").style(Style::new().fg(Color::DarkGray)));
        f.render_widget(p, area);
        return;
    };
    let id12: String = e.info.id.chars().take(12).collect();
    let block = Block::bordered()
        .title(format!(" {} ", e.info.name))
        .title(
            Line::from(format!(" {id12} "))
                .right_aligned()
                .style(Style::new().fg(Color::DarkGray)),
        );
    let inner = block.inner(area);
    f.render_widget(block, area);
    let [left, right] =
        Layout::horizontal([Constraint::Percentage(55), Constraint::Percentage(45)]).areas(inner);

    let dim = Style::new().fg(Color::DarkGray);
    let mut lines = vec![
        Line::from(vec![Span::styled("image    ", dim), Span::raw(&e.info.image)]),
        Line::from(vec![
            Span::styled("state    ", dim),
            Span::styled(e.info.state.clone(), Style::new().fg(state_color(&e.info.state))),
            Span::raw(format!("  ({})", e.info.status)),
        ]),
        Line::from(vec![Span::styled("created  ", dim), Span::raw(format!("{} ago", human::ago(e.info.created)))]),
        Line::from(vec![
            Span::styled("ports    ", dim),
            Span::raw(if e.info.ports.is_empty() {
                "—".to_string()
            } else {
                e.info.ports.join(", ")
            }),
        ]),
    ];
    if let Some(s) = &e.snap {
        lines.push(Line::from(vec![
            Span::styled("io       ", dim),
            Span::raw(format!(
                "net ↓{} ↑{}  ·  blk r{} w{}  ·  pids {}",
                human::bytes(s.net_rx),
                human::bytes(s.net_tx),
                human::bytes(s.block_read),
                human::bytes(s.block_write),
                s.pids
            )),
        ]));
    }

    f.render_widget(Paragraph::new(lines), left);
    let spark = Sparkline::default()
        .data(&e.history)
        .max(e.spark_max())
        .style(Style::new().fg(Color::Cyan))
        .block(Block::bordered().title(" CPU % (recent) "));
    f.render_widget(spark, right);
}

fn draw_footer(f: &mut Frame, area: Rect, app: &App) {
    let line = match &app.message {
        Some(msg) => Line::from(Span::styled(
            format!(" ⚠ {msg}  (retrying… press 'r' to force) "),
            Style::new().fg(Color::Red),
        )),
        None => Line::from(Span::styled(
            " ↑/↓ select · c/m/n/p/u sort · a all · space pause · r refresh · ? help · q quit ",
            Style::new().fg(Color::DarkGray),
        )),
    };
    f.render_widget(Paragraph::new(line), area);
}

fn draw_help(f: &mut Frame, area: Rect) {
    let popup = centered_rect(area, 50, 60);
    f.render_widget(Clear, popup);
    let dim = Style::new().fg(Color::DarkGray);
    let key = Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD);
    let mk = |k: &str, d: &str| {
        Line::from(vec![
            Span::styled(format!("{k:<12}"), key),
            Span::raw(d.to_string()),
        ])
    };
    let lines = vec![
        mk("q / Esc", "quit"),
        mk("j / ↓", "select next container"),
        mk("k / ↑", "select previous container"),
        mk("g / G", "jump to top / bottom"),
        mk("PgUp/PgDn", "page selection"),
        mk("c", "sort by CPU %"),
        mk("m", "sort by memory usage"),
        mk("n", "sort by name"),
        mk("p", "sort by PIDs"),
        mk("u", "sort by network I/O"),
        mk("a", "toggle stopped containers"),
        mk("space", "pause / resume live updates"),
        mk("r", "force refresh now"),
        Line::from(""),
        Line::from(vec![Span::styled("stats source: ", dim), Span::raw("Docker Engine streaming /stats API")]),
    ];
    let p = Paragraph::new(lines).block(
        Block::bordered()
            .title(" keys ")
            .title(Line::from(" any key closes ").right_aligned().style(dim)),
    );
    f.render_widget(p, popup);
}

fn centered_rect(area: Rect, pct_y: u16, pct_x: u16) -> Rect {
    let vertical = Layout::vertical([
        Constraint::Percentage((100 - pct_y) / 2),
        Constraint::Percentage(pct_y),
        Constraint::Percentage((100 - pct_y) / 2),
    ])
    .split(area);
    Layout::horizontal([
        Constraint::Percentage((100 - pct_x) / 2),
        Constraint::Percentage(pct_x),
        Constraint::Percentage((100 - pct_x) / 2),
    ])
    .split(vertical[1])[1]
}

fn heat(v: f64) -> Color {
    if v < 50.0 {
        Color::Green
    } else if v < 80.0 {
        Color::Yellow
    } else {
        Color::Red
    }
}

fn heat_mem(v: f64) -> Color {
    if v < 60.0 {
        Color::Green
    } else if v < 85.0 {
        Color::Yellow
    } else {
        Color::Red
    }
}

fn state_color(state: &str) -> Color {
    match state {
        "running" => Color::Green,
        "paused" => Color::Yellow,
        "restarting" => Color::Magenta,
        "exited" | "dead" => Color::Red,
        "created" => Color::Blue,
        _ => Color::Reset,
    }
}
