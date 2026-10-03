//! ratatui front end.

use crate::command::{self, Command};
use crate::session::{Session, Stop};
use gb_core::{SCREEN_HEIGHT, SCREEN_WIDTH};
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;
use std::collections::VecDeque;
use std::time::Duration;

struct App {
    cursor: u16,
    running: bool,
    /// `Some` while the command prompt is open.
    input: Option<String>,
    log: VecDeque<String>,
    history: Vec<String>,
}

impl App {
    fn say(&mut self, line: impl Into<String>) {
        self.log.push_back(line.into());
        while self.log.len() > 50 {
            self.log.pop_front();
        }
    }
}

pub fn run(session: &mut Session) -> Result<(), String> {
    let mut app = App {
        cursor: session.pc(),
        running: false,
        input: None,
        log: VecDeque::new(),
        history: Vec::new(),
    };
    app.say("gbdbg ready: s step, n over, f frame, Space run, : command, ? help");
    let mut terminal = ratatui::init();
    let result = event_loop(&mut terminal, session, &mut app);
    ratatui::restore();
    result
}

fn event_loop(terminal: &mut ratatui::DefaultTerminal, s: &mut Session, app: &mut App) -> Result<(), String> {
    while !s.quit {
        terminal.draw(|f| draw(f, s, app)).map_err(|e| e.to_string())?;
        let wait = if app.running {
            Duration::ZERO
        } else {
            Duration::from_millis(100)
        };
        if event::poll(wait).map_err(|e| e.to_string())? {
            if let Event::Key(k) = event::read().map_err(|e| e.to_string())? {
                if k.kind != KeyEventKind::Press {
                    continue;
                }
                if k.modifiers.contains(KeyModifiers::CONTROL) && k.code == KeyCode::Char('c') {
                    break;
                }
                handle_key(k.code, s, app);
            }
        }
        if app.running {
            let stop = s.run_chunk();
            if stop != Stop::Cap {
                app.running = false;
                app.cursor = s.pc();
                let msg = s.describe(&stop);
                app.say(msg);
            }
        }
    }
    Ok(())
}

fn after_exec(s: &Session, app: &mut App, stop: Stop) {
    app.cursor = s.pc();
    let msg = s.describe(&stop);
    app.say(msg);
}

fn handle_key(code: KeyCode, s: &mut Session, app: &mut App) {
    if let Some(input) = &mut app.input {
        match code {
            KeyCode::Esc => app.input = None,
            KeyCode::Backspace => {
                input.pop();
            }
            KeyCode::Char(c) => input.push(c),
            KeyCode::Enter => {
                let line = app.input.take().unwrap_or_default();
                run_line(&line, s, app);
            }
            _ => {}
        }
        return;
    }
    if app.running {
        if matches!(code, KeyCode::Char(' ') | KeyCode::Char('c') | KeyCode::Esc) {
            app.running = false;
            app.cursor = s.pc();
            app.say("stopped by user");
        }
        return;
    }
    match code {
        KeyCode::Char('q') => s.quit = true,
        KeyCode::Char('s') => {
            let st = s.step();
            after_exec(s, app, st);
        }
        KeyCode::Char('n') => {
            let st = s.step_over();
            after_exec(s, app, st);
        }
        KeyCode::Char('f') => {
            let st = s.run_frame();
            after_exec(s, app, st);
        }
        KeyCode::Char('r') => {
            let st = s.run_to_ret();
            after_exec(s, app, st);
        }
        KeyCode::Char(' ') | KeyCode::Char('c') => {
            app.running = true;
            app.say("running...");
        }
        KeyCode::Char('b') => {
            let on = s.toggle_break(app.cursor);
            app.say(format!(
                "breakpoint {} at ${:04X}",
                if on { "set" } else { "cleared" },
                app.cursor
            ));
        }
        KeyCode::Char('w') => app.input = Some("watch ".into()),
        KeyCode::Char('g') => app.input = Some("goto ".into()),
        KeyCode::Char(':') => app.input = Some(String::new()),
        KeyCode::Char('?') => run_line("help", s, app),
        KeyCode::Down => app.cursor = app.cursor.wrapping_add(s.disasm(app.cursor).1 as u16),
        KeyCode::Up => app.cursor = s.prev_instruction(app.cursor),
        KeyCode::Home => app.cursor = s.pc(),
        KeyCode::PageDown => s.mem_addr = s.mem_addr.wrapping_add(0x100),
        KeyCode::PageUp => s.mem_addr = s.mem_addr.wrapping_sub(0x100),
        _ => {}
    }
}

fn run_line(line: &str, s: &mut Session, app: &mut App) {
    match command::parse(line) {
        Ok(None) => {}
        Ok(Some(cmd)) => {
            app.history.push(line.to_string());
            let moves = matches!(
                cmd,
                Command::Step(_)
                    | Command::Next
                    | Command::Frame
                    | Command::Continue(_)
                    | Command::Ret
                    | Command::Set(..)
            );
            if matches!(cmd, Command::Continue(None)) {
                app.running = true;
                app.say("running...");
                return;
            }
            for l in s.run_command(cmd) {
                app.say(l);
            }
            if moves {
                app.cursor = s.pc();
            }
        }
        Err(e) => app.say(format!("error: {e}")),
    }
}

fn block(title: &str) -> Block<'_> {
    Block::default().borders(Borders::ALL).title(title)
}

fn draw(f: &mut Frame, s: &Session, app: &App) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(10), Constraint::Length(1)])
        .split(f.area());
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(40), Constraint::Length(62)])
        .split(rows[0]);
    let left = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage(45),
            Constraint::Percentage(30),
            Constraint::Percentage(25),
        ])
        .split(cols[0]);
    let right = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(6), Constraint::Min(6), Constraint::Length(5)])
        .split(cols[1]);

    draw_disasm(f, left[0], s, app);
    draw_memory(f, left[1], s);
    let log: Vec<Line> = {
        let h = left[2].height.saturating_sub(2) as usize;
        let skip = app.log.len().saturating_sub(h);
        app.log.iter().skip(skip).map(|l| Line::raw(l.as_str())).collect()
    };
    f.render_widget(Paragraph::new(log).block(block("Log")), left[2]);

    let regs: Vec<Line> = s.regs_lines().into_iter().map(Line::raw).collect();
    f.render_widget(
        Paragraph::new(regs).block(block(if s.tracing() { "Registers [TRACE]" } else { "Registers" })),
        right[0],
    );
    draw_screen(f, right[1], s);
    let text = s.serial_text();
    let h = right[2].height.saturating_sub(2) as usize;
    let lines: Vec<&str> = text.lines().collect();
    let tail: Vec<Line> = lines[lines.len().saturating_sub(h)..]
        .iter()
        .map(|l| Line::raw(*l))
        .collect();
    f.render_widget(Paragraph::new(tail).block(block("Serial")), right[2]);

    let status = match &app.input {
        Some(i) => Line::from(vec![
            Span::styled(":", Style::new().fg(Color::Yellow)),
            Span::raw(i.as_str()),
            Span::raw("_"),
        ]),
        None if app.running => Line::styled("RUNNING (Space to stop)", Style::new().fg(Color::Green)),
        None => Line::styled(
            "s step  n over  f frame  r ret  Space run  b bp  w watch  g goto  : cmd  ? help  q quit",
            Style::new().add_modifier(Modifier::DIM),
        ),
    };
    f.render_widget(Paragraph::new(status), rows[1]);
}

fn draw_disasm(f: &mut Frame, area: Rect, s: &Session, app: &App) {
    let h = area.height.saturating_sub(2) as usize;
    let mut addr = s.disasm_start(app.cursor, h / 3);
    let mut lines = Vec::new();
    for _ in 0..h {
        let mut style = Style::new();
        if addr == s.pc() {
            style = style.bg(Color::Blue).add_modifier(Modifier::BOLD);
        }
        if addr == app.cursor {
            style = style.add_modifier(Modifier::UNDERLINED);
        }
        if s.breaks.contains(&addr) {
            style = style.fg(Color::Red);
        }
        lines.push(Line::styled(s.disasm_line(addr), style));
        addr = addr.wrapping_add(s.disasm(addr).1 as u16);
    }
    f.render_widget(
        Paragraph::new(lines).block(block("Disassembly (* breakpoint, > PC)")),
        area,
    );
}

fn draw_memory(f: &mut Frame, area: Rect, s: &Session) {
    let h = area.height.saturating_sub(2);
    let lines: Vec<Line> = s.hex_lines(s.mem_addr, h * 16).into_iter().map(Line::raw).collect();
    let title = if s.watches.is_empty() {
        "Memory (PgUp/PgDn, g goto)".to_string()
    } else {
        let w: Vec<String> = s.watches.keys().map(|a| format!("${a:04X}")).collect();
        format!("Memory (watching {})", w.join(" "))
    };
    f.render_widget(Paragraph::new(lines).block(block(&title)), area);
}

/// Half-block preview, box-averaged down to fit the pane.
fn draw_screen(f: &mut Frame, area: Rect, s: &Session) {
    let inner = block("Screen").inner(area);
    f.render_widget(block("Screen"), area);
    let fb = s.gb.bus.ppu.framebuffer();
    let cols = (inner.width as usize).min(SCREEN_WIDTH);
    if cols == 0 || inner.height == 0 {
        return;
    }
    let mut pix_rows = (cols * SCREEN_HEIGHT).div_ceil(SCREEN_WIDTH);
    let max_rows = inner.height as usize * 2;
    let cols = if pix_rows > max_rows {
        pix_rows = max_rows;
        (pix_rows * SCREEN_WIDTH / SCREEN_HEIGHT).max(1)
    } else {
        cols
    };
    let pix_rows = pix_rows + pix_rows % 2;
    let sample = |cx: usize, py: usize| -> Color {
        let (x0, x1) = (
            cx * SCREEN_WIDTH / cols,
            ((cx + 1) * SCREEN_WIDTH / cols).max(cx * SCREEN_WIDTH / cols + 1),
        );
        let (y0, y1) = (
            py * SCREEN_HEIGHT / pix_rows,
            ((py + 1) * SCREEN_HEIGHT / pix_rows).max(py * SCREEN_HEIGHT / pix_rows + 1),
        );
        let (mut sum, mut n) = (0u32, 0u32);
        for y in y0..y1.min(SCREEN_HEIGHT) {
            for x in x0..x1.min(SCREEN_WIDTH) {
                sum += fb[y * SCREEN_WIDTH + x] & 0xFF;
                n += 1;
            }
        }
        let g = (sum / n.max(1)) as u8;
        Color::Rgb(g, g, g)
    };
    let x_off = (inner.width as usize - cols) / 2;
    let lines: Vec<Line> = (0..pix_rows / 2)
        .map(|row| {
            let mut spans = vec![Span::raw(" ".repeat(x_off))];
            spans.extend(
                (0..cols).map(|cx| Span::styled("▀", Style::new().fg(sample(cx, row * 2)).bg(sample(cx, row * 2 + 1)))),
            );
            Line::from(spans)
        })
        .collect();
    f.render_widget(Paragraph::new(lines), inner);
}
