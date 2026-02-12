use std::path::PathBuf;
use std::sync::mpsc;

use anyhow::Result;
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};

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
                    // Only care about .jsonl files
                    if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
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
