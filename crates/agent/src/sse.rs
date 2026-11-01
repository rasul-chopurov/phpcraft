//! Reads one streamed Messages API response (Server-Sent Events) into content blocks,
//! forwarding text and thinking deltas as they arrive.

use std::io::BufRead;
use std::sync::mpsc;

use serde_json::Value;

use crate::AgentEvent;

pub struct Message {
    /// Content blocks as the API sent them, ready to be replayed in the history.
    pub content: Vec<Value>,
    pub stop_reason: Option<String>,
}

pub fn read_message(reader: impl BufRead, events: &mpsc::Sender<AgentEvent>, cancelled: &dyn Fn() -> bool) -> Result<Message, String> {
    let mut content: Vec<Value> = Vec::new();
    // Tool input JSON arrives in pieces per block.
    let mut partial_json: Vec<String> = Vec::new();
    let mut stop_reason = None;
    for line in reader.lines() {
        if cancelled() {
            break;
        }
        let line = line.map_err(|e| format!("Stream error: {e}"))?;
        let Some(data) = line.strip_prefix("data:") else { continue };
        let Ok(event) = serde_json::from_str::<Value>(data.trim()) else { continue };
        match event["type"].as_str().unwrap_or("") {
            "content_block_start" => {
                let index = event["index"].as_u64().unwrap_or(0) as usize;
                if content.len() <= index {
                    content.resize(index + 1, Value::Null);
                    partial_json.resize(index + 1, String::new());
                }
                content[index] = event["content_block"].clone();
            }
            "content_block_delta" => {
                let index = event["index"].as_u64().unwrap_or(0) as usize;
                let Some(block) = content.get_mut(index) else { continue };
                let delta = &event["delta"];
                match delta["type"].as_str().unwrap_or("") {
                    "text_delta" => {
                        let text = delta["text"].as_str().unwrap_or("");
                        append(block, "text", text);
                        let _ = events.send(AgentEvent::Text(text.to_string()));
                    }
                    "thinking_delta" => {
                        let text = delta["thinking"].as_str().unwrap_or("");
                        append(block, "thinking", text);
                        let _ = events.send(AgentEvent::Thinking(text.to_string()));
                    }
                    "signature_delta" => block["signature"] = delta["signature"].clone(),
                    "input_json_delta" => partial_json[index].push_str(delta["partial_json"].as_str().unwrap_or("")),
                    _ => {}
                }
            }
            "content_block_stop" => {
                let index = event["index"].as_u64().unwrap_or(0) as usize;
                if let Some(block) = content.get_mut(index)
                    && block["type"] == "tool_use"
                {
                    let raw = &partial_json[index];
                    // Eager input streaming: the API doesn't validate the JSON, so parse it
                    // here; a cut-off or invalid input becomes a string the loop rejects.
                    block["input"] = if raw.trim().is_empty() {
                        Value::Object(Default::default())
                    } else {
                        serde_json::from_str(raw).unwrap_or(Value::String(raw.clone()))
                    };
                }
            }
            "message_delta" => {
                if let Some(reason) = event["delta"]["stop_reason"].as_str() {
                    stop_reason = Some(reason.to_string());
                }
            }
            "error" => return Err(format!("API error: {}", event["error"]["message"].as_str().unwrap_or("unknown"))),
            _ => {}
        }
    }
    content.retain(|b| !b.is_null());
    Ok(Message { content, stop_reason })
}

fn append(block: &mut Value, key: &str, text: &str) {
    let current = block[key].as_str().unwrap_or("").to_string();
    block[key] = Value::String(current + text);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assembles_blocks() {
        let stream = r#"event: message_start
data: {"type":"message_start","message":{"id":"msg_1"}}

event: content_block_start
data: {"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}

event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"Plan"}}

event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"sig"}}

event: content_block_start
data: {"type":"content_block_start","index":1,"content_block":{"type":"text","text":""}}

event: content_block_delta
data: {"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"Hello "}}

event: content_block_delta
data: {"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"there"}}

event: content_block_start
data: {"type":"content_block_start","index":2,"content_block":{"type":"tool_use","id":"toolu_1","name":"read_file","input":{}}}

event: content_block_delta
data: {"type":"content_block_delta","index":2,"delta":{"type":"input_json_delta","partial_json":"{\"path\": \"app/"}}

event: content_block_delta
data: {"type":"content_block_delta","index":2,"delta":{"type":"input_json_delta","partial_json":"User.php\"}"}}

event: content_block_stop
data: {"type":"content_block_stop","index":2}

event: message_delta
data: {"type":"message_delta","delta":{"stop_reason":"tool_use"}}
"#;
        let (tx, rx) = mpsc::channel();
        let message = read_message(stream.as_bytes(), &tx, &|| false).unwrap();
        assert_eq!(message.stop_reason.as_deref(), Some("tool_use"));
        assert_eq!(message.content[0]["thinking"], "Plan");
        assert_eq!(message.content[0]["signature"], "sig");
        assert_eq!(message.content[1]["text"], "Hello there");
        assert_eq!(message.content[2]["input"]["path"], "app/User.php");
        let texts: Vec<String> = rx.try_iter().filter_map(|e| if let AgentEvent::Text(t) = e { Some(t) } else { None }).collect();
        assert_eq!(texts, ["Hello ", "there"]);
    }

    #[test]
    fn truncated_tool_input_is_kept_as_string() {
        let stream = "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"tool_use\",\"id\":\"t\",\"name\":\"write_file\",\"input\":{}}}\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"path\\\": \\\"a\"}}\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n";
        let (tx, _rx) = mpsc::channel();
        let message = read_message(stream.as_bytes(), &tx, &|| false).unwrap();
        assert!(message.content[0]["input"].is_string());
    }
}
