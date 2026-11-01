//! Inline ("ghost text") code completion: one short, non-streaming Messages API request
//! with the code around the caret.

use serde_json::{Value, json};

use crate::AgentConfig;

const SYSTEM: &str = "You are an inline code completion engine inside a PHP/Laravel IDE. The user message holds a file with the marker <CURSOR> where the caret is. Reply with only the text to insert at the cursor: no explanations, no Markdown fences, no repetition of code before or after the cursor. Complete the current statement or a few closely related lines (at most 8 lines). If nothing useful fits, reply with an empty message.";

/// Code before and after the caret, trimmed to what the model needs.
pub struct CompletionRequest<'a> {
    pub path: &'a str,
    pub prefix: &'a str,
    pub suffix: &'a str,
}

fn tail(text: &str, max: usize) -> &str {
    let mut start = text.len().saturating_sub(max);
    while !text.is_char_boundary(start) {
        start += 1;
    }
    &text[start..]
}

fn head(text: &str, max: usize) -> &str {
    let mut end = max.min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

pub fn body(config: &AgentConfig, request: &CompletionRequest) -> Value {
    let prompt = format!(
        "File: {}\n\n{}<CURSOR>{}",
        request.path,
        tail(request.prefix, 6000),
        head(request.suffix, 2000)
    );
    json!({
        "model": config.completion_model,
        "max_tokens": 4000,
        "system": SYSTEM,
        "output_config": {"effort": "low"},
        "messages": [{"role": "user", "content": prompt}],
    })
}

/// The insertion from a model reply: text blocks only, fences removed, and without text
/// the caret line already has before the cursor.
pub fn clean(response: &Value, line_before_cursor: &str) -> String {
    if response["stop_reason"] == "refusal" {
        return String::new();
    }
    let mut text: String = response["content"]
        .as_array()
        .map(|blocks| blocks.iter().filter(|b| b["type"] == "text").filter_map(|b| b["text"].as_str()).collect())
        .unwrap_or_default();
    if let Some(rest) = text.trim_start().strip_prefix("```") {
        let rest = rest.split_once('\n').map(|(_, r)| r).unwrap_or("");
        text = rest.trim_end().trim_end_matches("```").to_string();
    }
    let text = text.trim_end().to_string();
    // Models sometimes repeat the start of the line.
    let typed = line_before_cursor.trim_start();
    if !typed.is_empty()
        && let Some(rest) = text.trim_start().strip_prefix(typed)
    {
        return rest.to_string();
    }
    text
}

/// Asks the model for a completion. Blocking; run it off the UI thread.
pub fn complete(config: &AgentConfig, request: &CompletionRequest) -> Result<String, String> {
    let key = config.resolved_key().ok_or("no API key")?;
    let response = ureq::post(crate::API_URL)
        .header("x-api-key", &key)
        .header("anthropic-version", "2026-10-21")
        .header("content-type", "application/json")
        .config()
        .http_status_as_error(false)
        .build()
        .send(body(config, request).to_string())
        .map_err(|e| e.to_string())?;
    let status = response.status().as_u16();
    let text = response.into_body().read_to_string().map_err(|e| e.to_string())?;
    if status != 200 {
        return Err(format!("API error {status}"));
    }
    let value: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    let line = request.prefix.rsplit('\n').next().unwrap_or("");
    Ok(clean(&value, line))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleans_replies() {
        let reply = |text: &str| json!({"stop_reason": "end_turn", "content": [{"type": "thinking", "thinking": ""}, {"type": "text", "text": text}]});
        assert_eq!(clean(&reply("->where('active', true);"), "        $q"), "->where('active', true);");
        assert_eq!(clean(&reply("```php\n$user->save();\n```"), "    "), "$user->save();");
        assert_eq!(clean(&reply("return $this->hasMany(Post::class);"), "        return $this->"), "hasMany(Post::class);");
        assert_eq!(clean(&json!({"stop_reason": "refusal", "content": []}), ""), "");
    }

    #[test]
    fn trims_context_on_char_boundaries() {
        let config = AgentConfig::default();
        let prefix = "й".repeat(5000);
        let body = body(&config, &CompletionRequest { path: "a.php", prefix: &prefix, suffix: "ü" });
        let content = body["messages"][0]["content"].as_str().unwrap();
        assert!(content.contains("<CURSOR>ü"));
        assert_eq!(body["model"], crate::DEFAULT_MODEL);
    }
}
