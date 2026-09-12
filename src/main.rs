use std::env;
use std::io;
use std::path::PathBuf;
use std::process::Command;

use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event::{
    self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers,
};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::{Frame, Terminal};
use serde::{Deserialize, Serialize};

const SNIPPETS_HEADER: &str = "\
# Prompt Deck snippets — hand-editable.
# Each entry is a [[snippet]] with a name and text. Prompt Deck rewrites this
# file when you add, edit, or delete from the Snippets tab.
";

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
#[cfg(any(windows, target_os = "macos"))]
fn pick_file(start_dir: Option<&str>) -> Option<String> {
    let mut dialog = rfd::FileDialog::new().set_title("Select a file to insert");
    if let Some(dir) = start_dir {
        dialog = dialog.set_directory(dir);
    }
    dialog
        .pick_file()
        .map(|path| path.to_string_lossy().into_owned())
}

/// Linux has no supported in-process dialog; use `zenity` (GNOME) or `kdialog` (KDE).
/// Keeps the binary free of gtk/wayland build dependencies.
#[cfg(all(unix, not(target_os = "macos")))]
fn pick_file(start_dir: Option<&str>) -> Option<String> {
    use std::process::Stdio;

    // `None` = command not installed; `Some(None)` = cancelled; `Some(Some(p))` = chosen.
    fn run(cmd: &str, args: &[String]) -> Option<Option<String>> {
        match Command::new(cmd)
            .args(args)
            .stderr(Stdio::null())
            .output()
        {
            Err(_) => None,
            Ok(out) => {
                if out.status.success() {
                    let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
                    Some(if path.is_empty() { None } else { Some(path) })
                } else {
                    Some(None)
                }
            }
        }
    }

    let mut zenity = vec!["--file-selection".to_string()];
    zenity.push("--title=Select a file to insert".to_string());
    if let Some(dir) = start_dir.filter(|d| !d.is_empty()) {
        zenity.push(format!("--filename={dir}"));
    }
    if let Some(result) = run("zenity", &zenity) {
        return result;
    }

    let kdialog = vec![
        "--getopenfilename".to_string(),
        start_dir.unwrap_or("").to_string(),
    ];
    if let Some(result) = run("kdialog", &kdialog) {
        return result;
    }

    eprintln!("prompt-deck: no native file dialog found (install zenity or kdialog)");
    None
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

// ---------------------------------------------------------------------------
// Snippets storage
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Clone, Default)]
struct Snippet {
    name: String,
    #[serde(default)]
    text: String,
}

#[derive(Serialize, Deserialize, Default)]
struct SnippetFile {
    #[serde(default, rename = "snippet")]
    snippets: Vec<Snippet>,
}

fn config_dir() -> PathBuf {
    if let Ok(dir) = env::var("HERDR_PLUGIN_CONFIG_DIR") {
        if !dir.is_empty() {
            return PathBuf::from(dir);
        }
    }
    if let Ok(appdata) = env::var("APPDATA") {
        return PathBuf::from(appdata)
            .join("herdr")
            .join("plugins")
            .join("config")
            .join("prompt-deck");
    }
    if let Ok(home) = env::var("HOME") {
        return PathBuf::from(home)
            .join(".config")
            .join("herdr")
            .join("plugins")
            .join("config")
            .join("prompt-deck");
    }
    PathBuf::from("prompt-deck-config")
}

fn snippets_path() -> PathBuf {
    config_dir().join("snippets.toml")
}

fn load_snippets() -> Vec<Snippet> {
    let path = snippets_path();
    match std::fs::read_to_string(&path) {
        // A file we can read but not parse is left untouched; never clobber it.
        Ok(text) => toml::from_str::<SnippetFile>(&text)
            .map(|file| file.snippets)
            .unwrap_or_default(),
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            let empty: Vec<Snippet> = Vec::new();
            let _ = save_snippets(&empty);
            empty
        }
        Err(_) => Vec::new(),
    }
}

fn save_snippets(snippets: &[Snippet]) -> io::Result<()> {
    let file = SnippetFile {
        snippets: snippets.to_vec(),
    };
    let body = toml::to_string_pretty(&file).unwrap_or_default();
    let path = snippets_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, format!("{SNIPPETS_HEADER}\n{body}"))
}

// ---------------------------------------------------------------------------
// App
// ---------------------------------------------------------------------------

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

#[derive(Clone, Copy, PartialEq, Eq)]
enum Field {
    Name,
    Text,
}

struct Edit {
    name: String,
    text: String,
    field: Field,
    index: Option<usize>,
}

struct App {
    target: String,
    mode: Mode,
    recent: Vec<String>,
    status: Option<String>,
    should_quit: bool,
    snippets: Vec<Snippet>,
    filter: String,
    selected: usize,
    editing: Option<Edit>,
}

impl App {
    fn new(target: String) -> Self {
        App {
            target,
            mode: Mode::Files,
            recent: Vec::new(),
            status: None,
            should_quit: false,
            snippets: load_snippets(),
            filter: String::new(),
            selected: 0,
            editing: None,
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

    fn filtered_indices(&self) -> Vec<usize> {
        let query = self.filter.to_lowercase();
        self.snippets
            .iter()
            .enumerate()
            .filter(|(_, s)| {
                query.is_empty()
                    || s.name.to_lowercase().contains(&query)
                    || s.text.to_lowercase().contains(&query)
            })
            .map(|(i, _)| i)
            .collect()
    }

    fn clamp_selection(&mut self) {
        let count = self.filtered_indices().len();
        if count == 0 {
            self.selected = 0;
        } else if self.selected >= count {
            self.selected = count - 1;
        }
    }

    fn insert_selected(&mut self) {
        let indices = self.filtered_indices();
        let Some(&index) = indices.get(self.selected) else {
            return;
        };
        let snippet = self.snippets[index].clone();
        if self.target.is_empty() {
            self.status = Some("no target pane".to_string());
            return;
        }
        send_text(&self.target, &snippet.text);
        self.status = Some(format!("inserted '{}'", snippet.name));
    }

    fn start_new_snippet(&mut self) {
        self.editing = Some(Edit {
            name: String::new(),
            text: String::new(),
            field: Field::Name,
            index: None,
        });
    }

    fn start_edit_snippet(&mut self) {
        let indices = self.filtered_indices();
        let Some(&index) = indices.get(self.selected) else {
            return;
        };
        let snippet = self.snippets[index].clone();
        self.editing = Some(Edit {
            name: snippet.name,
            text: snippet.text,
            field: Field::Name,
            index: Some(index),
        });
    }

    fn delete_selected(&mut self) {
        let indices = self.filtered_indices();
        let Some(&index) = indices.get(self.selected) else {
            return;
        };
        let removed = self.snippets.remove(index);
        let _ = save_snippets(&self.snippets);
        self.status = Some(format!("deleted '{}'", removed.name));
        self.clamp_selection();
    }

    fn commit_edit(&mut self) {
        let Some(edit) = self.editing.take() else {
            return;
        };
        let name = edit.name.trim().to_string();
        if name.is_empty() {
            self.status = Some("a name is required".to_string());
            self.editing = Some(edit);
            return;
        }
        let snippet = Snippet { name, text: edit.text };
        match edit.index {
            Some(i) if i < self.snippets.len() => self.snippets[i] = snippet,
            _ => self.snippets.push(snippet),
        }
        let _ = save_snippets(&self.snippets);
        self.status = Some(format!("saved '{}'", edit.name.trim()));
        self.filter.clear();
        self.selected = 0;
    }

    fn on_key(&mut self, key: KeyEvent) {
        if self.editing.is_some() {
            self.edit_key(key);
            return;
        }
        if key.code == KeyCode::Esc {
            self.should_quit = true;
            return;
        }
        match key.code {
            KeyCode::Tab => {
                self.mode = Mode::from_index(self.mode.index() + 1);
                return;
            }
            KeyCode::BackTab => {
                self.mode = Mode::from_index(self.mode.index() + Mode::ALL.len() - 1);
                return;
            }
            _ => {}
        }
        match self.mode {
            Mode::Files => self.files_key(key),
            Mode::Snippets => self.snippets_key(key),
            Mode::Editor => {}
        }
    }

    fn files_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('b') => self.browse(),
            KeyCode::Char('1') => self.mode = Mode::Files,
            KeyCode::Char('2') => self.mode = Mode::Snippets,
            KeyCode::Char('3') => self.mode = Mode::Editor,
            _ => {}
        }
    }

    fn snippets_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Up => self.selected = self.selected.saturating_sub(1),
            KeyCode::Down => {
                if self.selected + 1 < self.filtered_indices().len() {
                    self.selected += 1;
                }
            }
            KeyCode::Backspace => {
                self.filter.pop();
                self.clamp_selection();
            }
            KeyCode::Enter => self.insert_selected(),
            KeyCode::Char('n') if ctrl => self.start_new_snippet(),
            KeyCode::Char('e') if ctrl => self.start_edit_snippet(),
            KeyCode::Char('d') if ctrl => self.delete_selected(),
            KeyCode::Char(c) if !ctrl => {
                self.filter.push(c);
                self.clamp_selection();
            }
            _ => {}
        }
    }

    fn edit_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let mut save = false;
        let mut cancel = false;

        if let Some(edit) = self.editing.as_mut() {
            match key.code {
                KeyCode::Esc => cancel = true,
                KeyCode::Tab | KeyCode::BackTab => {
                    edit.field = match edit.field {
                        Field::Name => Field::Text,
                        Field::Text => Field::Name,
                    };
                }
                KeyCode::Backspace => match edit.field {
                    Field::Name => {
                        edit.name.pop();
                    }
                    Field::Text => {
                        edit.text.pop();
                    }
                },
                KeyCode::Enter => match edit.field {
                    Field::Name => edit.field = Field::Text,
                    Field::Text => edit.text.push('\n'),
                },
                KeyCode::Char('s') if ctrl => save = true,
                KeyCode::Char(c) if !ctrl => match edit.field {
                    Field::Name => edit.name.push(c),
                    Field::Text => edit.text.push(c),
                },
                _ => {}
            }
        }

        if cancel {
            self.editing = None;
        }
        if save {
            self.commit_edit();
        }
    }
}

// ---------------------------------------------------------------------------
// Entry / loop
// ---------------------------------------------------------------------------

const DECK_TITLE: &str = "Prompt Deck";

fn herdr_output(args: &[&str]) -> io::Result<std::process::Output> {
    Command::new(herdr()).args(args).output()
}

#[derive(Deserialize, Clone, Default)]
struct Pane {
    #[serde(default)]
    pane_id: String,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    focused: bool,
    #[serde(default)]
    label: Option<String>,
    #[serde(default)]
    terminal_title: Option<String>,
    #[serde(default)]
    terminal_title_stripped: Option<String>,
}

impl Pane {
    fn is_deck(&self) -> bool {
        self.label.as_deref() == Some(DECK_TITLE)
            || self.terminal_title.as_deref() == Some(DECK_TITLE)
            || self.terminal_title_stripped.as_deref() == Some(DECK_TITLE)
    }
}

fn pane_list() -> io::Result<Vec<Pane>> {
    let out = herdr_output(&["pane", "list"])?;
    let value: serde_json::Value =
        serde_json::from_slice(&out.stdout).unwrap_or(serde_json::Value::Null);
    let panes = value
        .get("result")
        .and_then(|r| r.get("panes"))
        .and_then(|p| p.as_array());
    Ok(panes
        .map(|array| {
            array
                .iter()
                .filter_map(|p| serde_json::from_value::<Pane>(p.clone()).ok())
                .collect()
        })
        .unwrap_or_default())
}

fn current_pane() -> io::Result<Option<Pane>> {
    let out = herdr_output(&["pane", "current"])?;
    let value: serde_json::Value =
        serde_json::from_slice(&out.stdout).unwrap_or(serde_json::Value::Null);
    Ok(value
        .get("result")
        .and_then(|r| r.get("pane"))
        .and_then(|p| serde_json::from_value::<Pane>(p.clone()).ok()))
}

/// Herdr has no focus-by-id; focusing a pane is a momentary zoom on/off cycle.
fn focus_pane(pane_id: &str) {
    let _ = Command::new(herdr())
        .args(["pane", "zoom", pane_id, "--on"])
        .status();
    let _ = Command::new(herdr())
        .args(["pane", "zoom", pane_id, "--off"])
        .status();
}

fn plugin_config_dir() -> Option<String> {
    let out = Command::new(herdr())
        .args(["plugin", "config-dir", "prompt-deck"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let dir = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if dir.is_empty() {
        None
    } else {
        Some(dir)
    }
}

fn extract_new_pane_id(bytes: &[u8]) -> Option<String> {
    let value: serde_json::Value = serde_json::from_slice(bytes).ok()?;
    value
        .get("result")?
        .get("pane")?
        .get("pane_id")?
        .as_str()
        .map(str::to_string)
}

fn shell_quote(path: &str) -> String {
    if cfg!(windows) {
        format!("'{}'", path.replace('\'', "''"))
    } else {
        format!("'{}'", path.replace('\'', "'\\''"))
    }
}

/// The command typed into the new pane's shell to start the deck.
fn pane_run_command(target: &str) -> String {
    let exe = env::current_exe().unwrap_or_default();
    let quoted = shell_quote(&exe.to_string_lossy());
    if cfg!(windows) {
        format!("& {quoted} pane --target {target}")
    } else {
        format!("{quoted} pane --target {target}")
    }
}

/// Toggle the deck: focus it if it exists, otherwise split a slim pane at the bottom
/// and start the deck binary there, targeting the pane the caller came from.
fn launch() -> io::Result<()> {
    let panes = pane_list()?;

    if let Some(deck) = panes.iter().find(|p| p.is_deck()) {
        focus_pane(&deck.pane_id);
        return Ok(());
    }

    let target = match panes.iter().find(|p| p.focused).cloned() {
        Some(pane) => pane,
        None => current_pane()?.ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotFound, "no focused pane to target")
        })?,
    };

    let cwd = target.cwd.clone().unwrap_or_else(|| ".".to_string());
    let mut args: Vec<String> = ["pane", "split", "--direction", "down", "--cwd", &cwd, "--ratio", "0.9", "--focus"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    if let Some(config_dir) = plugin_config_dir() {
        args.push("--env".to_string());
        args.push(format!("HERDR_PLUGIN_CONFIG_DIR={config_dir}"));
    }

    let out = Command::new(herdr()).args(&args).output()?;
    let new_pane = extract_new_pane_id(&out.stdout)
        .ok_or_else(|| io::Error::new(io::ErrorKind::Other, "pane split returned no pane"))?;

    let _ = Command::new(herdr())
        .args(["pane", "rename", &new_pane, DECK_TITLE])
        .status();

    let command = pane_run_command(&target.pane_id);
    Command::new(herdr())
        .args(["pane", "run", &new_pane, &command])
        .status()?;
    Ok(())
}

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    match args.first().map(String::as_str).unwrap_or("pane") {
        "launch" => {
            if let Err(err) = launch() {
                eprintln!("prompt-deck: {err}");
                std::process::exit(1);
            }
        }
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
                app.on_key(key);
            }
        }
        if app.should_quit {
            return Ok(());
        }
    }
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

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
        Mode::Files => content_files(app),
        Mode::Snippets => content_snippets(app),
        Mode::Editor => content_editor(),
    }
}

fn content_files(app: &App) -> Vec<Line<'static>> {
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

fn content_snippets(app: &App) -> Vec<Line<'static>> {
    if let Some(edit) = &app.editing {
        let title = if edit.index.is_some() {
            "Edit snippet"
        } else {
            "New snippet"
        };
        let bold = Style::default().add_modifier(Modifier::BOLD);
        let field_style = |active: bool| {
            if active {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default().fg(Color::Gray)
            }
        };

        let mut lines = vec![Line::from(Span::styled(title.to_string(), bold))];
        lines.push(Line::from(vec![
            Span::styled("  name  ", field_style(edit.field == Field::Name)),
            Span::styled(
                format!(
                    "{}{}",
                    edit.name,
                    if edit.field == Field::Name { "_" } else { "" }
                ),
                Style::default().fg(Color::White),
            ),
        ]));
        lines.push(Line::from(Span::styled(
            "  text",
            field_style(edit.field == Field::Text),
        )));
        let text_lines: Vec<&str> = edit.text.split('\n').collect();
        for (i, part) in text_lines.iter().enumerate() {
            let last = i + 1 == text_lines.len();
            let cursor = if last && edit.field == Field::Text {
                "_"
            } else {
                ""
            };
            lines.push(Line::from(Span::styled(
                format!("    {part}{cursor}"),
                Style::default().fg(Color::White),
            )));
        }
        return lines;
    }

    let mut lines = vec![Line::from(vec![
        Span::styled("  filter  ", Style::default().fg(Color::DarkGray)),
        Span::styled(app.filter.clone(), Style::default().fg(Color::White)),
        Span::styled("_", Style::default().fg(Color::Yellow)),
    ])];

    let indices = app.filtered_indices();
    if indices.is_empty() {
        lines.push(Line::from(Span::styled(
            "  no snippets yet — ctrl+n to create one",
            Style::default().fg(Color::DarkGray),
        )));
    }
    for (row, &index) in indices.iter().enumerate() {
        let snippet = &app.snippets[index];
        let selected = row == app.selected;
        let style = if selected {
            Style::default()
                .fg(Color::Black)
                .bg(Color::Yellow)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Gray)
        };
        let preview = snippet.text.lines().next().unwrap_or("");
        lines.push(Line::from(Span::styled(
            format!(
                "  {:<20}  {}",
                truncate(&snippet.name, 20),
                truncate(preview, 48)
            ),
            style,
        )));
    }
    lines
}

fn content_editor() -> Vec<Line<'static>> {
    vec![
        Line::from(Span::styled(
            "Editor",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            "  coming next: compose text and insert it",
            Style::default().fg(Color::DarkGray),
        )),
    ]
}

fn footer(app: &App, width: u16) -> Paragraph<'static> {
    let hints: &[(&str, &str)] = if app.editing.is_some() {
        &[("tab", "field"), ("ctrl+s", "save"), ("esc", "cancel")]
    } else {
        match app.mode {
            Mode::Files => &[("b", "browse"), ("tab", "tools"), ("esc", "close")],
            Mode::Snippets => &[
                ("type", "filter"),
                ("↑↓", "select"),
                ("↵", "insert"),
                ("ctrl+n", "new"),
                ("ctrl+e", "edit"),
                ("ctrl+d", "delete"),
                ("tab", "tools"),
                ("esc", "close"),
            ],
            Mode::Editor => &[("tab", "tools"), ("esc", "close")],
        }
    };

    let mut spans: Vec<Span> = Vec::new();
    for (k, label) in hints {
        spans.push(Span::styled(
            format!(" {k} "),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(
            format!("{label}  "),
            Style::default().fg(Color::DarkGray),
        ));
    }

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

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deck_detected_from_any_title_field() {
        let mut pane = Pane::default();
        assert!(!pane.is_deck());
        pane.label = Some("Prompt Deck".to_string());
        assert!(pane.is_deck());
        pane.label = None;
        pane.terminal_title = Some("Prompt Deck".to_string());
        assert!(pane.is_deck());
        pane.terminal_title = None;
        pane.terminal_title_stripped = Some("Prompt Deck".to_string());
        assert!(pane.is_deck());
    }

    #[test]
    fn unrelated_titles_are_not_the_deck() {
        let pane = Pane {
            terminal_title: Some("OC | building a plugin".to_string()),
            ..Default::default()
        };
        assert!(!pane.is_deck());
    }

    #[test]
    fn extracts_pane_id_from_split_reply() {
        let json = br#"{"result":{"pane":{"pane_id":"w1:p2"},"type":"pane_info"}}"#;
        assert_eq!(extract_new_pane_id(json).as_deref(), Some("w1:p2"));
        assert_eq!(extract_new_pane_id(b"not json"), None);
    }
}
