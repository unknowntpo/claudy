use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use ratatui::widgets::ListState;

use crate::session::{self, Session};
use crate::watcher::{SessionWatcher, WatchEvent};

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
    pub should_quit: bool,
    pub base_path: PathBuf,
    pub watcher: Option<SessionWatcher>,
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
            should_quit: false,
            base_path,
            watcher,
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

    fn handle_file_modified(&mut self, path: &PathBuf) {
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
            self.update_sort();
        } else {
            self.handle_file_created(path);
        }
    }

    fn handle_file_created(&mut self, path: &PathBuf) {
        if let Ok(Some(session)) = session::discover_single_session(path) {
            let id = session.id.clone();
            self.sessions.insert(id, session);
            self.update_sort();
        }
    }

    fn update_sort(&mut self) {
        let old_selected = self.selected_session.clone();
        self.sorted_session_ids = sort_session_ids(&self.sessions);

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
            KeyCode::Char('j') | KeyCode::Down => self.move_selection(1),
            KeyCode::Char('k') | KeyCode::Up => self.move_selection(-1),
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
            KeyCode::PageDown => {
                self.chat_scroll = self.chat_scroll.saturating_add(20);
                self.chat_scroll_locked_to_bottom = false;
            }
            KeyCode::PageUp => {
                self.chat_scroll = self.chat_scroll.saturating_sub(20);
                self.chat_scroll_locked_to_bottom = false;
            }
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
        }
    }

    fn refresh_all(&mut self) {
        if let Ok(sessions) = session::discover_sessions(&self.base_path) {
            self.sessions = sessions;
            self.update_sort();
        }
    }

    pub fn run_event_loop(&mut self, terminal: &mut ratatui::DefaultTerminal) -> Result<()> {
        let tick_rate = Duration::from_millis(250);
        let mut last_tick = Instant::now();

        loop {
            terminal.draw(|f| crate::ui::draw(f, self))?;

            let timeout = tick_rate.saturating_sub(last_tick.elapsed());
            if event::poll(timeout)? {
                if let Event::Key(key) = event::read()? {
                    self.handle_key_event(key);
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
