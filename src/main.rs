mod gpu;

use std::io;
use std::time::{Duration, Instant};

use crossterm::{
    event::{self, Event, KeyCode},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, Gauge, Paragraph, Row, Table};
use ratatui::Terminal;

const GREEN: Color = Color::Rgb(0, 255, 200);
const DIM_GREEN: Color = Color::Rgb(0, 128, 100);
const YELLOW: Color = Color::Rgb(255, 255, 0);
const RED: Color = Color::Rgb(255, 60, 60);

fn main() -> anyhow::Result<()> {
    let gpu_info = gpu::find_gpu()?;

    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
        original_hook(info);
    }));

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = run(&mut terminal, &gpu_info);

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    if let Err(e) = result {
        eprintln!("Error: {}", e);
        std::process::exit(1);
    }
    Ok(())
}

fn run(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    gpu_info: &gpu::GpuInfo,
) -> anyhow::Result<()> {
    let tick = Duration::from_millis(500);
    let mut last_tick = Instant::now();

    loop {
        let stats = gpu::read_stats(gpu_info);
        let procs = gpu::read_process_gpu_mem();

        terminal.draw(|f| {
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(3),
                    Constraint::Length(7),
                    Constraint::Min(8),
                    Constraint::Length(1),
                ])
                .split(f.area());

            draw_header(f, chunks[0], gpu_info, &stats);
            draw_gauges(f, chunks[1], gpu_info, &stats);
            draw_processes(f, chunks[2], &procs);
            draw_footer(f, chunks[3]);
        })?;

        let timeout = tick.saturating_sub(last_tick.elapsed());
        if event::poll(timeout)? {
            if let Event::Key(key) = event::read()? {
                if matches!(key.code, KeyCode::Char('q') | KeyCode::Esc) {
                    return Ok(());
                }
            }
        }

        if last_tick.elapsed() >= tick {
            last_tick = Instant::now();
        }
    }
}

fn draw_header(f: &mut ratatui::Frame, area: Rect, info: &gpu::GpuInfo, stats: &gpu::GpuStats) {
    let temp_c = stats.temp_millicelsius as f64 / 1000.0;
    let title = format!(
        " {} │ {} │ {:.0}°C │ {}MHz / {}MHz ",
        info.name, info.driver_version, temp_c, stats.sclk_mhz, stats.mclk_mhz
    );
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(GREEN))
        .title(Span::styled("vram-map", Style::default().fg(GREEN).add_modifier(Modifier::BOLD)));
    let para = Paragraph::new(Line::from(Span::styled(title, Style::default().fg(GREEN))))
        .block(block);
    f.render_widget(para, area);
}

fn draw_gauges(f: &mut ratatui::Frame, area: Rect, info: &gpu::GpuInfo, stats: &gpu::GpuStats) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(DIM_GREEN))
        .title(Span::styled("Memory", Style::default().fg(GREEN)));

    let inner = block.inner(area);
    f.render_widget(block, area);

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(inner);

    let vram_pct = if info.vram_total > 0 {
        (stats.vram_used as f64 / info.vram_total as f64 * 100.0) as u16
    } else { 0 };
    let gtt_pct = if info.gtt_total > 0 {
        (stats.gtt_used as f64 / info.gtt_total as f64 * 100.0) as u16
    } else { 0 };

    let vram_label = format!(
        "VRAM: {} / {} ({:.0}%)",
        fmt_bytes(stats.vram_used), fmt_bytes(info.vram_total), vram_pct
    );
    let gtt_label = format!(
        "GTT:  {} / {} ({:.0}%)",
        fmt_bytes(stats.gtt_used), fmt_bytes(info.gtt_total), gtt_pct
    );

    let vram_gauge = Gauge::default()
        .gauge_style(Style::default().fg(bar_color(vram_pct)).bg(Color::Black))
        .ratio(vram_pct.min(100) as f64 / 100.0)
        .label(Span::styled(vram_label, Style::default().fg(GREEN)));
    f.render_widget(vram_gauge, rows[0]);

    let gtt_gauge = Gauge::default()
        .gauge_style(Style::default().fg(bar_color(gtt_pct)).bg(Color::Black))
        .ratio(gtt_pct.min(100) as f64 / 100.0)
        .label(Span::styled(gtt_label, Style::default().fg(GREEN)));
    f.render_widget(gtt_gauge, rows[1]);

    let gpu_label = format!("GPU:  {}%", stats.gpu_busy_pct);
    let gpu_gauge = Gauge::default()
        .gauge_style(Style::default().fg(bar_color(stats.gpu_busy_pct as u16)).bg(Color::Black))
        .ratio(stats.gpu_busy_pct.min(100) as f64 / 100.0)
        .label(Span::styled(gpu_label, Style::default().fg(GREEN)));
    f.render_widget(gpu_gauge, rows[2]);

    let combined_used = stats.vram_used + stats.gtt_used;
    let combined_total = info.vram_total + info.gtt_total;
    let combined_pct = if combined_total > 0 {
        (combined_used as f64 / combined_total as f64 * 100.0) as u16
    } else { 0 };
    let combined_label = format!(
        "UMA:  {} / {} ({:.0}%)",
        fmt_bytes(combined_used), fmt_bytes(combined_total), combined_pct
    );
    let combined_gauge = Gauge::default()
        .gauge_style(Style::default().fg(bar_color(combined_pct)).bg(Color::Black))
        .ratio(combined_pct.min(100) as f64 / 100.0)
        .label(Span::styled(combined_label, Style::default().fg(GREEN)));
    f.render_widget(combined_gauge, rows[3]);
}

fn draw_processes(f: &mut ratatui::Frame, area: Rect, procs: &[gpu::ProcessGpuMem]) {
    let header = Row::new(vec![
        Cell::from("PID").style(Style::default().fg(GREEN).add_modifier(Modifier::BOLD)),
        Cell::from("Process").style(Style::default().fg(GREEN).add_modifier(Modifier::BOLD)),
        Cell::from("VRAM").style(Style::default().fg(GREEN).add_modifier(Modifier::BOLD)),
        Cell::from("GTT").style(Style::default().fg(GREEN).add_modifier(Modifier::BOLD)),
        Cell::from("Total").style(Style::default().fg(GREEN).add_modifier(Modifier::BOLD)),
    ]);

    let rows: Vec<Row> = procs.iter().map(|p| {
        Row::new(vec![
            Cell::from(p.pid.to_string()).style(Style::default().fg(DIM_GREEN)),
            Cell::from(p.name.clone()).style(Style::default().fg(GREEN)),
            Cell::from(fmt_kib(p.vram_kib)).style(Style::default().fg(Color::Rgb(100, 200, 255))),
            Cell::from(fmt_kib(p.gtt_kib)).style(Style::default().fg(Color::Rgb(200, 200, 100))),
            Cell::from(fmt_kib(p.total_kib())).style(Style::default().fg(GREEN).add_modifier(Modifier::BOLD)),
        ])
    }).collect();

    let table = Table::new(
        rows,
        [
            Constraint::Length(8),
            Constraint::Min(20),
            Constraint::Length(12),
            Constraint::Length(12),
            Constraint::Length(12),
        ],
    )
    .header(header)
    .block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(DIM_GREEN))
            .title(Span::styled(
                format!("Processes ({})", procs.len()),
                Style::default().fg(GREEN),
            )),
    );
    f.render_widget(table, area);
}

fn draw_footer(f: &mut ratatui::Frame, area: Rect) {
    let footer = Paragraph::new(Line::from(vec![
        Span::styled(" q", Style::default().fg(GREEN).add_modifier(Modifier::BOLD)),
        Span::styled(" quit  ", Style::default().fg(DIM_GREEN)),
        Span::styled("refresh: 500ms", Style::default().fg(DIM_GREEN)),
    ]));
    f.render_widget(footer, area);
}

fn bar_color(pct: u16) -> Color {
    if pct > 90 { RED } else if pct > 70 { YELLOW } else { GREEN }
}

fn fmt_bytes(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{} B", bytes)
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KiB", bytes as f64 / 1024.0)
    } else if bytes < 1024 * 1024 * 1024 {
        format!("{:.1} MiB", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.2} GiB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
    }
}

fn fmt_kib(kib: u64) -> String {
    if kib < 1024 {
        format!("{} KiB", kib)
    } else if kib < 1024 * 1024 {
        format!("{:.1} MiB", kib as f64 / 1024.0)
    } else {
        format!("{:.2} GiB", kib as f64 / (1024.0 * 1024.0))
    }
}
