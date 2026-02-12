use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::Deserialize;

use crate::message::{self, SessionMessage};

/// Threshold in seconds for considering a session "active"
const ACTIVE_THRESHOLD_SECS: u64 = 300; // 5 minutes

#[derive(Debug, Clone)]
pub struct Session {
    pub id: String,
    pub project_slug: String,
    pub slug: Option<String>,
    pub custom_title: Option<String>,
    pub summary: Option<String>,
    pub git_branch: Option<String>,
    pub cwd: Option<String>,
    pub messages: Vec<SessionMessage>,
    pub last_activity: DateTime<Utc>,
    pub file_offset: u64,
    pub file_path: PathBuf,
    pub total_tokens_in: u64,
    pub total_tokens_out: u64,
}

impl Session {
    pub fn display_name(&self) -> String {
        // Priority: customTitle > slug > summary > short session id
        let name = self
            .custom_title
            .as_deref()
            .or(self.slug.as_deref())
            .or(self.summary.as_deref())
            .unwrap_or(self.short_id());
        if let Some(ref branch) = self.git_branch {
            format!("{} ({})", name, branch)
        } else {
            name.to_string()
        }
    }


    pub fn short_id(&self) -> &str {
        &self.id[..8.min(self.id.len())]
    }

    pub fn is_active(&self) -> bool {
        let mtime = fs::metadata(&self.file_path)
            .and_then(|m| m.modified())
            .unwrap_or(SystemTime::UNIX_EPOCH);
        let elapsed = SystemTime::now()
            .duration_since(mtime)
            .unwrap_or_default();
        elapsed.as_secs() < ACTIVE_THRESHOLD_SECS
    }
}

// sessions-index.json structures

#[derive(Debug, Deserialize)]
struct SessionsIndex {
    entries: Vec<IndexEntry>,
}

#[derive(Debug, Deserialize)]
struct IndexEntry {
    #[serde(rename = "sessionId")]
    session_id: String,
    #[serde(rename = "customTitle")]
    custom_title: Option<String>,
    summary: Option<String>,
    #[serde(rename = "gitBranch")]
    git_branch: Option<String>,
    #[serde(rename = "projectPath")]
    project_path: Option<String>,
}

/// Load sessions-index.json metadata for a project directory
fn load_sessions_index(project_dir: &Path) -> HashMap<String, IndexEntry> {
    let index_path = project_dir.join("sessions-index.json");
    let mut map = HashMap::new();
    if let Ok(data) = fs::read_to_string(&index_path) {
        if let Ok(index) = serde_json::from_str::<SessionsIndex>(&data) {
            for entry in index.entries {
                map.insert(entry.session_id.clone(), entry);
            }
        }
    }
    map
}

/// Discover all sessions from ~/.claude/projects/
pub fn discover_sessions(base_path: &Path) -> Result<HashMap<String, Session>> {
    let mut sessions = HashMap::new();

    if !base_path.exists() {
        return Ok(sessions);
    }

    for project_entry in fs::read_dir(base_path)? {
        let project_entry = project_entry?;
        let project_path = project_entry.path();
        if !project_path.is_dir() {
            continue;
        }

        let project_slug = project_entry
            .file_name()
            .to_string_lossy()
            .to_string();

        // Load sessions-index.json for this project
        let index = load_sessions_index(&project_path);

        // Find all .jsonl files in this project directory
        for file_entry in fs::read_dir(&project_path)? {
            let file_entry = file_entry?;
            let file_path = file_entry.path();
            if file_path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }

            let session_id = file_path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("unknown")
                .to_string();

            // Skip subagent files
            if session_id.starts_with("agent-") {
                continue;
            }

            let index_entry = index.get(&session_id);

            match parse_session_file(&file_path, &project_slug, index_entry) {
                Ok(session) => {
                    sessions.insert(session.id.clone(), session);
                }
                Err(_) => {
                    sessions.insert(
                        session_id.clone(),
                        Session {
                            id: session_id,
                            project_slug: project_slug.clone(),
                            slug: None,
                            custom_title: index_entry.and_then(|e| e.custom_title.clone()),
                            summary: index_entry.and_then(|e| e.summary.clone()),
                            git_branch: index_entry.and_then(|e| e.git_branch.clone()),
                            cwd: index_entry.and_then(|e| e.project_path.clone()),
                            messages: Vec::new(),
                            last_activity: Utc::now(),
                            file_offset: 0,
                            file_path,
                            total_tokens_in: 0,
                            total_tokens_out: 0,
                        },
                    );
                }
            }
        }
    }

    Ok(sessions)
}

fn parse_session_file(
    file_path: &Path,
    project_slug: &str,
    index_entry: Option<&IndexEntry>,
) -> Result<Session> {
    let file = fs::File::open(file_path)?;
    let file_len = file.metadata()?.len();
    let reader = BufReader::new(file);

    let session_id = file_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown")
        .to_string();

    let mut messages = Vec::new();
    let mut git_branch = index_entry.and_then(|e| e.git_branch.clone());
    let mut cwd = index_entry.and_then(|e| e.project_path.clone());
    let mut slug: Option<String> = None;
    let mut last_activity = Utc::now();
    let mut total_tokens_in: u64 = 0;
    let mut total_tokens_out: u64 = 0;
    for line in reader.lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => continue,
        };
        if line.trim().is_empty() {
            continue;
        }

        // Keep extracting metadata until we have all fields
        if git_branch.is_none() || cwd.is_none() || slug.is_none() {
            if let Some(meta) = message::extract_meta(&line) {
                if git_branch.is_none() {
                    git_branch = meta.git_branch;
                }
                if cwd.is_none() {
                    cwd = meta.cwd;
                }
                if slug.is_none() {
                    slug = meta.slug;
                }
            }
        }

        if let Some(msg) = message::parse_line(&line) {
            last_activity = msg.timestamp;
            if let Some(tin) = msg.tokens_in {
                total_tokens_in += tin;
            }
            if let Some(tout) = msg.tokens_out {
                total_tokens_out += tout;
            }
            messages.push(msg);
        }
    }

    Ok(Session {
        id: session_id,
        project_slug: project_slug.to_string(),
        slug,
        custom_title: index_entry.and_then(|e| e.custom_title.clone()),
        summary: index_entry.and_then(|e| e.summary.clone()),
        git_branch,
        cwd,
        messages,
        last_activity,
        file_offset: file_len,
        file_path: file_path.to_path_buf(),
        total_tokens_in,
        total_tokens_out,
    })
}

/// Read new lines from a session file starting at the given offset
pub fn read_new_lines(session: &mut Session) -> Result<Vec<SessionMessage>> {
    let file = fs::File::open(&session.file_path)?;
    let file_len = file.metadata()?.len();

    if file_len <= session.file_offset {
        return Ok(Vec::new());
    }

    let mut reader = BufReader::new(file);
    reader.seek(SeekFrom::Start(session.file_offset))?;

    let mut new_messages = Vec::new();

    for line in reader.lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => continue,
        };
        if line.trim().is_empty() {
            continue;
        }

        // Update metadata if not yet set
        if session.git_branch.is_none() || session.cwd.is_none() || session.slug.is_none() {
            if let Some(meta) = message::extract_meta(&line) {
                if session.git_branch.is_none() {
                    session.git_branch = meta.git_branch;
                }
                if session.cwd.is_none() {
                    session.cwd = meta.cwd;
                }
                if session.slug.is_none() {
                    session.slug = meta.slug;
                }
            }
        }

        if let Some(msg) = message::parse_line(&line) {
            session.last_activity = msg.timestamp;
            if let Some(tin) = msg.tokens_in {
                session.total_tokens_in += tin;
            }
            if let Some(tout) = msg.tokens_out {
                session.total_tokens_out += tout;
            }
            new_messages.push(msg.clone());
            session.messages.push(msg);
        }
    }

    session.file_offset = file_len;
    Ok(new_messages)
}

/// Discover a single new session from a JSONL file path
pub fn discover_single_session(file_path: &Path) -> Result<Option<Session>> {
    if file_path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
        return Ok(None);
    }

    let project_dir = file_path.parent().unwrap_or(Path::new(""));
    let project_slug = project_dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown")
        .to_string();

    let session_id = file_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown")
        .to_string();

    let index = load_sessions_index(project_dir);
    let index_entry = index.get(&session_id);

    let session = parse_session_file(file_path, &project_slug, index_entry)?;
    Ok(Some(session))
}

/// Re-read sessions-index.json and update session metadata (names, titles)
pub fn refresh_index_metadata(base_path: &Path, sessions: &mut HashMap<String, Session>) {
    if !base_path.exists() {
        return;
    }
    let Ok(entries) = fs::read_dir(base_path) else {
        return;
    };
    for project_entry in entries.flatten() {
        let project_path = project_entry.path();
        if !project_path.is_dir() {
            continue;
        }
        let index = load_sessions_index(&project_path);
        for (session_id, entry) in &index {
            if let Some(session) = sessions.get_mut(session_id) {
                if entry.custom_title.is_some() {
                    session.custom_title = entry.custom_title.clone();
                }
                if entry.summary.is_some() {
                    session.summary = entry.summary.clone();
                }
            }
        }
    }
}
