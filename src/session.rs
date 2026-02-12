use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use anyhow::Result;
use chrono::{DateTime, Utc};

use crate::message::{self, SessionMessage};

#[derive(Debug, Clone)]
pub struct Session {
    pub id: String,
    pub project_slug: String,
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
        if let Some(ref branch) = self.git_branch {
            format!("{} ({})", self.project_slug, branch)
        } else {
            self.project_slug.clone()
        }
    }

    pub fn short_id(&self) -> &str {
        &self.id[..8.min(self.id.len())]
    }
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

            match parse_session_file(&file_path, &project_slug) {
                Ok(session) => {
                    sessions.insert(session.id.clone(), session);
                }
                Err(e) => {
                    eprintln!("Warning: failed to parse {}: {}", file_path.display(), e);
                    // Create a minimal session entry anyway
                    sessions.insert(
                        session_id.clone(),
                        Session {
                            id: session_id,
                            project_slug: project_slug.clone(),
                            git_branch: None,
                            cwd: None,
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

fn parse_session_file(file_path: &Path, project_slug: &str) -> Result<Session> {
    let file = fs::File::open(file_path)?;
    let file_len = file.metadata()?.len();
    let reader = BufReader::new(file);

    let session_id = file_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown")
        .to_string();

    let mut messages = Vec::new();
    let mut git_branch = None;
    let mut cwd = None;
    let mut last_activity = Utc::now();
    let mut total_tokens_in: u64 = 0;
    let mut total_tokens_out: u64 = 0;
    let mut meta_extracted = false;

    for line in reader.lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => continue,
        };
        if line.trim().is_empty() {
            continue;
        }

        // Extract metadata from early messages
        if !meta_extracted {
            if let Some(meta) = message::extract_meta(&line) {
                if git_branch.is_none() {
                    git_branch = meta.git_branch;
                }
                if cwd.is_none() {
                    cwd = meta.cwd;
                }
                meta_extracted = true;
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
        if session.git_branch.is_none() || session.cwd.is_none() {
            if let Some(meta) = message::extract_meta(&line) {
                if session.git_branch.is_none() {
                    session.git_branch = meta.git_branch;
                }
                if session.cwd.is_none() {
                    session.cwd = meta.cwd;
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

    let project_slug = file_path
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .unwrap_or("unknown")
        .to_string();

    let session = parse_session_file(file_path, &project_slug)?;
    Ok(Some(session))
}
