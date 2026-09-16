use anyhow::{Context, Result};
use async_trait::async_trait;
use reqwest::Client;
use serde::Deserialize;
use serde_json::{json, Value};
use std::time::Duration;

use super::{Tool, ToolResult};

const BRAVE_ENDPOINT: &str = "https://api.search.brave.com/res/v1/web/search";
const DUCKDUCKGO_ENDPOINT: &str = "https://html.duckduckgo.com/html/";

pub struct WebSearchTool {
    client: Client,
    brave_key: Option<String>,
}

#[derive(Deserialize)]
struct SearchInput {
    query: String,
    #[serde(default = "default_count")]
    count: usize,
    #[serde(default)]
    freshness: Option<String>,
}

#[derive(Debug, Deserialize)]
struct BraveResponse {
    web: Option<BraveWeb>,
}

#[derive(Debug, Deserialize)]
struct BraveWeb {
    results: Vec<BraveResult>,
}

#[derive(Debug, Deserialize)]
struct BraveResult {
    title: String,
    url: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    page_age: Option<String>,
}

#[derive(Debug, PartialEq)]
struct SearchResult {
    title: String,
    url: String,
    snippet: String,
    age: Option<String>,
}

fn default_count() -> usize {
    5
}

impl WebSearchTool {
    pub fn new(brave_key: Option<String>) -> Result<Self> {
        let client = Client::builder()
            .timeout(Duration::from_secs(15))
            .user_agent("Hyusk/0.1 native-web-search")
            .build()
            .context("Failed to initialize native web search")?;

        Ok(Self {
            client,
            brave_key: brave_key.filter(|key| !key.trim().is_empty()),
        })
    }

    async fn search_brave(
        &self,
        key: &str,
        query: &str,
        count: usize,
        freshness: Option<&str>,
    ) -> Result<Vec<SearchResult>> {
        let count_text = count.to_string();
        let mut request = self
            .client
            .get(BRAVE_ENDPOINT)
            .header("Accept", "application/json")
            .header("X-Subscription-Token", key)
            .query(&[
                ("q", query),
                ("count", count_text.as_str()),
                ("country", "IN"),
                ("search_lang", "en"),
                ("safesearch", "moderate"),
                ("text_decorations", "false"),
            ]);

        if let Some(freshness) = brave_freshness(freshness) {
            request = request.query(&[("freshness", freshness)]);
        }

        let response = request
            .send()
            .await
            .context("Brave Search request failed")?
            .error_for_status()
            .context("Brave Search returned an error")?
            .json::<BraveResponse>()
            .await
            .context("Brave Search returned invalid JSON")?;

        Ok(response
            .web
            .map(|web| {
                web.results
                    .into_iter()
                    .take(count)
                    .map(|result| SearchResult {
                        title: result.title,
                        url: result.url,
                        snippet: result.description,
                        age: result.page_age,
                    })
                    .collect()
            })
            .unwrap_or_default())
    }

    async fn search_duckduckgo(&self, query: &str, count: usize) -> Result<Vec<SearchResult>> {
        let response = self
            .client
            .get(DUCKDUCKGO_ENDPOINT)
            .query(&[("q", query), ("kl", "in-en"), ("kp", "1")])
            .send()
            .await
            .context("DuckDuckGo search request failed")?
            .error_for_status()
            .context("DuckDuckGo search returned an error")?
            .text()
            .await
            .context("Could not read DuckDuckGo search results")?;

        Ok(parse_duckduckgo(&response, count))
    }
}

#[async_trait]
impl Tool for WebSearchTool {
    fn name(&self) -> &str {
        "web_search"
    }

    fn description(&self) -> &str {
        "Search the live web directly without opening or controlling a browser. Use this for current events, changing facts, online documentation, recommendations, and source-backed answers. Results contain titles, URLs, and snippets; mention source names in the answer."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Focused web search query, at most 600 characters."
                },
                "count": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 10,
                    "default": 5
                },
                "freshness": {
                    "type": "string",
                    "enum": ["day", "week", "month", "year"],
                    "description": "Optional recency filter. Supported when Brave Search is configured."
                }
            },
            "required": ["query"]
        })
    }

    async fn execute(&self, input: &str) -> Result<ToolResult> {
        let input: SearchInput =
            serde_json::from_str(input).context("Web search input must contain a query")?;
        let query = input.query.trim();

        if query.is_empty() {
            return Ok(ToolResult::failure("Search query must not be empty."));
        }
        if query.chars().count() > 600 || query.split_whitespace().count() > 75 {
            return Ok(ToolResult::failure(
                "Search query must be at most 600 characters and 75 words.",
            ));
        }

        let count = input.count.clamp(1, 10);
        let brave_results = if let Some(key) = self.brave_key.as_deref() {
            match self
                .search_brave(key, query, count, input.freshness.as_deref())
                .await
            {
                Ok(results) if !results.is_empty() => Some(results),
                Ok(_) => None,
                Err(error) => {
                    eprintln!("[Web] Brave Search failed; using DuckDuckGo: {error}");
                    None
                }
            }
        } else {
            None
        };

        let (provider, results) = match brave_results {
            Some(results) => ("Brave Search", results),
            None => (
                "DuckDuckGo HTML",
                self.search_duckduckgo(query, count).await?,
            ),
        };

        if results.is_empty() {
            return Ok(ToolResult::failure(format!(
                "{provider} returned no results for '{query}'."
            )));
        }

        let mut output = format!("Live web results from {provider} for: {query}\n");
        for (index, result) in results.iter().enumerate() {
            output.push_str(&format!(
                "\n{}. {}\nURL: {}\nSnippet: {}",
                index + 1,
                truncate(&result.title, 300),
                truncate(&result.url, 2_048),
                truncate(&result.snippet, 1_000)
            ));
            if let Some(age) = result.age.as_deref() {
                output.push_str(&format!("\nAge: {age}"));
            }
            output.push('\n');
        }

        Ok(ToolResult::success(output))
    }
}

fn brave_freshness(value: Option<&str>) -> Option<&'static str> {
    match value {
        Some("day") => Some("pd"),
        Some("week") => Some("pw"),
        Some("month") => Some("pm"),
        Some("year") => Some("py"),
        _ => None,
    }
}

fn parse_duckduckgo(html: &str, limit: usize) -> Vec<SearchResult> {
    html.split("class=\"result results_links")
        .skip(1)
        .filter_map(parse_duckduckgo_result)
        .take(limit)
        .collect()
}

fn parse_duckduckgo_result(block: &str) -> Option<SearchResult> {
    let anchor = block.find("class=\"result__a\"")?;
    let href_start = block[anchor..].find("href=\"")? + anchor + 6;
    let href_end = block[href_start..].find('"')? + href_start;
    let title_start = block[href_end..].find('>')? + href_end + 1;
    let title_end = block[title_start..].find("</a>")? + title_start;

    let snippet_marker = block.find("class=\"result__snippet\"")?;
    let snippet_start = block[snippet_marker..].find('>')? + snippet_marker + 1;
    let snippet_end = block[snippet_start..].find("</a>")? + snippet_start;

    let href = decode_html_entities(&block[href_start..href_end]);
    let url = duckduckgo_destination(&href)?;

    Some(SearchResult {
        title: clean_html(&block[title_start..title_end]),
        url,
        snippet: clean_html(&block[snippet_start..snippet_end]),
        age: None,
    })
}

fn duckduckgo_destination(href: &str) -> Option<String> {
    let query = href.split_once('?')?.1;
    let encoded = query
        .split('&')
        .find_map(|part| part.strip_prefix("uddg="))?;
    let decoded = percent_decode(encoded)?;

    (decoded.starts_with("https://") || decoded.starts_with("http://")).then_some(decoded)
}

fn percent_decode(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;

    while index < bytes.len() {
        match bytes[index] {
            b'%' if index + 2 < bytes.len() => {
                let high = hex_value(bytes[index + 1])?;
                let low = hex_value(bytes[index + 2])?;
                decoded.push(high * 16 + low);
                index += 3;
            }
            b'+' => {
                decoded.push(b' ');
                index += 1;
            }
            byte => {
                decoded.push(byte);
                index += 1;
            }
        }
    }

    String::from_utf8(decoded).ok()
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn clean_html(value: &str) -> String {
    let mut plain = String::with_capacity(value.len());
    let mut inside_tag = false;

    for character in value.chars() {
        match character {
            '<' => inside_tag = true,
            '>' => inside_tag = false,
            _ if !inside_tag => plain.push(character),
            _ => {}
        }
    }

    decode_html_entities(&plain)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn truncate(value: &str, maximum: usize) -> String {
    if value.chars().count() <= maximum {
        return value.to_string();
    }

    value.chars().take(maximum).collect::<String>() + "…"
}

fn decode_html_entities(value: &str) -> String {
    value
        .replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&#x27;", "'")
        .replace("&#39;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&nbsp;", " ")
}

#[cfg(test)]
mod tests {
    use super::{brave_freshness, parse_duckduckgo};

    #[test]
    fn parses_duckduckgo_title_destination_and_snippet() {
        let html = r#"
          <div class="result results_links results_links_deep web-result ">
            <a class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com%2Fnews&amp;rut=x">Example &amp; News</a>
            <a class="result__snippet" href="ignored">A <b>fresh</b> result.</a>
          </div>
        "#;
        let results = parse_duckduckgo(html, 5);

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Example & News");
        assert_eq!(results[0].url, "https://example.com/news");
        assert_eq!(results[0].snippet, "A fresh result.");
    }

    #[test]
    fn maps_supported_brave_freshness_values() {
        assert_eq!(brave_freshness(Some("day")), Some("pd"));
        assert_eq!(brave_freshness(Some("week")), Some("pw"));
        assert_eq!(brave_freshness(Some("other")), None);
    }
}
