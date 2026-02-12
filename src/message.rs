use chrono::{DateTime, Utc};
use serde::Deserialize;

#[derive(Debug, Clone, PartialEq)]
pub enum MessageType {
    User,
    Assistant,
    Progress,
    ToolUse,
    Other,
}

#[derive(Debug, Clone)]
pub struct SessionMessage {
    pub msg_type: MessageType,
    pub timestamp: DateTime<Utc>,
    pub content: String,
    pub tokens_in: Option<u64>,
    pub tokens_out: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct RawMessage {
    #[serde(rename = "type")]
    pub msg_type: String,
    pub timestamp: Option<String>,
    pub message: Option<RawMessageContent>,
    #[serde(rename = "sessionId")]
    pub session_id: Option<String>,
    #[serde(rename = "gitBranch")]
    pub git_branch: Option<String>,
    pub cwd: Option<String>,
    pub slug: Option<String>,
    /// Present on "type": "summary" lines
    pub summary: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawMessageContent {
    pub content: Option<serde_json::Value>,
    pub usage: Option<RawUsage>,
}

#[derive(Debug, Deserialize)]
struct RawUsage {
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cache_read_input_tokens: Option<u64>,
    pub cache_creation_input_tokens: Option<u64>,
}

/// Extract displayable text content from a message content value
fn extract_text_content(content: &serde_json::Value) -> String {
    match content {
        serde_json::Value::String(s) => {
            // Strip XML-like tags for cleaner display
            let s = s.trim();
            if s.starts_with('<') && s.contains('>') {
                // Try to extract meaningful text from tagged content
                if let Some(idx) = s.rfind('>') {
                    let after = s[idx + 1..].trim();
                    if !after.is_empty() {
                        return after.to_string();
                    }
                }
                // For command-related content, show abbreviated form
                if s.contains("command-name") || s.contains("local-command") {
                    return "[command]".to_string();
                }
                s.to_string()
            } else {
                s.to_string()
            }
        }
        serde_json::Value::Array(arr) => {
            let mut parts = Vec::new();
            for item in arr {
                if let Some(obj) = item.as_object() {
                    match obj.get("type").and_then(|t| t.as_str()) {
                        Some("text") => {
                            if let Some(text) = obj.get("text").and_then(|t| t.as_str()) {
                                let trimmed = text.trim();
                                if !trimmed.is_empty() {
                                    parts.push(trimmed.to_string());
                                }
                            }
                        }
                        Some("tool_use") => {
                            let name = obj
                                .get("name")
                                .and_then(|n| n.as_str())
                                .unwrap_or("unknown");
                            parts.push(format!("[tool: {}]", name));
                        }
                        Some("tool_result") => {
                            parts.push("[tool result]".to_string());
                        }
                        _ => {}
                    }
                }
            }
            parts.join("\n")
        }
        _ => String::new(),
    }
}

/// Determine if content array contains tool_use blocks
fn has_tool_use(content: &serde_json::Value) -> bool {
    if let Some(arr) = content.as_array() {
        arr.iter().any(|item| {
            item.as_object()
                .and_then(|o| o.get("type"))
                .and_then(|t| t.as_str())
                == Some("tool_use")
        })
    } else {
        false
    }
}

/// Parse a single JSONL line into an optional SessionMessage
pub fn parse_line(line: &str) -> Option<SessionMessage> {
    let raw: RawMessage = serde_json::from_str(line).ok()?;

    let msg_type_str = raw.msg_type.as_str();

    // Skip file-history-snapshot and queue-operation
    match msg_type_str {
        "file-history-snapshot" | "queue-operation" => return None,
        _ => {}
    }

    let timestamp = raw
        .timestamp
        .as_ref()
        .and_then(|ts| DateTime::parse_from_rfc3339(ts).ok())
        .map(|dt| dt.with_timezone(&Utc))
        .unwrap_or_else(Utc::now);

    let (msg_type, content, tokens_in, tokens_out) = match msg_type_str {
        "user" => {
            let content = raw
                .message
                .as_ref()
                .and_then(|m| m.content.as_ref())
                .map(extract_text_content)
                .unwrap_or_default();
            (MessageType::User, content, None, None)
        }
        "assistant" => {
            let msg = raw.message.as_ref();
            let content = msg
                .and_then(|m| m.content.as_ref())
                .map(|c| {
                    if has_tool_use(c) {
                        // Still extract text, but note tool usage
                        extract_text_content(c)
                    } else {
                        extract_text_content(c)
                    }
                })
                .unwrap_or_default();

            let tokens_in = msg.and_then(|m| {
                m.usage.as_ref().map(|u| {
                    u.input_tokens.unwrap_or(0)
                        + u.cache_read_input_tokens.unwrap_or(0)
                        + u.cache_creation_input_tokens.unwrap_or(0)
                })
            });
            let tokens_out = msg.and_then(|m| m.usage.as_ref().and_then(|u| u.output_tokens));

            let actual_type = if msg
                .and_then(|m| m.content.as_ref())
                .map(has_tool_use)
                .unwrap_or(false)
            {
                MessageType::ToolUse
            } else {
                MessageType::Assistant
            };

            (actual_type, content, tokens_in, tokens_out)
        }
        "progress" => {
            let content = "[progress]".to_string();
            (MessageType::Progress, content, None, None)
        }
        _ => {
            let content = format!("[{}]", msg_type_str);
            (MessageType::Other, content, None, None)
        }
    };

    // Skip empty or uninteresting messages
    if content.is_empty() || content == "[command]" {
        return None;
    }

    Some(SessionMessage {
        msg_type,
        timestamp,
        content,
        tokens_in,
        tokens_out,
    })
}

pub struct SessionMeta {
    pub git_branch: Option<String>,
    pub cwd: Option<String>,
    pub slug: Option<String>,
    pub summary: Option<String>,
}

/// Extract metadata from a JSONL line. Works on both regular messages
/// (with sessionId) and "type": "summary" lines.
pub fn extract_meta(line: &str) -> Option<SessionMeta> {
    let raw: RawMessage = serde_json::from_str(line).ok()?;

    // "type": "summary" lines have summary but no sessionId
    if raw.msg_type == "summary" {
        return Some(SessionMeta {
            git_branch: None,
            cwd: None,
            slug: None,
            summary: raw.summary,
        });
    }

    // Regular messages need sessionId
    raw.session_id?;
    Some(SessionMeta {
        git_branch: raw.git_branch,
        cwd: raw.cwd,
        slug: raw.slug,
        summary: None,
    })
}
