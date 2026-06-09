use crate::config::SearchConfig;
use reqwest::blocking::Client;
use serde::Deserialize;
use std::sync::Mutex;
use std::time::{Duration, Instant};

const DDG_LITE: &str = "https://lite.duckduckgo.com/lite/";
const UA: &str =
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) yappr";
const AVAILABILITY_TTL: Duration = Duration::from_secs(60);

// Cache the reachability probe so we don't pay timeout latency on every spoken
// question. Refreshed at most once per AVAILABILITY_TTL.
static AVAILABILITY: Mutex<Option<(Instant, bool)>> = Mutex::new(None);

/// True if a search backend can actually serve a query: the local SearXNG
/// endpoint responds, or DuckDuckGo is reachable as a fallback. Used to decide
/// whether to offer the `web_search` tool to the model at all.
pub fn available(cfg: &SearchConfig) -> bool {
    if !cfg.enabled {
        return false;
    }
    if let Ok(guard) = AVAILABILITY.lock() {
        if let Some((at, value)) = *guard {
            if at.elapsed() < AVAILABILITY_TTL {
                return value;
            }
        }
    }
    let value = searxng_reachable(cfg) || ddg_reachable();
    if let Ok(mut guard) = AVAILABILITY.lock() {
        *guard = Some((Instant::now(), value));
    }
    value
}

pub fn web_search(cfg: &SearchConfig, query: &str) -> String {
    match searxng_search(cfg, query) {
        Ok(results) if !results.is_empty() => return format_results(results, cfg.max_results),
        _ => {}
    }
    match ddg_search(cfg, query) {
        Ok(results) if !results.is_empty() => format_results(results, cfg.max_results),
        Ok(_) => "web_search returned no results".to_string(),
        Err(err) => format!("web_search unavailable: {err}"),
    }
}

struct Hit {
    title: String,
    snippet: String,
    url: String,
}

fn client(timeout_secs: u64) -> Result<Client, reqwest::Error> {
    Client::builder()
        .timeout(Duration::from_secs(timeout_secs))
        .user_agent(UA)
        .build()
}

fn searxng_reachable(cfg: &SearchConfig) -> bool {
    client(2)
        .and_then(|c| c.get(&cfg.endpoint).query(&[("q", "ping")]).send())
        .map(|r| r.status().is_success())
        .unwrap_or(false)
}

fn ddg_reachable() -> bool {
    client(2)
        .and_then(|c| c.get(DDG_LITE).send())
        .map(|r| r.status().is_success())
        .unwrap_or(false)
}

fn searxng_search(cfg: &SearchConfig, query: &str) -> Result<Vec<Hit>, Box<dyn std::error::Error>> {
    let parsed: SearchResponse = client(cfg.timeout_secs)?
        .get(&cfg.endpoint)
        .query(&[("q", query), ("format", "json")])
        .send()?
        .error_for_status()?
        .json()?;
    Ok(parsed
        .results
        .into_iter()
        .map(|r| Hit {
            title: r.title.unwrap_or_default(),
            snippet: r.content.unwrap_or_default(),
            url: r.url.unwrap_or_default(),
        })
        .collect())
}

/// Scrape DuckDuckGo's lite HTML endpoint with the blocking reqwest already in
/// the tree (no extra HTML-parser dependency). Lite returns a stable table where
/// each result is an `<a ... class="result-link" href="URL">TITLE</a>` followed
/// by a `<td class="result-snippet">SNIPPET</td>`.
fn ddg_search(cfg: &SearchConfig, query: &str) -> Result<Vec<Hit>, Box<dyn std::error::Error>> {
    let html = client(cfg.timeout_secs)?
        .post(DDG_LITE)
        .form(&[("q", query)])
        .send()?
        .error_for_status()?
        .text()?;
    Ok(parse_ddg_lite(&html))
}

fn parse_ddg_lite(html: &str) -> Vec<Hit> {
    let mut hits = Vec::new();
    for (idx, _) in html.match_indices("result-link") {
        let after = &html[idx..];
        let Some(url) = attr_before(&html[..idx], "href") else {
            continue;
        };
        // Title runs to the anchor's closing tag; snippet to its cell's.
        let title = inner_text(after, "</a>").unwrap_or_default();
        let snippet = after
            .find("result-snippet")
            .and_then(|s| inner_text(&after[s..], "</td>"))
            .unwrap_or_default();
        if !url.is_empty() && !title.is_empty() {
            hits.push(Hit {
                title,
                snippet,
                url,
            });
        }
    }
    hits
}

/// Text from the first `>` up to `close_tag`, with any nested tags (e.g. the
/// `<b>` highlights DDG wraps matched terms in) stripped and entities decoded.
fn inner_text(s: &str, close_tag: &str) -> Option<String> {
    let start = s.find('>')? + 1;
    let end = s[start..].find(close_tag)? + start;
    Some(decode_entities(&strip_tags(&s[start..end])))
}

fn strip_tags(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for ch in s.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The value of `name="..."` in the last opening tag of `before` (the anchor
/// whose class we just matched). Searches backwards from the class attribute.
fn attr_before(before: &str, name: &str) -> Option<String> {
    let tag_start = before.rfind('<')?;
    let tag = &before[tag_start..];
    let key = format!("{name}=\"");
    let at = tag.find(&key)? + key.len();
    let end = tag[at..].find('"')? + at;
    Some(tag[at..end].to_string())
}

fn decode_entities(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#x27;", "'")
        .replace("&#39;", "'")
}

fn format_results(results: Vec<Hit>, max_results: usize) -> String {
    results
        .into_iter()
        .take(max_results)
        .map(|hit| {
            format!(
                "- {}: {} ({})",
                hit.title,
                truncate(&hit.snippet, 160),
                hit.url
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
    use super::{available, parse_ddg_lite, strip_tags, truncate, SearchConfig};

    #[test]
    fn unavailable_when_search_disabled() {
        // Must short-circuit without any network probe when disabled.
        let cfg = SearchConfig {
            enabled: false,
            endpoint: "http://127.0.0.1:9/unused".to_string(),
            max_results: 5,
            timeout_secs: 1,
        };
        assert!(!available(&cfg));
    }

    #[test]
    fn strip_tags_removes_highlights_and_collapses_space() {
        assert_eq!(
            strip_tags("<b>Rust</b>  is\n a   language"),
            "Rust is a language"
        );
    }

    #[test]
    fn truncates_long_search_snippets() {
        assert_eq!(truncate("abcdef", 3), "abc...");
    }

    #[test]
    fn leaves_short_search_snippets_unchanged() {
        assert_eq!(truncate("abc", 3), "abc");
    }

    // Hits the live DuckDuckGo lite endpoint. Excluded from the default run;
    // execute with: cargo test ddg_search_live -- --ignored --nocapture
    #[test]
    #[ignore]
    fn ddg_search_live() {
        use super::ddg_search;
        let cfg = SearchConfig {
            enabled: true,
            endpoint: "http://127.0.0.1:9/unused".to_string(),
            max_results: 5,
            timeout_secs: 15,
        };
        let hits = ddg_search(&cfg, "rust programming language").expect("ddg request failed");
        assert!(!hits.is_empty(), "expected at least one DDG hit");
        for hit in hits.iter().take(3) {
            assert!(hit.url.starts_with("http"), "bad url: {:?}", hit.url);
            assert!(!hit.title.is_empty(), "empty title");
            eprintln!("hit: {} | {} | {}", hit.title, hit.url, hit.snippet);
        }
    }

    #[test]
    fn parses_ddg_lite_rows() {
        // Mirrors real lite.duckduckgo.com markup: single-quoted class, href
        // before class, and <b> highlights inside the snippet.
        let html = r#"
            <table>
              <tr><td>
                <a rel="nofollow" href="https://example.com/a" class='result-link'>First &amp; Best</a>
              </td></tr>
              <tr><td class='result-snippet'><b>A</b> short snippet here.</td></tr>
              <tr><td>
                <a rel="nofollow" href="https://example.org/b" class='result-link'>Second</a>
              </td></tr>
              <tr><td class='result-snippet'>Another snippet.</td></tr>
            </table>
        "#;
        let hits = parse_ddg_lite(html);
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].title, "First & Best");
        assert_eq!(hits[0].url, "https://example.com/a");
        assert_eq!(hits[0].snippet, "A short snippet here.");
        assert_eq!(hits[1].url, "https://example.org/b");
        assert_eq!(hits[1].snippet, "Another snippet.");
    }
}
