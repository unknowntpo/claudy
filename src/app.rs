use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyModifiers, MouseEventKind};
use ratatui::layout::Rect;
use ratatui::widgets::ListState;

use crate::session::{self, Session};
use crate::watcher::{SessionWatcher, WatchEvent};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FocusPanel {
    Sessions,
    Chat,
}

pub struct App {
    pub sessions: HashMap<String, Session>,
    pub sorted_session_ids: Vec<String>,
    pub selected_session: Option<String>,
    pub list_state: ListState,
    pub chat_scroll: usize,
    pub chat_total_lines: usize,
    pub chat_scroll_locked_to_bottom: bool,
    pub filter_mode: bool,
    pub filter_text: Option<String>,
    pub show_active_only: bool,
    pub focus: FocusPanel,
    pub should_quit: bool,
    pub base_path: PathBuf,
    pub watcher: Option<SessionWatcher>,
    /// Stored layout rects for mouse hit testing
    pub session_list_area: Rect,
    pub chat_area: Rect,
    last_index_refresh: Instant,
}

impl App {
    pub fn new(base_path: PathBuf) -> Result<Self> {
        let sessions = session::discover_sessions(&base_path)?;
        let sorted_ids = sort_session_ids(&sessions);

        let selected = sorted_ids.first().cloned();
        let mut list_state = ListState::default();
        if !sorted_ids.is_empty() {
            list_state.select(Some(0));
        }

        // Start file watcher
        let watcher = SessionWatcher::new(base_path.clone()).ok();

        Ok(Self {
            sessions,
            sorted_session_ids: sorted_ids,
            selected_session: selected,
            list_state,
            chat_scroll: 0,
            chat_total_lines: 0,
            chat_scroll_locked_to_bottom: true,
            filter_mode: false,
            filter_text: None,
            show_active_only: false,
            focus: FocusPanel::Sessions,
            should_quit: false,
            base_path,
            watcher,
            session_list_area: Rect::default(),
            chat_area: Rect::default(),
            last_index_refresh: Instant::now(),
        })
    }

    pub fn tick(&mut self) {
        // Process file watcher events
        if let Some(ref watcher) = self.watcher {
            let events = watcher.poll();
            for evt in events {
                match evt {
                    WatchEvent::FileModified(path) => {
                        self.handle_file_modified(&path);
                    }
                    WatchEvent::FileCreated(path) => {
                        self.handle_file_created(&path);
                    }
                }
            }
        }

        // Periodically refresh sessions-index.json metadata (every 10s)
        if self.last_index_refresh.elapsed() >= Duration::from_secs(10) {
            session::refresh_index_metadata(&self.base_path, &mut self.sessions);
            self.last_index_refresh = Instant::now();
            self.update_sort();
        }
    }

    fn handle_file_modified(&mut self, path: &Path) {
        // Check if sessions-index.json changed
        if path.file_name().and_then(|n| n.to_str()) == Some("sessions-index.json") {
            session::refresh_index_metadata(&self.base_path, &mut self.sessions);
            self.update_sort();
            return;
        }

        let session_id = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();

        if let Some(session) = self.sessions.get_mut(&session_id) {
            let _ = session::read_new_lines(session);
            // Auto-scroll to bottom when chat pane is focused and viewing this session
            if self.focus == FocusPanel::Chat
                && self.selected_session.as_deref() == Some(&session_id)
            {
                self.chat_scroll_locked_to_bottom = true;
            }
            self.update_sort();
        } else {
            self.handle_file_created(path);
        }
    }

    fn handle_file_created(&mut self, path: &Path) {
        if let Ok(Some(session)) = session::discover_single_session(path) {
            let id = session.id.clone();
            self.sessions.insert(id, session);
            self.update_sort();
        }
    }

    fn update_sort(&mut self) {
        let old_selected = self.selected_session.clone();
        self.sorted_session_ids = sort_session_ids(&self.sessions);

        // Deduplicate sessions with the same slug (keep the most recent).
        // Before discarding duplicates, merge custom_title into the kept session.
        {
            let mut seen_slugs: std::collections::HashMap<String, String> =
                std::collections::HashMap::new(); // slug -> kept session id
            self.sorted_session_ids.retain(|id| {
                if let Some(session) = self.sessions.get(id)
                    && let Some(ref slug) = session.slug
                {
                    if let Some(kept_id) = seen_slugs.get(slug) {
                        // Duplicate: merge custom_title into the kept session
                        if session.custom_title.is_some() {
                            let ct = session.custom_title.clone();
                            if let Some(kept) = self.sessions.get_mut(kept_id)
                                && kept.custom_title.is_none()
                            {
                                kept.custom_title = ct;
                            }
                        }
                        return false;
                    }
                    seen_slugs.insert(slug.clone(), id.clone());
                }
                true
            });
        }

        // Apply active filter
        if self.show_active_only {
            self.sorted_session_ids.retain(|id| {
                self.sessions
                    .get(id)
                    .map(|s| s.is_active())
                    .unwrap_or(false)
            });
        }

        // Apply text filter
        if let Some(ref filter) = self.filter_text {
            let filter_lower = filter.to_lowercase();
            self.sorted_session_ids.retain(|id| {
                if let Some(s) = self.sessions.get(id) {
                    s.display_name().to_lowercase().contains(&filter_lower)
                        || s.id.contains(&filter_lower)
                        || s.summary
                            .as_deref()
                            .unwrap_or("")
                            .to_lowercase()
                            .contains(&filter_lower)
                } else {
                    false
                }
            });
        }

        // Restore selection
        if let Some(ref sel) = old_selected {
            if let Some(idx) = self.sorted_session_ids.iter().position(|id| id == sel) {
                self.list_state.select(Some(idx));
            } else if !self.sorted_session_ids.is_empty() {
                self.list_state.select(Some(0));
                self.selected_session = self.sorted_session_ids.first().cloned();
            } else {
                self.list_state.select(None);
                self.selected_session = None;
            }
        }
    }

    pub fn handle_key_event(&mut self, key: event::KeyEvent) {
        if self.filter_mode {
            self.handle_filter_key(key);
            return;
        }

        match key.code {
            KeyCode::Char('q') => self.should_quit = true,
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.should_quit = true;
            }
            KeyCode::Tab => {
                self.focus = match self.focus {
                    FocusPanel::Sessions => FocusPanel::Chat,
                    FocusPanel::Chat => FocusPanel::Sessions,
                };
                Self::drain_events();
            }
            KeyCode::Char('j') | KeyCode::Down => match self.focus {
                FocusPanel::Sessions => self.move_selection(1),
                FocusPanel::Chat => self.scroll_chat_down(3),
            },
            KeyCode::Char('k') | KeyCode::Up => match self.focus {
                FocusPanel::Sessions => self.move_selection(-1),
                FocusPanel::Chat => self.scroll_chat_up(3),
            },
            KeyCode::Enter => self.select_current(),
            KeyCode::Char('r') => self.refresh_all(),
            KeyCode::Char('a') => {
                self.show_active_only = !self.show_active_only;
                self.update_sort();
            }
            KeyCode::Char('/') => {
                self.filter_mode = true;
                self.filter_text = Some(String::new());
            }
            KeyCode::Char('G') => {
                self.chat_scroll_locked_to_bottom = true;
            }
            KeyCode::Char('g') => {
                self.chat_scroll = 0;
                self.chat_scroll_locked_to_bottom = false;
            }
            KeyCode::PageDown => self.scroll_chat_down(20),
            KeyCode::PageUp => self.scroll_chat_up(20),
            _ => {}
        }
    }

    fn handle_filter_key(&mut self, key: event::KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.filter_mode = false;
                self.filter_text = None;
                self.update_sort();
            }
            KeyCode::Enter => {
                self.filter_mode = false;
                self.update_sort();
            }
            KeyCode::Backspace => {
                if let Some(ref mut text) = self.filter_text {
                    text.pop();
                    if text.is_empty() {
                        self.filter_text = None;
                    }
                }
                self.update_sort();
            }
            KeyCode::Char(c) => {
                if let Some(ref mut text) = self.filter_text {
                    text.push(c);
                } else {
                    self.filter_text = Some(c.to_string());
                }
                self.update_sort();
            }
            _ => {}
        }
    }

    fn move_selection(&mut self, delta: i32) {
        let len = self.sorted_session_ids.len();
        if len == 0 {
            return;
        }
        let current = self.list_state.selected().unwrap_or(0);
        let new_idx = if delta > 0 {
            (current + delta as usize).min(len - 1)
        } else {
            current.saturating_sub((-delta) as usize)
        };
        self.list_state.select(Some(new_idx));
        self.selected_session = self.sorted_session_ids.get(new_idx).cloned();
        self.chat_scroll_locked_to_bottom = true;
    }

    fn select_current(&mut self) {
        if let Some(idx) = self.list_state.selected() {
            self.selected_session = self.sorted_session_ids.get(idx).cloned();
            self.chat_scroll_locked_to_bottom = true;
            self.focus = FocusPanel::Chat;
        }
    }

    fn scroll_chat_down(&mut self, amount: usize) {
        self.chat_scroll = self.chat_scroll.saturating_add(amount);
        self.chat_scroll_locked_to_bottom = false;
    }

    fn scroll_chat_up(&mut self, amount: usize) {
        self.chat_scroll = self.chat_scroll.saturating_sub(amount);
        self.chat_scroll_locked_to_bottom = false;
    }

    pub fn handle_mouse_event(&mut self, mouse: event::MouseEvent) {
        let x = mouse.column;
        let y = mouse.row;

        match mouse.kind {
            MouseEventKind::Down(_) => {
                let old_focus = self.focus;
                if self.rect_contains(self.session_list_area, x, y) {
                    self.focus = FocusPanel::Sessions;
                } else if self.rect_contains(self.chat_area, x, y) {
                    self.focus = FocusPanel::Chat;
                }
                if self.focus != old_focus {
                    Self::drain_events();
                }
            }
            MouseEventKind::ScrollDown => {
                // Only scroll if this panel is focused (prevents scroll leaking)
                if self.rect_contains(self.chat_area, x, y) && self.focus == FocusPanel::Chat {
                    self.scroll_chat_down(3);
                } else if self.rect_contains(self.session_list_area, x, y)
                    && self.focus == FocusPanel::Sessions
                {
                    self.move_selection(1);
                }
            }
            MouseEventKind::ScrollUp => {
                if self.rect_contains(self.chat_area, x, y) && self.focus == FocusPanel::Chat {
                    self.scroll_chat_up(3);
                } else if self.rect_contains(self.session_list_area, x, y)
                    && self.focus == FocusPanel::Sessions
                {
                    self.move_selection(-1);
                }
            }
            _ => {}
        }
    }

    fn rect_contains(&self, rect: Rect, x: u16, y: u16) -> bool {
        x >= rect.x && x < rect.x + rect.width && y >= rect.y && y < rect.y + rect.height
    }

    /// Drain all pending input events to cancel queued scrolls on focus change
    fn drain_events() {
        while event::poll(Duration::from_millis(0)).unwrap_or(false) {
            let _ = event::read();
        }
    }

    fn refresh_all(&mut self) {
        if let Ok(sessions) = session::discover_sessions(&self.base_path) {
            self.sessions = sessions;
            self.update_sort();
        }
    }

    pub fn run_event_loop(&mut self, terminal: &mut ratatui::DefaultTerminal) -> Result<()> {
        // Enable mouse capture
        crossterm::execute!(std::io::stdout(), crossterm::event::EnableMouseCapture)?;

        let tick_rate = Duration::from_millis(250);
        let mut last_tick = Instant::now();

        loop {
            terminal.draw(|f| crate::ui::draw(f, self))?;

            // Wait for at least one event, then batch-process ALL pending
            // events before next redraw. This ensures focus changes (click/Tab)
            // take effect immediately even with many queued scroll events.
            let timeout = tick_rate.saturating_sub(last_tick.elapsed());
            if event::poll(timeout)? {
                loop {
                    match event::read()? {
                        Event::Key(key) => self.handle_key_event(key),
                        Event::Mouse(mouse) => self.handle_mouse_event(mouse),
                        _ => {}
                    }
                    if self.should_quit {
                        break;
                    }
                    // Keep reading if more events available (no wait)
                    if !event::poll(Duration::ZERO)? {
                        break;
                    }
                }
            }

            if last_tick.elapsed() >= tick_rate {
                self.tick();
                last_tick = Instant::now();
            }

            if self.should_quit {
                break;
            }
        }

        crossterm::execute!(std::io::stdout(), crossterm::event::DisableMouseCapture)?;
        Ok(())
    }
}

fn sort_session_ids(sessions: &HashMap<String, Session>) -> Vec<String> {
    let mut ids: Vec<String> = sessions.keys().cloned().collect();
    ids.sort_by(|a, b| {
        let sa = sessions.get(a);
        let sb = sessions.get(b);
        match (sa, sb) {
            (Some(a), Some(b)) => b.last_activity.cmp(&a.last_activity),
            _ => std::cmp::Ordering::Equal,
        }
    });
    ids
}
