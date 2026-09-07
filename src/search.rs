use crate::config::SearchConfig;
use reqwest::blocking::Client;
use serde::Deserialize;
use std::sync::Mutex;
use std::time::{Duration, Instant};

const DDG_LITE: &str = "https://lite.duckduckgo.com/lite/";
const UA: &str =
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) yappr";
const AVAILABILITY_TTL: Duration = Duration::from_secs(60);

/// How many top results to fetch full text for, and how much text to keep from
/// each. Snippets alone are often just a site description ("real-time price,
/// chart, key statistics"), which names the page without containing the answer,
/// so the model had nothing to work from and told the user to go look. Fetching a
/// couple of pages gives it the actual figures.
///
/// Two pages keeps the added latency to roughly a second on a spoken turn, and
/// 1200 characters is enough for the lead paragraph or quote block where a
/// current value normally sits.
const FETCH_TOP_N: usize = 2;
const FETCH_CHARS: usize = 1200;
const FETCH_TIMEOUT: Duration = Duration::from_secs(6);

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

pub fn web_search(cfg: &SearchConfig, query: &str) -> WebSearchOutput {
    match searxng_search(cfg, query) {
        Ok(results) if !results.is_empty() => {
            return WebSearchOutput::success(results, cfg.max_results, "SearXNG")
        }
        _ => {}
    }
    match ddg_search(cfg, query) {
        Ok(results) if !results.is_empty() => {
            WebSearchOutput::success(results, cfg.max_results, "DuckDuckGo")
        }
        Ok(_) => WebSearchOutput::failure("web_search returned no results"),
        Err(err) => WebSearchOutput::failure(format!("web_search unavailable: {err}")),
    }
}

pub struct WebSearchOutput {
    pub content: String,
    pub result_count: usize,
    pub backend: &'static str,
}

impl WebSearchOutput {
    fn success(mut results: Vec<Hit>, max_results: usize, backend: &'static str) -> Self {
        rank_evidence(&mut results);
        let result_count = results.len().min(max_results);
        Self {
            content: format_results_with_pages(results, max_results),
            result_count,
            backend,
        }
    }

    fn failure(message: impl Into<String>) -> Self {
        Self {
            content: message.into(),
            result_count: 0,
            backend: "none",
        }
    }
}

/// Search engines often rank topic hubs above individual reports. Spoken
/// answers need evidence-rich snippets, so prefer results containing recency,
/// figures, and concrete events while retaining engine order for equal scores.
fn rank_evidence(results: &mut [Hit]) {
    results.sort_by_key(|hit| std::cmp::Reverse(evidence_score(hit)));
}

fn evidence_score(hit: &Hit) -> i32 {
    let title = hit.title.to_ascii_lowercase();
    let snippet = hit.snippet.to_ascii_lowercase();
    let mut score = 0;

    if hit.snippet.chars().count() >= 100 {
        score += 1;
    }
    if hit.snippet.chars().any(|ch| ch.is_ascii_digit()) {
        score += 2;
    }
    if snippet.contains(" ago") || hit.published.is_some() {
        score += 3;
    }
    if [
        " announced ",
        " said ",
        " killed ",
        " struck ",
        " strikes ",
        " launched ",
        " agreed ",
        " signed ",
        " responded ",
    ]
    .iter()
    .any(|term| snippet.contains(term))
    {
        score += 3;
    }

    if [
        "stay on top of",
        "real-time coverage",
        "read full articles",
        "premier source",
        "latest news and updates",
        "breaking news, updates & analysis",
    ]
    .iter()
    .any(|phrase| title.contains(phrase) || snippet.contains(phrase))
    {
        score -= 6;
    }
    score
}

struct Hit {
    title: String,
    snippet: String,
    url: String,
    published: Option<String>,
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
            published: r.published_date,
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
                published: None,
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

/// Format results, fetching page text for the top few.
///
/// A snippet is often the site's own description rather than its content, so on
/// its own it names the page without answering the question. Fetching the top
/// results in parallel adds their lead text as `Page text:`.
fn format_results_with_pages(results: Vec<Hit>, max_results: usize) -> String {
    let kept: Vec<Hit> = results.into_iter().take(max_results).collect();
    // Fetch concurrently so total latency is the slowest page, not their sum.
    let fetched: Vec<Option<String>> = std::thread::scope(|scope| {
        let handles: Vec<_> = kept
            .iter()
            .take(FETCH_TOP_N)
            .map(|hit| {
                let url = hit.url.clone();
                scope.spawn(move || fetch_page_text(&url))
            })
            .collect();
        handles.into_iter().map(|h| h.join().ok().flatten()).collect()
    });
    kept.into_iter()
        .enumerate()
        .map(|(index, hit)| {
            let page = fetched.get(index).cloned().flatten();
            format_hit(index, hit, page)
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// Download a page and reduce it to readable text.
///
/// Best-effort: any failure returns None and the caller falls back to the
/// snippet. Plenty of sites reject a non-browser client (MarketWatch answers 401),
/// so a miss here is normal rather than an error worth surfacing.
fn fetch_page_text(url: &str) -> Option<String> {
    let response = client(FETCH_TIMEOUT.as_secs())
        .ok()?
        .get(url)
        .send()
        .ok()?
        .error_for_status()
        .ok()?;
    // Skip PDFs and other binaries; only markup is worth stripping.
    let is_html = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.contains("html"));
    if !is_html {
        return None;
    }
    let body = response.text().ok()?;
    let text = html_to_text(&body);
    (text.len() > 80).then(|| truncate(&text, FETCH_CHARS))
}

/// Strip markup to plain text: drop non-content elements, remove tags, collapse
/// whitespace, and decode the handful of entities that survive that.
fn html_to_text(html: &str) -> String {
    let mut out = String::with_capacity(html.len() / 4);
    let mut chars = html.char_indices().peekable();
    let mut skip_until: Option<&str> = None;
    while let Some((i, ch)) = chars.next() {
        if let Some(close) = skip_until {
            // Inside script/style/etc: resume only after the matching close tag.
            if ch == '<' && html[i..].to_ascii_lowercase().starts_with(close) {
                skip_until = None;
                for _ in 0..close.len().saturating_sub(1) {
                    chars.next();
                }
            }
            continue;
        }
        if ch == '<' {
            let rest = html[i..].to_ascii_lowercase();
            for (open, close) in [
                ("<script", "</script"),
                ("<style", "</style"),
                ("<noscript", "</noscript"),
                ("<svg", "</svg"),
                ("<head", "</head"),
            ] {
                if rest.starts_with(open) {
                    skip_until = Some(close);
                    break;
                }
            }
            if skip_until.is_some() {
                continue;
            }
            // Skip the tag itself, emitting a space so words don't run together.
            for (_, c) in chars.by_ref() {
                if c == '>' {
                    break;
                }
            }
            out.push(' ');
            continue;
        }
        out.push(ch);
    }
    let decoded = out
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'");
    decoded.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn format_hit(index: usize, hit: Hit, page: Option<String>) -> String {
    let mut fields = vec![
        format!("SEARCH RESULT {}", index + 1),
        format!("Title: {}", hit.title),
        format!("Source: {}", source_domain(&hit.url)),
    ];
    if let Some(published) = hit.published.filter(|value| !value.trim().is_empty()) {
        fields.push(format!("Published: {published}"));
    }
    fields.push(format!("Summary: {}", truncate(&hit.snippet, 360)));
    if let Some(page) = page {
        fields.push(format!("Page text: {page}"));
    }
    fields.join("\n")
}

fn source_domain(url: &str) -> String {
    reqwest::Url::parse(url)
        .ok()
        .and_then(|url| url.host_str().map(str::to_string))
        .map(|host| host.strip_prefix("www.").unwrap_or(&host).to_string())
        .unwrap_or_else(|| "unknown source".to_string())
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
    #[serde(default, rename = "publishedDate", alias = "published_date")]
    published_date: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::{
        available, format_hit, html_to_text, parse_ddg_lite, rank_evidence, source_domain, strip_tags,
        truncate, Hit, SearchConfig,
    };

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

    #[test]
    fn formats_search_evidence_without_raw_links() {
        let hit = Hit {
            title: "A concrete development".to_string(),
            snippet: "Officials announced the change on Friday.".to_string(),
            url: "https://www.reuters.com/world/example".to_string(),
            published: Some("2026-07-10T10:30:00Z".to_string()),
        };
        let text = format_hit(0, hit, None);
        assert!(text.contains("SEARCH RESULT 1"));
        assert!(text.contains("Source: reuters.com"));
        assert!(text.contains("Published: 2026-07-10"));
        // The answer is read aloud, so a URL in the evidence can be spoken back.
        assert!(!text.contains("https://"));
    }

    #[test]
    fn page_text_is_included_when_a_fetch_succeeds() {
        // The fix for snippets that describe a page without answering the
        // question: the fetched body is what carries the actual figure.
        let hit = Hit {
            title: "NVIDIA (NVDA) Stock Price".to_string(),
            snippet: "real-time price, chart, key statistics, news, and more.".to_string(),
            url: "https://stockanalysis.com/stocks/nvda/".to_string(),
            published: None,
        };
        let text = format_hit(0, hit, Some("NVDA closed at $215.94, up 1.2%.".to_string()));
        assert!(text.contains("Page text: NVDA closed at $215.94"));
        assert!(!text.contains("https://"));
    }

    #[test]
    fn html_to_text_drops_scripts_and_collapses_whitespace() {
        let html = "<html><head><title>t</title></head><body>\n  <script>var x = 1 < 2;</script>\n  <p>Price is <b>$215.94</b>&nbsp;today</p>\n  <style>.a{color:red}</style>\n</body></html>";
        let text = html_to_text(html);
        // Script and style bodies must not leak in as prose.
        assert!(!text.contains("var x"), "script body leaked: {text}");
        assert!(!text.contains("color:red"), "style body leaked: {text}");
        assert!(text.contains("Price is"));
        assert!(text.contains("$215.94"));
        // Entities decoded and runs of whitespace collapsed.
        assert!(text.contains("today"));
        assert!(!text.contains("&nbsp;"));
        assert!(!text.contains("  "), "whitespace not collapsed: {text}");
    }

    #[test]
    fn html_to_text_keeps_word_boundaries_across_tags() {
        // Tags become spaces, so adjacent words must not merge.
        let text = html_to_text("<p>first</p><p>second</p>");
        assert!(text.contains("first second"), "got {text}");
    }

    #[test]
    fn extracts_source_domain_for_spoken_search_context() {
        assert_eq!(
            source_domain("https://www.example.com/news/item"),
            "example.com"
        );
    }

    #[test]
    fn ranks_concrete_reports_ahead_of_generic_news_hubs() {
        let mut hits = vec![
            Hit {
                title: "Iran: Latest news and updates".to_string(),
                snippet: "Stay on top of the latest developments and updated coverage.".to_string(),
                url: "https://example.com/iran".to_string(),
                published: None,
            },
            Hit {
                title: "Officials announce new agreement".to_string(),
                snippet:
                    "2 hours ago · Officials announced a ceasefire after 14 people were killed."
                        .to_string(),
                url: "https://example.org/article".to_string(),
                published: None,
            },
        ];
        rank_evidence(&mut hits);
        assert_eq!(hits[0].title, "Officials announce new agreement");
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
