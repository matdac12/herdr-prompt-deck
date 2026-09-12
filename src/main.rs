use std::env;
use std::io;
use std::process::Command;

use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::{Frame, Terminal};

fn herdr() -> String {
    env::var("HERDR_BIN_PATH").unwrap_or_else(|_| "herdr".to_string())
}

fn send_text(pane: &str, text: &str) {
    match Command::new(herdr())
        .args(["pane", "send-text", pane, text])
        .status()
    {
        Ok(status) if status.success() => {}
        Ok(status) => eprintln!("prompt-deck: send-text exited {status}"),
        Err(err) => eprintln!("prompt-deck: send-text failed: {err}"),
    }
}

fn close_pane(pane: &str) {
    let _ = Command::new(herdr()).args(["pane", "close", pane]).status();
}

/// Cwd of a pane, so the file dialog opens near the agent's project.
fn pane_cwd(pane_id: &str) -> Option<String> {
    let output = Command::new(herdr()).args(["pane", "list"]).output().ok()?;
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).ok()?;
    let panes = json.get("result")?.get("panes")?.as_array()?;
    panes.iter().find_map(|p| {
        if p.get("pane_id").and_then(|v| v.as_str()) == Some(pane_id) {
            p.get("cwd").and_then(|v| v.as_str()).map(str::to_string)
        } else {
            None
        }
    })
}

/// Native OS file dialog; returns the chosen file's absolute path.
fn pick_file(start_dir: Option<&str>) -> Option<String> {
    let mut dialog = rfd::FileDialog::new().set_title("Select a file to insert");
    if let Some(dir) = start_dir {
        dialog = dialog.set_directory(dir);
    }
    dialog
        .pick_file()
        .map(|path| path.to_string_lossy().into_owned())
}

fn parse_target(args: &[String]) -> Option<String> {
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if arg == "--target" {
            return iter.next().cloned();
        }
    }
    None
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Mode {
    Files,
    Snippets,
    Editor,
}

impl Mode {
    const ALL: [Mode; 3] = [Mode::Files, Mode::Snippets, Mode::Editor];

    fn title(self) -> &'static str {
        match self {
            Mode::Files => "Files",
            Mode::Snippets => "Snippets",
            Mode::Editor => "Editor",
        }
    }

    fn index(self) -> usize {
        Self::ALL.iter().position(|m| *m == self).unwrap_or(0)
    }

    fn from_index(i: usize) -> Mode {
        Self::ALL[i % Self::ALL.len()]
    }
}

struct App {
    target: String,
    mode: Mode,
    recent: Vec<String>,
    status: Option<String>,
    should_quit: bool,
}

impl App {
    fn new(target: String) -> Self {
        App {
            target,
            mode: Mode::Files,
            recent: Vec::new(),
            status: None,
            should_quit: false,
        }
    }

    fn browse(&mut self) {
        if self.target.is_empty() {
            self.status = Some("no target pane".to_string());
            return;
        }
        let cwd = pane_cwd(&self.target);
        if let Some(path) = pick_file(cwd.as_deref()) {
            send_text(&self.target, &format!("{path} "));
            self.status = Some(format!("inserted {path}"));
            self.recent.retain(|p| p != &path);
            self.recent.insert(0, path);
            self.recent.truncate(5);
        }
    }

    fn on_key(&mut self, code: KeyCode) {
        match code {
            KeyCode::Char('1') => self.mode = Mode::from_index(0),
            KeyCode::Char('2') => self.mode = Mode::from_index(1),
            KeyCode::Char('3') => self.mode = Mode::from_index(2),
            KeyCode::Tab => self.mode = Mode::from_index(self.mode.index() + 1),
            KeyCode::BackTab => {
                self.mode = Mode::from_index(self.mode.index() + Mode::ALL.len() - 1)
            }
            KeyCode::Char('b') if self.mode == Mode::Files => self.browse(),
            KeyCode::Esc | KeyCode::Char('q') => self.should_quit = true,
            _ => {}
        }
    }
}

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    match args.first().map(String::as_str).unwrap_or("pane") {
        "pane" => {
            let target = parse_target(&args).unwrap_or_default();
            let mut app = App::new(target);
            if let Err(err) = run_pane(&mut app) {
                eprintln!("prompt-deck: {err}");
            }
        }
        other => eprintln!("prompt-deck: unknown mode '{other}'"),
    }
}

fn run_pane(app: &mut App) -> io::Result<()> {
    let self_pane = env::var("HERDR_PANE_ID").unwrap_or_default();

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout))?;

    let result = run_loop(&mut terminal, app);
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    result?;

    if !self_pane.is_empty() {
        close_pane(&self_pane);
    }
    Ok(())
}

fn run_loop(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>, app: &mut App) -> io::Result<()> {
    loop {
        terminal.draw(|frame| ui(frame, app))?;
        if let Event::Key(key) = event::read()? {
            if key.kind == KeyEventKind::Press {
                app.on_key(key.code);
            }
        }
        if app.should_quit {
            return Ok(());
        }
    }
}

fn ui(frame: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(frame.area());

    frame.render_widget(mode_bar(app, chunks[0].width), chunks[0]);
    frame.render_widget(Paragraph::new(content(app)), chunks[1]);
    frame.render_widget(footer(app, chunks[2].width), chunks[2]);
}

fn mode_bar(app: &App, width: u16) -> Paragraph<'static> {
    let mut spans = vec![Span::styled(
        " Prompt Deck ",
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
    )];

    for (i, mode) in Mode::ALL.iter().enumerate() {
        let style = if *mode == app.mode {
            Style::default()
                .fg(Color::Black)
                .bg(Color::Yellow)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        spans.push(Span::styled(format!(" {} {} ", i + 1, mode.title()), style));
        spans.push(Span::raw(" "));
    }

    let target = format!(" target: {} ", app.target);
    let used: usize = spans.iter().map(|s| s.content.chars().count()).sum();
    let work = width as usize;
    if used + target.len() < work {
        spans.push(Span::raw(" ".repeat(work - used - target.len())));
    }
    spans.push(Span::styled(target, Style::default().fg(Color::Cyan)));

    Paragraph::new(Line::from(spans))
}

fn content(app: &App) -> Vec<Line<'static>> {
    match app.mode {
        Mode::Files => {
            let mut lines = vec![
                Line::from(Span::styled(
                    "When you need a file in the prompt",
                    Style::default().fg(Color::Gray),
                )),
                Line::from(vec![
                    Span::styled(
                        "  b",
                        Style::default()
                            .fg(Color::Yellow)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::raw("  open the native file dialog and insert its path"),
                ]),
            ];
            if !app.recent.is_empty() {
                lines.push(Line::from(Span::styled(
                    "  recently inserted",
                    Style::default().fg(Color::DarkGray),
                )));
                for path in &app.recent {
                    lines.push(Line::from(Span::styled(
                        format!("    {path}"),
                        Style::default().fg(Color::DarkGray),
                    )));
                }
            }
            lines
        }
        Mode::Snippets => vec![
            Line::from(Span::styled(
                "Snippets",
                Style::default().add_modifier(Modifier::BOLD),
            )),
            Line::from(Span::styled(
                "  coming next: snippets.toml list, new / edit / delete",
                Style::default().fg(Color::DarkGray),
            )),
        ],
        Mode::Editor => vec![
            Line::from(Span::styled(
                "Editor",
                Style::default().add_modifier(Modifier::BOLD),
            )),
            Line::from(Span::styled(
                "  coming next: compose text and insert it",
                Style::default().fg(Color::DarkGray),
            )),
        ],
    }
}

fn footer(app: &App, width: u16) -> Paragraph<'static> {
    let key = |k: &'static str, label: &'static str| {
        vec![
            Span::styled(
                format!(" {k} "),
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!("{label}  "), Style::default().fg(Color::DarkGray)),
        ]
    };

    let mut spans = Vec::new();
    if app.mode == Mode::Files {
        spans.extend(key("b", "browse"));
    }
    spans.extend(key("1/2/3", "tools"));
    spans.extend(key("tab", "next"));
    spans.extend(key("esc", "close"));

    if let Some(status) = &app.status {
        let text = format!("  {status} ");
        let used: usize = spans.iter().map(|s| s.content.chars().count()).sum();
        let work = width as usize;
        if used + text.len() < work {
            spans.push(Span::raw(" ".repeat(work - used - text.len())));
        }
        spans.push(Span::styled(text, Style::default().fg(Color::Green)));
    }

    Paragraph::new(Line::from(spans))
}
