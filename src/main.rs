mod ellipsized_text;
mod frecency;

use ellipsized_text::ellipsized_text;
use frecency::Frecency;
use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};
use strsim::jaro_winkler;
use iced::font::{Family, Weight};
use iced::keyboard::{key::Named, Key};
use iced::widget::{column, container, image, mouse_area, row, scrollable, stack, text, text_input, Column};
use iced::window;
use iced::{Color, Element, Font, Length, Padding, Subscription, Task, Theme};
use iced::application::Appearance;
use serde::Deserialize;
use std::collections::HashMap;
use std::env;
use std::fs;
use std::io::{BufRead, BufReader};
use std::os::unix::net::UnixListener;
use std::path::PathBuf;
use std::process::Command;

const GOLDEN_RATIO: f32 = 1.618;
const TEXT_SIZE: u16 = 16;
const INPUT_SIZE: f32 = TEXT_SIZE as f32 * GOLDEN_RATIO;  // ~26px
const INPUT_PADDING: f32 = 8.0 * GOLDEN_RATIO;  // ~13px
const ICON_SIZE: u16 = 20;
const ROW_PADDING: u16 = 4;

fn socket_path() -> PathBuf {
    let runtime_dir = env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".to_string());
    PathBuf::from(runtime_dir).join("launcher.sock")
}
const FONT: Font = Font {
    family: Family::Name("IBM Plex Sans"),
    weight: Weight::Medium,
    ..Font::DEFAULT
};
fn get_width() -> u16 {
    // Query monitor and calculate golden ratio width
    let output = Command::new("hyprctl")
        .args(["monitors", "-j"])
        .output()
        .ok();

    if let Some(output) = output {
        if let Ok(monitors) = serde_json::from_slice::<Vec<serde_json::Value>>(&output.stdout) {
            if let Some(mon) = monitors.first() {
                let width = mon["width"].as_f64().unwrap_or(1920.0);
                let scale = mon["scale"].as_f64().unwrap_or(1.0);
                let logical_width = width / scale;
                return (logical_width * 0.382) as u16; // 1 - φ
            }
        }
    }
    611 // fallback
}

fn main() -> iced::Result {
    env_logger::init();
    let window_settings = window::Settings {
        size: iced::Size::new(800.0, 600.0), // Will be fullscreened by hyprland
        decorations: false,
        resizable: true,
        transparent: true,
        platform_specific: window::settings::PlatformSpecific {
            application_id: "launcher".to_string(),
            ..Default::default()
        },
        ..Default::default()
    };

    iced::application("launcher", App::update, App::view)
        .subscription(App::subscription)
        .theme(|_| Theme::Dark)
        .style(|_state, _theme| Appearance {
            background_color: Color::from_rgba(0.0, 0.0, 0.0, 0.3),
            text_color: Color::WHITE,
        })
        .default_font(FONT)
        .window(window_settings)
        .exit_on_close_request(false)
        .run_with(App::new)
}

#[derive(Clone, Debug)]
enum Entry {
    Desktop {
        name: String,
        desktop_file: PathBuf,
        action: Option<String>,
        exec: Option<String>,
        terminal: bool,
        icon: Option<PathBuf>,
        keywords: Vec<String>,
    },
    Window {
        title: String,
        class: String,
        address: String,
        icon: Option<PathBuf>,
    },
}

impl Entry {
    fn name(&self) -> &str {
        match self {
            Entry::Desktop { name, .. } => name,
            Entry::Window { title, class, .. } => {
                if title.is_empty() { class } else { title }
            }
        }
    }

    fn searchable(&self) -> Vec<&str> {
        match self {
            Entry::Desktop { name, keywords, .. } => {
                let mut v = vec![name.as_str()];
                v.extend(keywords.iter().map(|s| s.as_str()));
                v
            }
            Entry::Window { title, class, .. } => vec![title.as_str(), class.as_str()],
        }
    }

    fn icon(&self) -> Option<&PathBuf> {
        match self {
            Entry::Desktop { icon, .. } => icon.as_ref(),
            Entry::Window { icon, .. } => icon.as_ref(),
        }
    }

    fn is_window(&self) -> bool {
        matches!(self, Entry::Window { .. })
    }

    fn frecency_key(&self) -> String {
        match self {
            Entry::Desktop { desktop_file, action, .. } => {
                let base = desktop_file.to_string_lossy().to_string();
                if let Some(act) = action {
                    format!("{}#{}", base, act)
                } else {
                    base
                }
            }
            Entry::Window { class, .. } => format!("window:{}", class),
        }
    }
}

struct App {
    query: String,
    entries: Vec<Entry>,
    filtered: Vec<usize>,
    selected: usize,
    matcher: Matcher,
    visible: bool,
    icon_index: HashMap<String, PathBuf>,
    wmclass_icons: HashMap<String, PathBuf>,
    width: u16,
    frecency: Frecency,
}

#[derive(Debug, Clone)]
enum Message {
    QueryChanged(String),
    Submit,
    SelectNext,
    SelectPrev,
    Select(usize),
    ClearQuery,
    DeleteWord,
    TabComplete,
    CursorToEnd,
    Hide,
    Toggle,
    Show,
    Reset,
}

fn ipc_subscription() -> Subscription<Message> {
    Subscription::run(|| {
        let path = socket_path();
        iced::stream::channel(100, |mut output| async move {
            use iced::futures::SinkExt;

            // Remove old socket
            let _ = fs::remove_file(&path);

            let listener = match UnixListener::bind(&path) {
                Ok(l) => l,
                Err(e) => {
                    eprintln!("Failed to bind socket: {}", e);
                    std::future::pending::<()>().await;
                    unreachable!()
                }
            };

            // Set non-blocking for async compatibility
            listener.set_nonblocking(true).ok();

            loop {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let reader = BufReader::new(&stream);
                        for line in reader.lines().flatten() {
                            let msg = match line.trim() {
                                "toggle" => Some(Message::Toggle),
                                "show" => Some(Message::Show),
                                "hide" => Some(Message::Hide),
                                _ => None,
                            };
                            if let Some(m) = msg {
                                let _ = output.send(m).await;
                            }
                        }
                    }
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                    }
                    Err(_) => {
                        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                    }
                }
            }
        })
    })
}

impl App {
    fn new() -> (Self, Task<Message>) {
        let icon_index = build_icon_index();
        let wmclass_icons = build_wmclass_icon_map(&icon_index);
        let entries = collect_entries(&icon_index, &wmclass_icons);
        let filtered: Vec<usize> = (0..entries.len()).collect();

        (
            Self {
                query: String::new(),
                entries,
                filtered,
                selected: 0,
                matcher: Matcher::new(Config::DEFAULT),
                visible: true,
                icon_index,
                wmclass_icons,
                width: get_width(),
                frecency: Frecency::load(),
            },
            text_input::focus(text_input::Id::new("search")),
        )
    }

    fn subscription(&self) -> Subscription<Message> {
        use iced::event::{self, Event};
        use iced::keyboard::Event as KeyEvent;
        use iced::window::Event as WindowEvent;

        // Listen to ALL events and filter
        let events = event::listen_with(|event, _status, _window| {
            match &event {
                Event::Keyboard(KeyEvent::KeyPressed { key, modifiers, .. }) => {
                    match key {
                        Key::Named(Named::Escape) => Some(Message::Hide),
                        Key::Named(Named::ArrowDown) => Some(Message::SelectNext),
                        Key::Named(Named::ArrowUp) => Some(Message::SelectPrev),
                        Key::Named(Named::Enter) => Some(Message::Submit),
                        Key::Named(Named::Tab) => Some(Message::TabComplete),
                        Key::Character(c) if modifiers.control() => {
                            match c.to_lowercase().as_str() {
                                "j" => Some(Message::SelectNext),
                                "k" => Some(Message::SelectPrev),
                                "n" => Some(Message::SelectNext),
                                "p" => Some(Message::SelectPrev),
                                "u" => Some(Message::ClearQuery),
                                "w" => Some(Message::DeleteWord),
                                "e" => Some(Message::CursorToEnd),
                                _ => None,
                            }
                        }
                        _ => None,
                    }
                }
                Event::Window(WindowEvent::CloseRequested) => Some(Message::Hide),
                Event::Window(WindowEvent::Unfocused) => None,
                Event::Window(WindowEvent::Focused) => Some(Message::Show),
                _ => None,
            }
        });

        Subscription::batch([events, ipc_subscription()])
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        eprintln!("MSG: {:?}, visible: {}", message, self.visible);
        match message {
            Message::Reset => {
                // Unfocused - hide (click outside closes overlay)
                eprintln!("Reset triggered - hiding");
                return self.hide();
            }
            Message::QueryChanged(query) => {
                self.query = query;
                self.filter();
                self.selected = 0;
            }
            Message::Submit => {
                if !self.visible {
                    return Task::none();
                }
                if let Some(&idx) = self.filtered.get(self.selected) {
                    if let Some(entry) = self.entries.get(idx) {
                        self.frecency.record(&entry.frecency_key());
                        // Hide special workspace FIRST so app opens on main workspace
                        let _ = Command::new("hyprctl")
                            .args(["dispatch", "togglespecialworkspace", "launcher"])
                            .output();
                        self.visible = false;
                        self.query.clear();
                        self.selected = 0;
                        activate(entry);
                        return Task::none();
                    }
                }
            }
            Message::SelectNext => {
                let max_visible = 20.min(self.filtered.len());
                if self.selected < max_visible.saturating_sub(1) {
                    self.selected += 1;
                }
                return self.scroll_to_visible(true);
            }
            Message::SelectPrev => {
                self.selected = self.selected.saturating_sub(1);
                return self.scroll_to_visible(false);
            }
            Message::Select(i) => {
                if !self.visible {
                    return Task::none();
                }
                if let Some(&idx) = self.filtered.get(i) {
                    if let Some(entry) = self.entries.get(idx) {
                        self.frecency.record(&entry.frecency_key());
                        // Hide special workspace FIRST so app opens on main workspace
                        let _ = Command::new("hyprctl")
                            .args(["dispatch", "togglespecialworkspace", "launcher"])
                            .output();
                        self.visible = false;
                        self.query.clear();
                        self.selected = 0;
                        activate(entry);
                        return Task::none();
                    }
                }
            }
            Message::ClearQuery => {
                self.query.clear();
                self.filter();
                self.selected = 0;
            }
            Message::DeleteWord => {
                // Delete last word (from end to previous space/start)
                let trimmed = self.query.trim_end();
                if let Some(pos) = trimmed.rfind(|c: char| c.is_whitespace()) {
                    self.query = trimmed[..=pos].to_string();
                } else {
                    self.query.clear();
                }
                self.filter();
                self.selected = 0;
            }
            Message::TabComplete => {
                // Complete to top match if it starts with query
                if let Some(&idx) = self.filtered.first() {
                    let name = self.entries[idx].name();
                    if name.to_lowercase().starts_with(&self.query.to_lowercase()) {
                        self.query = name.to_string();
                        self.filter();
                        self.selected = 0;
                        return text_input::move_cursor_to_end(text_input::Id::new("search"));
                    }
                }
            }
            Message::CursorToEnd => {
                return text_input::move_cursor_to_end(text_input::Id::new("search"));
            }
            Message::Hide => {
                return self.hide();
            }
            Message::Show => {
                return self.show();
            }
            Message::Toggle => {
                if self.visible {
                    return self.hide();
                } else {
                    return self.show();
                }
            }
        }
        Task::none()
    }

    fn hide(&mut self) -> Task<Message> {
        self.visible = false;
        self.query.clear();
        self.selected = 0;
        // Toggle special workspace OFF (hides it)
        let _ = Command::new("hyprctl")
            .args(["dispatch", "togglespecialworkspace", "launcher"])
            .output();
        Task::none()
    }

    fn show(&mut self) -> Task<Message> {
        self.visible = true;
        self.query.clear();
        self.entries = collect_entries(&self.icon_index, &self.wmclass_icons);
        self.filter();
        self.selected = 0;
        // Just focus input - hyprland handles showing the workspace
        Task::batch([
            text_input::focus(text_input::Id::new("search")),
            scrollable::scroll_to(scrollable::Id::new("results"), scrollable::AbsoluteOffset { x: 0.0, y: 0.0 }),
        ])
    }

    fn view(&self) -> Element<'_, Message> {

        // Ghost text: show when top result starts with query
        let ghost_completion = if !self.query.is_empty() {
            self.filtered.first().and_then(|&idx| {
                let name = self.entries[idx].name();
                let name_lower = name.to_lowercase();
                let query_lower = self.query.to_lowercase();
                if name_lower.starts_with(&query_lower) {
                    Some(name.chars().skip(self.query.chars().count()).collect::<String>())
                } else {
                    None
                }
            })
        } else {
            None
        };

        let input_field = text_input("", &self.query)
            .id(text_input::Id::new("search"))
            .on_input(Message::QueryChanged)
            .padding(Padding { top: INPUT_PADDING, right: INPUT_PADDING, bottom: INPUT_PADDING, left: 0.0 })
            .size(INPUT_SIZE)
            .width(Length::Fill)
            .line_height(text::LineHeight::Absolute(INPUT_SIZE.into()))
            .style(|_theme, _status| text_input::Style {
                background: iced::Background::Color(iced::Color::TRANSPARENT),
                border: iced::Border::default(),
                icon: iced::Color::from_rgb(0.5, 0.5, 0.5),
                placeholder: iced::Color::from_rgb(0.4, 0.4, 0.4),
                value: iced::Color::from_rgb(0.85, 0.85, 0.85),
                selection: iced::Color::from_rgba(0.3, 0.5, 0.8, 0.3),
            });

        // Ghost text: use same layout as input - invisible query + visible completion
        let ghost_text = ghost_completion.unwrap_or_default();
        let ghost_visible = !ghost_text.is_empty();

        let ghost_layer: Element<Message> = row![
            // Invisible spacer matching query text
            text(self.query.clone())
                .size(INPUT_SIZE)
                .font(FONT)
                .line_height(text::LineHeight::Absolute(INPUT_SIZE.into()))
                .color(iced::Color::TRANSPARENT),
            // Visible ghost completion with ellipsis
            ellipsized_text(ghost_text.clone())
                .size(INPUT_SIZE)
                .font(FONT)
                .line_height(text::LineHeight::Absolute(INPUT_SIZE.into()))
                .color(if ghost_visible {
                    iced::Color::from_rgba(0.5, 0.5, 0.5, 0.6)
                } else {
                    iced::Color::TRANSPARENT
                })
        ]
        .padding(Padding { top: INPUT_PADDING, right: INPUT_PADDING, bottom: INPUT_PADDING, left: 0.0 })
        .into();

        // Prompt character
        let prompt: Element<Message> = container(
            text(">")
                .size(INPUT_SIZE)
                .font(FONT)
                .line_height(text::LineHeight::Absolute(INPUT_SIZE.into()))
                .color(iced::Color::from_rgb(0.4, 0.4, 0.4))
        )
        .padding(Padding { top: INPUT_PADDING, right: 8.0, bottom: INPUT_PADDING, left: INPUT_PADDING })
        .into();

        // Stack: ghost behind (first), input on top (second)
        let input_row: Element<Message> = row![prompt, stack![ghost_layer, input_field].width(Length::Fill)]
            .width(Length::Fill)
            .into();

        // Input area with background
        let input_area: Element<Message> = container(input_row)
            .width(Length::Fill)
            .style(|_theme| container::Style {
                background: Some(iced::Background::Color(iced::Color::from_rgba(0.0, 0.0, 0.0, 0.85))),
                border: iced::Border {
                    radius: 3.0.into(),
                    ..Default::default()
                },
                ..Default::default()
            })
            .into();

        let results: Column<Message> = self
            .filtered
            .iter()
            .take(20)
            .enumerate()
            .fold(Column::new().spacing(0), |col, (i, &idx)| {
                let entry = &self.entries[idx];
                let selected = i == self.selected;

                let name = entry.name();

                let base_color = if selected {
                    iced::Color::from_rgb(0.9, 0.9, 0.9)
                } else {
                    iced::Color::from_rgb(0.6, 0.6, 0.6)
                };

                // Use ellipsized text with proper truncation
                let icon_container_size = ICON_SIZE + 4;
                let label: Element<Message> = ellipsized_text(name)
                    .size(TEXT_SIZE)
                    .line_height(iced::widget::text::LineHeight::Absolute((icon_container_size as f32).into()))
                    .color(base_color)
                    .font(FONT)
                    .into();

                // Icon in circular container with subtle fill
                let is_window = entry.is_window();
                let icon_inner: Element<Message> = if let Some(icon_path) = entry.icon() {
                    image(icon_path.clone())
                        .width(ICON_SIZE)
                        .height(ICON_SIZE)
                        .into()
                } else {
                    iced::widget::Space::new(ICON_SIZE, ICON_SIZE).into()
                };
                let icon_element: Element<Message> = container(icon_inner)
                    .width(icon_container_size)
                    .height(icon_container_size)
                    .center_x(icon_container_size)
                    .center_y(icon_container_size)
                    .style(move |_theme| container::Style {
                        background: Some(iced::Background::Color(iced::Color::from_rgba(1.0, 1.0, 1.0, 0.05))),
                        border: iced::Border {
                            radius: (icon_container_size as f32 / 2.0).into(),
                            width: if is_window { 1.5 } else { 0.0 },
                            color: iced::Color::from_rgb(0.4, 0.65, 0.85),
                        },
                        ..Default::default()
                    })
                    .into();

                let content: Element<Message> = row![icon_element, label]
                    .spacing(6)
                    .align_y(iced::Alignment::Center)
                    .into();

                let row_container = container(content)
                    .padding([ROW_PADDING, ROW_PADDING])
                    .width(Length::Fill)
                    .style(move |_theme| container::Style {
                        background: if selected {
                            Some(iced::Background::Color(iced::Color::from_rgba(0.3, 0.5, 0.8, 0.15)))
                        } else {
                            None
                        },
                        ..Default::default()
                    });

                col.push(
                    mouse_area(row_container)
                        .on_press(Message::Select(i))
                        .interaction(iced::mouse::Interaction::Pointer)
                )
            });

        let scroll_area = scrollable(results)
            .id(scrollable::Id::new("results"))
            .height(Length::Shrink)
            .style(|_theme, _status| scrollable::Style {
                container: container::Style::default(),
                vertical_rail: scrollable::Rail {
                    background: None,
                    border: iced::Border::default(),
                    scroller: scrollable::Scroller {
                        color: iced::Color::TRANSPARENT,
                        border: iced::Border::default(),
                    },
                },
                horizontal_rail: scrollable::Rail {
                    background: None,
                    border: iced::Border::default(),
                    scroller: scrollable::Scroller {
                        color: iced::Color::TRANSPARENT,
                        border: iced::Border::default(),
                    },
                },
                gap: None,
            });

        let content: Element<Message> = if self.filtered.is_empty() {
            if self.query.is_empty() {
                column![input_area].spacing(0).into()
            } else {
                // No matches state
                let no_matches = container(
                    text("No matches")
                        .size(TEXT_SIZE)
                        .color(iced::Color::from_rgba(0.5, 0.5, 0.5, 0.7))
                        .font(FONT)
                )
                .padding([ROW_PADDING, INPUT_PADDING as u16])
                .width(Length::Fill);
                column![input_area, no_matches].spacing(0).into()
            }
        } else {
            column![input_area, scroll_area].spacing(0).into()
        };

        // Launcher panel with content
        let panel = container(content)
            .width(self.width);

        // Fullscreen container that centers the panel at golden ratio
        container(panel)
            .width(Length::Fill)
            .height(Length::Fill)
            .align_x(iced::alignment::Horizontal::Center)
            .align_y(iced::alignment::Vertical::Top)
            .padding(Padding { top: 200.0, ..Padding::ZERO }) // Approximate golden ratio from top
            .style(|_theme| container::Style {
                background: None,
                ..Default::default()
            })
            .into()
    }

    fn scroll_to_visible(&self, going_down: bool) -> Task<Message> {
        let icon_container_size = ICON_SIZE + 4;
        let row_height = (icon_container_size + ROW_PADDING * 2) as f32;
        let visible_rows = 10;

        let offset = if going_down {
            if self.selected >= visible_rows {
                let top = self.selected - visible_rows + 1;
                top as f32 * row_height
            } else {
                return Task::none();
            }
        } else {
            self.selected as f32 * row_height
        };

        scrollable::scroll_to(
            scrollable::Id::new("results"),
            scrollable::AbsoluteOffset { x: 0.0, y: offset },
        )
    }

    fn filter(&mut self) {
        if self.query.is_empty() {
            // Sort by frecency when no query
            let mut scored: Vec<_> = self.entries.iter().enumerate()
                .map(|(idx, e)| (self.frecency.score(&e.frecency_key()), idx))
                .collect();
            scored.sort_by(|a, b| b.0.cmp(&a.0));
            self.filtered = scored.into_iter().map(|(_, idx)| idx).collect();
        } else {
            let pattern = Pattern::parse(&self.query, CaseMatching::Ignore, Normalization::Smart);
            let query_lower = self.query.to_lowercase();
            let mut scored: Vec<_> = self
                .entries
                .iter()
                .enumerate()
                .filter_map(|(idx, e)| {
                    // Try nucleo (subsequence matching)
                    let nucleo_score: u32 = e
                        .searchable()
                        .iter()
                        .filter_map(|s| {
                            let mut buf = Vec::new();
                            let haystack = Utf32Str::new(s, &mut buf);
                            pattern.score(haystack, &mut self.matcher)
                        })
                        .max()
                        .unwrap_or(0);

                    // Only use jaro-winkler for typo tolerance if nucleo found nothing
                    // and require high similarity (0.85+)
                    let jw_score: u32 = if nucleo_score == 0 {
                        e.searchable()
                            .iter()
                            .map(|s| (jaro_winkler(&query_lower, &s.to_lowercase()) * 1000.0) as u32)
                            .filter(|&s| s >= 850) // Only accept high similarity
                            .max()
                            .unwrap_or(0)
                    } else {
                        0
                    };

                    // Check for exact prefix match (huge bonus)
                    let prefix_bonus: u32 = if e.searchable().iter().any(|s|
                        s.to_lowercase().starts_with(&query_lower)
                    ) { 10000 } else { 0 };

                    // Combine: match score dominates, frecency as tiebreaker
                    let match_score = nucleo_score.max(jw_score) + prefix_bonus;
                    if match_score == 0 { return None; }
                    let frecency_boost = self.frecency.score(&e.frecency_key()).min(200);
                    let total = match_score * 10 + frecency_boost;
                    Some((total, idx))
                })
                .collect();
            scored.sort_by(|a, b| b.0.cmp(&a.0));
            self.filtered = scored.into_iter().map(|(_, idx)| idx).collect();
        }
    }
}

fn activate(entry: &Entry) {
    match entry {
        Entry::Desktop { desktop_file, action, exec, terminal, .. } => {
            // Get the exec line - either from action or main entry
            let exec_to_run = if let Some(action_id) = action {
                get_action_exec(desktop_file, action_id).or_else(|| exec.clone())
            } else {
                exec.clone()
            };

            if let Some(exec_line) = exec_to_run {
                // Parse exec line, removing field codes (%u, %F, etc.)
                let cmd_str: String = exec_line
                    .split_whitespace()
                    .filter(|s| !s.starts_with('%'))
                    .collect::<Vec<_>>()
                    .join(" ");

                // Parse command into binary + args
                let mut cmd_parts = cmd_str.split_whitespace();
                let Some(bin) = cmd_parts.next() else { return };
                let args: Vec<&str> = cmd_parts.collect();

                if *terminal && action.is_none() {
                    // Terminal apps: parse TERMINAL (may have args like "kitty --single-instance")
                    let term = env::var("TERMINAL").expect("TERMINAL env var must be set");
                    let mut term_parts = term.split_whitespace();
                    let term_bin = term_parts.next().unwrap();
                    let term_args: Vec<&str> = term_parts.collect();
                    let _ = Command::new(term_bin)
                        .args(&term_args)
                        .arg("-e")
                        .arg(bin)
                        .args(&args)
                        .stdin(std::process::Stdio::null())
                        .stdout(std::process::Stdio::null())
                        .stderr(std::process::Stdio::null())
                        .spawn();
                } else {
                    // GUI apps: direct exec
                    let _ = Command::new(bin)
                        .args(&args)
                        .stdin(std::process::Stdio::null())
                        .stdout(std::process::Stdio::null())
                        .stderr(std::process::Stdio::null())
                        .spawn();
                }
            }
        }
        Entry::Window { address, .. } => {
            let _ = Command::new("hyprctl")
                .args(["dispatch", "focuswindow", &format!("address:{}", address)])
                .output();
        }
    }
}

fn get_action_exec(desktop_file: &PathBuf, action_id: &str) -> Option<String> {
    let content = fs::read_to_string(desktop_file).ok()?;
    let target_section = format!("[Desktop Action {}]", action_id);
    let mut in_target_section = false;

    for line in content.lines() {
        let line = line.trim();
        if line.starts_with('[') && line.ends_with(']') {
            in_target_section = line == target_section;
            continue;
        }
        if in_target_section {
            if let Some(exec) = line.strip_prefix("Exec=") {
                return Some(exec.to_string());
            }
        }
    }
    None
}

fn collect_entries(icon_index: &HashMap<String, PathBuf>, wmclass_icons: &HashMap<String, PathBuf>) -> Vec<Entry> {
    let mut entries = collect_hyprland_windows(icon_index, wmclass_icons);
    entries.extend(collect_desktop_entries(icon_index));
    entries
}

#[derive(Deserialize)]
struct HyprClient {
    address: String,
    title: String,
    class: String,
    #[serde(rename = "focusHistoryID")]
    focus_history_id: i32,
}

fn build_wmclass_icon_map(icon_index: &HashMap<String, PathBuf>) -> HashMap<String, PathBuf> {
    let mut map = HashMap::new();

    for dir in get_applications_dirs() {
        if let Ok(read_dir) = fs::read_dir(&dir) {
            for entry in read_dir.flatten() {
                let path = entry.path();
                if path.extension().is_some_and(|e| e == "desktop") {
                    if let Ok(content) = fs::read_to_string(&path) {
                        let mut wmclass = None;
                        let mut icon_name = None;

                        for line in content.lines() {
                            if let Some(v) = line.strip_prefix("StartupWMClass=") {
                                wmclass = Some(v.to_lowercase());
                            } else if let Some(v) = line.strip_prefix("Icon=") {
                                icon_name = Some(v.to_string());
                            }
                        }

                        if let (Some(wm), Some(icon)) = (wmclass, icon_name) {
                            if let Some(icon_path) = icon_index.get(&icon) {
                                map.entry(wm).or_insert_with(|| icon_path.clone());
                            }
                        }
                    }
                }
            }
        }
    }

    map
}

fn collect_hyprland_windows(icon_index: &HashMap<String, PathBuf>, wmclass_icons: &HashMap<String, PathBuf>) -> Vec<Entry> {
    let output = Command::new("hyprctl")
        .args(["clients", "-j"])
        .output()
        .ok();

    let Some(output) = output else { return vec![] };
    if !output.status.success() { return vec![]; }

    let mut clients: Vec<HyprClient> = serde_json::from_slice(&output.stdout).unwrap_or_default();

    // Sort by focus recency (lower focusHistoryID = more recent)
    clients.sort_by_key(|c| c.focus_history_id);

    clients
        .into_iter()
        .filter(|c| !c.class.is_empty() && c.class != "launcher")
        .map(|c| {
            let class_lower = c.class.to_lowercase();
            let icon = wmclass_icons.get(&class_lower)
                .or_else(|| icon_index.get(&class_lower))
                .cloned();
            Entry::Window {
                title: c.title,
                class: c.class,
                address: c.address,
                icon,
            }
        })
        .collect()
}

fn collect_desktop_entries(icon_index: &HashMap<String, PathBuf>) -> Vec<Entry> {
    let mut seen_files: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut entries = Vec::new();

    for dir in get_applications_dirs() {
        if let Ok(read_dir) = fs::read_dir(&dir) {
            for entry in read_dir.flatten() {
                let path = entry.path();
                if path.extension().is_some_and(|e| e == "desktop") {
                    let key = path.file_name().unwrap().to_string_lossy().to_string();
                    if seen_files.insert(key) {
                        entries.extend(parse_desktop_file(&path, icon_index));
                    }
                }
            }
        }
    }

    entries.sort_by(|a, b| a.name().to_lowercase().cmp(&b.name().to_lowercase()));
    entries
}

fn get_applications_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();

    if let Ok(home) = env::var("HOME") {
        dirs.push(PathBuf::from(home).join(".local/share/applications"));
    }

    let data_dirs = env::var("XDG_DATA_DIRS")
        .unwrap_or_else(|_| "/usr/local/share:/usr/share".to_string());
    for dir in data_dirs.split(':') {
        dirs.push(PathBuf::from(dir).join("applications"));
    }

    dirs.push(PathBuf::from("/var/lib/flatpak/exports/share/applications"));

    dirs
}

fn get_icon_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();

    if let Ok(home) = env::var("HOME") {
        dirs.push(PathBuf::from(&home).join(".local/share/icons"));
        dirs.push(PathBuf::from(&home).join(".icons"));
    }

    let data_dirs = env::var("XDG_DATA_DIRS")
        .unwrap_or_else(|_| "/usr/local/share:/usr/share".to_string());
    for dir in data_dirs.split(':') {
        dirs.push(PathBuf::from(dir).join("icons"));
    }

    dirs.push(PathBuf::from("/usr/share/pixmaps"));

    dirs
}

fn build_icon_index() -> HashMap<String, PathBuf> {
    let mut index: HashMap<String, PathBuf> = HashMap::new();

    let sizes = ["256x256", "128x128", "64x64", "48x48", "32x32", "24x24", "scalable"];
    let categories = ["apps", "applications"];

    for base_dir in get_icon_dirs() {
        let hicolor = base_dir.join("hicolor");
        for size in &sizes {
            for category in &categories {
                let dir = hicolor.join(size).join(category);
                if let Ok(entries) = fs::read_dir(&dir) {
                    for entry in entries.flatten() {
                        let path = entry.path();
                        if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                            index.entry(stem.to_string()).or_insert(path);
                        }
                    }
                }
            }
        }

        if let Ok(entries) = fs::read_dir(&base_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file() {
                    if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                        index.entry(stem.to_string()).or_insert(path);
                    }
                }
            }
        }
    }

    index
}

fn parse_desktop_file(path: &PathBuf, icon_index: &HashMap<String, PathBuf>) -> Vec<Entry> {
    let content = match fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return vec![],
    };

    let mut entries = Vec::new();
    let mut main_name = None;
    let mut main_exec = None;
    let mut main_icon_name = None;
    let mut no_display = false;
    let mut hidden = false;
    let mut terminal = false;
    let mut keywords = Vec::new();
    let mut actions_list: Vec<String> = Vec::new();

    // Action data: action_id -> (name, exec)
    let mut actions: HashMap<String, (Option<String>, Option<String>)> = HashMap::new();
    let mut current_section = String::new();
    let mut current_action_id: Option<String> = None;

    for line in content.lines() {
        let line = line.trim();

        if line.starts_with('[') && line.ends_with(']') {
            current_section = line[1..line.len()-1].to_string();
            if current_section.starts_with("Desktop Action ") {
                current_action_id = Some(current_section["Desktop Action ".len()..].to_string());
            } else {
                current_action_id = None;
            }
            continue;
        }

        if let Some((key, value)) = line.split_once('=') {
            if current_section == "Desktop Entry" {
                match key {
                    "Name" if main_name.is_none() => main_name = Some(value.to_string()),
                    "Exec" => main_exec = Some(value.to_string()),
                    "Icon" => main_icon_name = Some(value.to_string()),
                    "NoDisplay" => no_display = value == "true",
                    "Hidden" => hidden = value == "true",
                    "Terminal" => terminal = value == "true",
                    "Keywords" => {
                        keywords = value.split(';').filter(|s| !s.is_empty()).map(|s| s.to_string()).collect();
                    }
                    "Actions" => {
                        actions_list = value.split(';').filter(|s| !s.is_empty()).map(|s| s.to_string()).collect();
                    }
                    _ => {}
                }
            } else if let Some(ref action_id) = current_action_id {
                let entry = actions.entry(action_id.clone()).or_insert((None, None));
                match key {
                    "Name" => entry.0 = Some(value.to_string()),
                    "Exec" => entry.1 = Some(value.to_string()),
                    _ => {}
                }
            }
        }
    }

    if no_display || hidden {
        return vec![];
    }

    let icon = main_icon_name.as_ref().and_then(|name| {
        let path = PathBuf::from(name);
        if path.is_absolute() && path.exists() {
            return Some(path);
        }
        icon_index.get(name).cloned()
    });

    // Add main entry
    if let Some(name) = main_name.clone() {
        if main_exec.is_some() {
            entries.push(Entry::Desktop {
                name,
                desktop_file: path.clone(),
                action: None,
                exec: main_exec.clone(),
                terminal,
                icon: icon.clone(),
                keywords: keywords.clone(),
            });
        }
    }

    // Add action entries
    for action_id in actions_list {
        if let Some((Some(action_name), Some(action_exec))) = actions.get(&action_id) {
            let display_name = if let Some(ref app_name) = main_name {
                format!("{}: {}", app_name, action_name)
            } else {
                action_name.clone()
            };
            entries.push(Entry::Desktop {
                name: display_name,
                desktop_file: path.clone(),
                action: Some(action_id.clone()),
                exec: Some(action_exec.clone()),
                terminal,
                icon: icon.clone(),
                keywords: vec![],
            });
        }
    }

    entries
}
