use std::path::PathBuf;
use std::sync::mpsc;

use anyhow::Result;
use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};

pub enum WatchEvent {
    FileModified(PathBuf),
    FileCreated(PathBuf),
}

pub struct SessionWatcher {
    _watcher: RecommendedWatcher,
    pub rx: mpsc::Receiver<WatchEvent>,
}

impl SessionWatcher {
    pub fn new(watch_path: PathBuf) -> Result<Self> {
        let (tx, rx) = mpsc::channel();

        let mut watcher = notify::recommended_watcher(move |res: Result<Event, notify::Error>| {
            if let Ok(event) = res {
                for path in &event.paths {
                    let ext = path.extension().and_then(|e| e.to_str());
                    let fname = path.file_name().and_then(|n| n.to_str());

                    // Only care about .jsonl files and sessions-index.json
                    let dominated = ext == Some("jsonl") || fname == Some("sessions-index.json");
                    if !dominated {
                        continue;
                    }

                    let watch_event = match event.kind {
                        EventKind::Modify(_) => Some(WatchEvent::FileModified(path.clone())),
                        EventKind::Create(_) => Some(WatchEvent::FileCreated(path.clone())),
                        _ => None,
                    };
                    if let Some(evt) = watch_event {
                        let _ = tx.send(evt);
                    }
                }
            }
        })?;

        // Reduce poll interval for lower latency on macOS FSEvents
        watcher
            .configure(Config::default().with_poll_interval(std::time::Duration::from_secs(1)))?;
        watcher.watch(&watch_path, RecursiveMode::Recursive)?;

        Ok(Self {
            _watcher: watcher,
            rx,
        })
    }

    /// Non-blocking poll for watch events
    pub fn poll(&self) -> Vec<WatchEvent> {
        let mut events = Vec::new();
        while let Ok(evt) = self.rx.try_recv() {
            events.push(evt);
        }
        events
    }
}
