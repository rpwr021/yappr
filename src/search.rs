use crate::config::SearchConfig;
use reqwest::blocking::Client;
use serde::Deserialize;
use std::time::Duration;

pub fn web_search(cfg: &SearchConfig, query: &str) -> String {
    let client = match Client::builder()
        .timeout(Duration::from_secs(cfg.timeout_secs))
        .build()
    {
        Ok(client) => client,
        Err(err) => return format!("web_search unavailable: {err}"),
    };
    let response = client
        .get(&cfg.endpoint)
        .query(&[("q", query), ("format", "json")])
        .send();
    let Ok(response) = response else {
        return "web_search unavailable: local SearXNG did not respond".to_string();
    };
    let Ok(parsed) = response.json::<SearchResponse>() else {
        return "web_search returned an unreadable response".to_string();
    };
    if parsed.results.is_empty() {
        return "web_search returned no results".to_string();
    }
    parsed
        .results
        .into_iter()
        .take(cfg.max_results)
        .map(|result| {
            let snippet = truncate(&result.content.unwrap_or_default(), 160);
            format!(
                "- {}: {} ({})",
                result.title.unwrap_or_default(),
                snippet,
                result.url.unwrap_or_default()
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn truncate(value: &str, max_chars: usize) -> String {
    let mut out = value.chars().take(max_chars).collect::<String>();
    if value.chars().count() > max_chars {
        out.push_str("...");
    }
    out
}

#[derive(Debug, Deserialize)]
struct SearchResponse {
    results: Vec<SearchResult>,
}

#[derive(Debug, Deserialize)]
struct SearchResult {
    title: Option<String>,
    content: Option<String>,
    url: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::truncate;

    #[test]
    fn truncates_long_search_snippets() {
        assert_eq!(truncate("abcdef", 3), "abc...");
    }

    #[test]
    fn leaves_short_search_snippets_unchanged() {
        assert_eq!(truncate("abc", 3), "abc");
    }
}
