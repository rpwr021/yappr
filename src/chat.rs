use crate::config::Config;
use crate::search;
use base64::Engine;
use chrono::Local;
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::time::Duration;

#[derive(Clone, Copy)]
pub enum ChatMode {
    Spoken,
}

pub struct ChatClient {
    cfg: Config,
    http: Client,
    history: std::sync::Mutex<Vec<HistoryTurn>>,
}

impl ChatClient {
    pub fn new(cfg: Config) -> Result<Self, reqwest::Error> {
        let http = Client::builder()
            .timeout(Duration::from_secs(cfg.server.timeout_secs))
            .build()?;
        Ok(Self {
            cfg,
            http,
            history: std::sync::Mutex::new(Vec::new()),
        })
    }

    pub fn transcribe_wav(&self, wav: &[u8]) -> Result<String, Box<dyn std::error::Error>> {
        let prompt = transcription_prompt(&self.cfg);
        let audio = base64::engine::general_purpose::STANDARD.encode(wav);
        let body = json!({
            "messages": [{
                "role": "user",
                "content": [
                    {"type": "text", "text": prompt},
                    {"type": "input_audio", "input_audio": {"data": audio, "format": "wav"}}
                ]
            }],
            "temperature": 0,
            "max_tokens": 512,
            "reasoning_effort": "none",
            "chat_template_kwargs": {"enable_thinking": false}
        });
        let response: ChatResponse = self
            .http
            .post(&self.cfg.server.endpoint)
            .json(&body)
            .send()?
            .error_for_status()?
            .json()?;
        let content = response
            .choices
            .first()
            .and_then(|choice| choice.message.content.as_deref())
            .unwrap_or("")
            .trim();
        Ok(parse_translation_target(content, &self.cfg.language.target))
    }

    pub fn answer(
        &self,
        question: &str,
        _mode: ChatMode,
    ) -> Result<String, Box<dyn std::error::Error>> {
        let mut messages = vec![json!({"role": "system", "content": chat_system_prompt()})];
        self.append_recent_history(&mut messages);
        messages.push(json!({"role": "user", "content": question}));
        let first = self.chat_call(&messages, search::available(&self.cfg.search))?;
        let choice = first
            .choices
            .first()
            .ok_or("chat response had no choices")?;
        // Trigger the hand-off whenever the model emitted a tool call. Local
        // llama-server templates often set finish_reason to "stop" rather than
        // "tool_calls" even when tool_calls is populated, so keying only on
        // finish_reason would silently skip the search.
        if let Some(tool_call) = choice.requested_tool_call() {
            let query = tool_call.query().unwrap_or_else(|| question.to_string());
            crate::logger::debug_line(format!("web_search: {query}"));
            let results = search::web_search(&self.cfg.search, &query);
            messages.push(json!({
                "role": "assistant",
                "content": Value::Null,
                "tool_calls": [tool_call.assistant_json()]
            }));
            messages.push(json!({
                "role": "tool",
                "tool_call_id": tool_call.id,
                "name": "web_search",
                "content": results
            }));
            let second = self.chat_call(&messages, false)?;
            let answer = clean_spoken_text(
                second
                    .choices
                    .first()
                    .and_then(|c| c.message.content.as_deref())
                    .unwrap_or(""),
            );
            self.remember(question, &answer);
            return Ok(answer);
        }
        let answer = clean_spoken_text(choice.message.content.as_deref().unwrap_or(""));
        self.remember(question, &answer);
        Ok(answer)
    }

    fn chat_call(
        &self,
        messages: &[Value],
        with_tools: bool,
    ) -> Result<ChatResponse, Box<dyn std::error::Error>> {
        let mut body = json!({
            "messages": messages,
            "temperature": 0,
            "max_tokens": 512,
            "reasoning_effort": "none",
            "chat_template_kwargs": {"enable_thinking": false}
        });
        if with_tools {
            body["tools"] = json!([{
                "type": "function",
                "function": {
                    "name": "web_search",
                    "description": "Search the web for current, recent, or time-sensitive information (news, prices, events, recent releases, anything after your training cutoff). Do NOT use for general knowledge, math, or definitions.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "query": {
                                "type": "string",
                                "description": "A specific, keyword-style query. Include the explicit date for time-sensitive topics and a place name for local info."
                            }
                        },
                        "required": ["query"]
                    }
                }
            }]);
            body["tool_choice"] = json!("auto");
        }
        Ok(self
            .http
            .post(&self.cfg.server.endpoint)
            .json(&body)
            .send()?
            .error_for_status()?
            .json()?)
    }
}

#[derive(Clone)]
struct HistoryTurn {
    at: std::time::Instant,
    question: String,
    answer: String,
}

impl ChatClient {
    fn append_recent_history(&self, messages: &mut Vec<Value>) {
        let Ok(mut history) = self.history.lock() else {
            return;
        };
        let max_age = std::time::Duration::from_secs(self.cfg.chat.context_seconds.max(0) as u64);
        history.retain(|turn| turn.at.elapsed() <= max_age);
        let start = history.len().saturating_sub(4);
        for turn in &history[start..] {
            messages.push(json!({"role": "user", "content": turn.question}));
            messages.push(json!({"role": "assistant", "content": turn.answer}));
        }
    }

    fn remember(&self, question: &str, answer: &str) {
        if let Ok(mut history) = self.history.lock() {
            history.push(HistoryTurn {
                at: std::time::Instant::now(),
                question: question.to_string(),
                answer: answer.to_string(),
            });
            let keep_from = history.len().saturating_sub(4);
            if keep_from > 0 {
                history.drain(0..keep_from);
            }
        }
    }
}

fn transcription_prompt(cfg: &Config) -> String {
    let digits =
        "Write digits rather than words (e.g. write 1.7 not one point seven, and 3 not three).";
    let source = if cfg.language.source == "auto" {
        "the original language"
    } else {
        &cfg.language.source
    };
    if cfg.language.target == "auto" {
        format!("Transcribe the following speech segment in {source} into text. Output only the transcription with no extra commentary and no newlines. {digits}")
    } else {
        format!("Transcribe the following speech segment in {source}, then translate it into {}. First output the transcription, then a newline, then '{}: ' followed by the translation. {digits}", cfg.language.target, cfg.language.target)
    }
}

fn chat_system_prompt() -> String {
    format!(
        "The current date and time is {}. Answer the user's spoken question concisely in plain spoken prose. Do not use markdown, headings, bullet points, code blocks, or emoji. When time-sensitive or local, search with a specific keyword query that includes today's date and any place name. After searching, give the best direct answer from the results, including specific facts and numbers. Do not tell the user to check websites; summarize what the results say.",
        Local::now().format("%B %-d, %Y %-I:%M %p %Z")
    )
}

fn parse_translation_target(content: &str, target: &str) -> String {
    if target == "auto" {
        return content.trim().to_string();
    }
    let marker = format!("{target}:");
    if let Some((_, tail)) = content.rsplit_once(&marker) {
        return tail.trim().to_string();
    }
    content
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .unwrap_or(content)
        .trim()
        .to_string()
}

fn clean_spoken_text(text: &str) -> String {
    text.replace(['*', '`', '#'], "")
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

#[derive(Debug, Deserialize, Serialize)]
struct ChatResponse {
    choices: Vec<Choice>,
}

#[derive(Debug, Deserialize, Serialize)]
struct Choice {
    message: Message,
    finish_reason: Option<String>,
}

impl Choice {
    /// The tool call the model wants run, if any. Keys on the presence of a
    /// tool_calls entry rather than finish_reason, since local chat templates
    /// are inconsistent about finish_reason. Only honors calls for web_search
    /// (or with no name set, which some templates emit).
    fn requested_tool_call(&self) -> Option<&ToolCall> {
        let call = self.message.tool_calls.as_ref()?.first()?;
        match call.function.name.as_deref() {
            None | Some("web_search") => Some(call),
            Some(_) => None,
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
struct Message {
    content: Option<String>,
    tool_calls: Option<Vec<ToolCall>>,
}

#[derive(Debug, Deserialize, Serialize)]
struct ToolCall {
    id: String,
    #[serde(rename = "type")]
    kind: Option<String>,
    function: ToolFunction,
}

#[derive(Debug, Deserialize, Serialize)]
struct ToolFunction {
    name: Option<String>,
    arguments: String,
}

impl ToolCall {
    fn query(&self) -> Option<String> {
        serde_json::from_str::<Value>(&self.function.arguments)
            .ok()
            .and_then(|value| {
                value
                    .get("query")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
    }

    fn assistant_json(&self) -> Value {
        json!({
            "id": self.id,
            "type": self.kind.as_deref().unwrap_or("function"),
            "function": {
                "name": self.function.name.as_deref().unwrap_or("web_search"),
                "arguments": self.function.arguments
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{clean_spoken_text, parse_translation_target, Choice, ToolCall, ToolFunction};

    #[test]
    fn parses_translation_after_target_marker() {
        let text = "hello there\nSpanish: hola";
        assert_eq!(parse_translation_target(text, "Spanish"), "hola");
    }

    #[test]
    fn translation_parse_falls_back_to_last_non_empty_line() {
        let text = "hello there\n\nhola";
        assert_eq!(parse_translation_target(text, "Spanish"), "hola");
    }

    #[test]
    fn spoken_text_removes_markdown_noise() {
        assert_eq!(
            clean_spoken_text("# Title\n\n*hello* `there`"),
            "Title hello there"
        );
    }

    #[test]
    fn extracts_tool_query_json() {
        let call = ToolCall {
            id: "1".to_string(),
            kind: Some("function".to_string()),
            function: ToolFunction {
                name: Some("web_search".to_string()),
                arguments: r#"{"query":"weather June 8 2026 San Francisco"}"#.to_string(),
            },
        };
        assert_eq!(
            call.query().as_deref(),
            Some("weather June 8 2026 San Francisco")
        );
    }

    #[test]
    fn tool_query_is_none_for_malformed_arguments() {
        let call = ToolCall {
            id: "1".to_string(),
            kind: None,
            function: ToolFunction {
                name: Some("web_search".to_string()),
                arguments: "not json".to_string(),
            },
        };
        assert_eq!(call.query(), None);
    }

    fn choice_from(json: &str) -> Choice {
        serde_json::from_str(json).expect("valid choice json")
    }

    #[test]
    fn hands_off_when_finish_reason_is_tool_calls() {
        let choice = choice_from(
            r#"{"finish_reason":"tool_calls","message":{"content":null,
                "tool_calls":[{"id":"a","type":"function",
                "function":{"name":"web_search","arguments":"{\"query\":\"x\"}"}}]}}"#,
        );
        assert!(choice.requested_tool_call().is_some());
    }

    #[test]
    fn hands_off_when_tool_calls_present_but_finish_reason_is_stop() {
        // Local llama-server templates often report "stop" even with a tool call.
        let choice = choice_from(
            r#"{"finish_reason":"stop","message":{"content":null,
                "tool_calls":[{"id":"a","function":{"name":"web_search","arguments":"{}"}}]}}"#,
        );
        assert!(choice.requested_tool_call().is_some());
    }

    #[test]
    fn no_hand_off_when_no_tool_calls() {
        let choice =
            choice_from(r#"{"finish_reason":"stop","message":{"content":"just an answer"}}"#);
        assert!(choice.requested_tool_call().is_none());
    }

    #[test]
    fn ignores_tool_call_for_unknown_function() {
        let choice = choice_from(
            r#"{"finish_reason":"tool_calls","message":{"content":null,
                "tool_calls":[{"id":"a","function":{"name":"do_something_else","arguments":"{}"}}]}}"#,
        );
        assert!(choice.requested_tool_call().is_none());
    }
}
