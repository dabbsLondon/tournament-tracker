//! Article discovery from Goonhammer.
//!
//! Uses the WordPress RSS feed for discovery and WP REST API for content.

use chrono::NaiveDate;
use scraper::{Html, Selector};
use url::Url;

/// A discovered article from a category page or RSS feed.
#[derive(Debug, Clone)]
pub struct DiscoveredArticle {
    /// Full URL to the article
    pub url: Url,

    /// Article title
    pub title: String,

    /// Publication date if available
    pub date: Option<NaiveDate>,

    /// WordPress post ID (if discovered via RSS)
    pub wp_post_id: Option<u64>,
}

/// Parse a Goonhammer category page to find article links.
///
/// Uses WordPress-style selectors: `article h2 a` for links,
/// `time[datetime]` for dates.
pub fn discover_goonhammer_articles(html: &str, base_url: &Url) -> Vec<DiscoveredArticle> {
    let document = Html::parse_document(html);

    let article_sel = Selector::parse("article").unwrap();
    let link_sel = Selector::parse("h2 a").unwrap();
    let time_sel = Selector::parse("time[datetime]").unwrap();

    let mut articles = Vec::new();

    for article_el in document.select(&article_sel) {
        let Some(link_el) = article_el.select(&link_sel).next() else {
            continue;
        };

        let Some(href) = link_el.value().attr("href") else {
            continue;
        };

        let title = link_el.text().collect::<String>().trim().to_string();
        if title.is_empty() {
            continue;
        }

        let url = match base_url.join(href) {
            Ok(u) => u,
            Err(_) => continue,
        };

        let date = article_el.select(&time_sel).next().and_then(|time_el| {
            time_el
                .value()
                .attr("datetime")
                .and_then(parse_date)
        });

        articles.push(DiscoveredArticle {
            url,
            title,
            date,
            wp_post_id: None,
        });
    }

    articles
}

/// Discover articles from a WordPress RSS feed XML.
///
/// Extracts title, link, pubDate, and post-id from each `<item>`.
/// Uses simple string extraction since `scraper` is an HTML parser and
/// doesn't handle XML namespaces/case-sensitivity correctly.
pub fn discover_from_rss(xml: &str) -> Vec<DiscoveredArticle> {
    let mut articles = Vec::new();

    // Split on <item> blocks
    for item_chunk in xml.split("<item>").skip(1) {
        let item_end = item_chunk.find("</item>").unwrap_or(item_chunk.len());
        let item = &item_chunk[..item_end];

        let title = match extract_xml_tag(item, "title") {
            Some(t) => decode_xml_entities(t),
            None => continue,
        };

        let url_str = match extract_xml_tag(item, "link") {
            Some(u) => u,
            None => continue,
        };

        let url = match Url::parse(url_str.trim()) {
            Ok(u) => u,
            Err(_) => continue,
        };

        let date = extract_xml_tag(item, "pubDate")
            .and_then(|d| parse_rss_date(d.trim()));

        let wp_post_id = extract_xml_tag(item, "post-id")
            .and_then(|id| id.trim().parse::<u64>().ok());

        articles.push(DiscoveredArticle {
            url,
            title,
            date,
            wp_post_id,
        });
    }

    articles
}

/// Extract text content between XML open/close tags.
fn extract_xml_tag<'a>(xml: &'a str, tag: &str) -> Option<&'a str> {
    // Handle both <tag>content</tag> and <tag ...>content</tag>
    let open_start = xml.find(&format!("<{}", tag))?;
    let after_open = &xml[open_start..];
    let content_start = after_open.find('>')? + 1;
    let content = &after_open[content_start..];

    let close_tag = format!("</{}>", tag);
    let content_end = content.find(&close_tag)?;

    let text = &content[..content_end];

    // Strip CDATA wrapper if present
    let text = text.trim();
    if text.starts_with("<![CDATA[") && text.ends_with("]]>") {
        Some(&text[9..text.len() - 3])
    } else {
        Some(text)
    }
}

/// Decode common XML entities.
fn decode_xml_entities(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#8217;", "\u{2019}")
        .replace("&#8211;", "\u{2013}")
        .replace("&#8230;", "\u{2026}")
}

/// Filter articles by date range (inclusive bounds).
pub fn filter_by_date_range(
    articles: Vec<DiscoveredArticle>,
    from: Option<NaiveDate>,
    to: Option<NaiveDate>,
) -> Vec<DiscoveredArticle> {
    articles
        .into_iter()
        .filter(|a| {
            let Some(date) = a.date else {
                return true;
            };
            if let Some(from) = from {
                if date < from {
                    return false;
                }
            }
            if let Some(to) = to {
                if date > to {
                    return false;
                }
            }
            true
        })
        .collect()
}

/// Strip HTML tags and extract clean text content.
///
/// Preserves basic structure by converting block elements to newlines.
pub fn extract_text_from_html(html: &str) -> String {
    let document = Html::parse_fragment(html);

    let mut text = String::new();
    extract_text_recursive(&document.root_element(), &mut text);

    // Clean up excessive whitespace while preserving paragraph breaks
    let mut result = String::new();
    let mut prev_blank = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            if !prev_blank && !result.is_empty() {
                result.push('\n');
                prev_blank = true;
            }
        } else {
            result.push_str(trimmed);
            result.push('\n');
            prev_blank = false;
        }
    }

    result.trim().to_string()
}

fn extract_text_recursive(element: &scraper::ElementRef, text: &mut String) {
    // Block-level tags that should produce line breaks
    const BLOCK_TAGS: &[&str] = &[
        "p", "div", "h1", "h2", "h3", "h4", "h5", "h6",
        "li", "tr", "br", "hr", "blockquote", "pre",
    ];
    // Tags to skip entirely
    const SKIP_TAGS: &[&str] = &["script", "style", "noscript", "svg", "iframe"];

    for child in element.children() {
        match child.value() {
            scraper::node::Node::Text(t) => {
                let s = t.text.trim();
                if !s.is_empty() {
                    text.push_str(s);
                    text.push(' ');
                }
            }
            scraper::node::Node::Element(el) => {
                let tag = el.name();

                if SKIP_TAGS.contains(&tag) {
                    continue;
                }

                if let Some(child_ref) = scraper::ElementRef::wrap(child) {
                    let is_block = BLOCK_TAGS.contains(&tag);
                    if is_block {
                        text.push('\n');
                    }

                    // Add heading markers for structure
                    if tag.starts_with('h') && tag.len() == 2 {
                        text.push_str("## ");
                    }

                    extract_text_recursive(&child_ref, text);

                    if is_block {
                        text.push('\n');
                    }
                }
            }
            _ => {}
        }
    }
}

/// Parse various date formats commonly found in WordPress.
fn parse_date(s: &str) -> Option<NaiveDate> {
    if let Ok(d) = NaiveDate::parse_from_str(s, "%Y-%m-%d") {
        return Some(d);
    }
    if s.len() >= 10 {
        if let Ok(d) = NaiveDate::parse_from_str(&s[..10], "%Y-%m-%d") {
            return Some(d);
        }
    }
    None
}

/// Parse RSS pubDate format: "Fri, 06 Feb 2026 13:00:12 +0000"
fn parse_rss_date(s: &str) -> Option<NaiveDate> {
    // Try RFC 2822 style
    chrono::DateTime::parse_from_rfc2822(s)
        .ok()
        .map(|dt| dt.date_naive())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_html() -> &'static str {
        r#"
        <html>
        <body>
        <div class="posts">
            <article class="post-1">
                <h2 class="entry-title">
                    <a href="https://www.goonhammer.com/competitive-innovations-june-2025/">
                        Competitive Innovations in 10th: June 2025
                    </a>
                </h2>
                <time datetime="2025-06-20">June 20, 2025</time>
            </article>
            <article class="post-2">
                <h2 class="entry-title">
                    <a href="/competitive-innovations-may-2025/">
                        Competitive Innovations in 10th: May 2025
                    </a>
                </h2>
                <time datetime="2025-05-15T12:00:00+00:00">May 15, 2025</time>
            </article>
            <article class="post-3">
                <h2 class="entry-title">
                    <a href="https://www.goonhammer.com/competitive-innovations-april-2025/">
                        Competitive Innovations in 10th: April 2025
                    </a>
                </h2>
            </article>
        </div>
        </body>
        </html>
        "#
    }

    #[test]
    fn test_discover_goonhammer_articles() {
        let base = Url::parse("https://www.goonhammer.com/category/competitive-innovations/")
            .unwrap();
        let articles = discover_goonhammer_articles(sample_html(), &base);

        assert_eq!(articles.len(), 3);

        assert_eq!(
            articles[0].title,
            "Competitive Innovations in 10th: June 2025"
        );
        assert_eq!(
            articles[0].url.as_str(),
            "https://www.goonhammer.com/competitive-innovations-june-2025/"
        );
        assert_eq!(
            articles[0].date,
            Some(NaiveDate::from_ymd_opt(2025, 6, 20).unwrap())
        );

        assert_eq!(
            articles[1].url.as_str(),
            "https://www.goonhammer.com/competitive-innovations-may-2025/"
        );
        assert_eq!(
            articles[1].date,
            Some(NaiveDate::from_ymd_opt(2025, 5, 15).unwrap())
        );

        assert!(articles[2].date.is_none());
    }

    #[test]
    fn test_filter_by_date_range_from() {
        let base = Url::parse("https://www.goonhammer.com/").unwrap();
        let articles = discover_goonhammer_articles(sample_html(), &base);

        let filtered = filter_by_date_range(
            articles,
            Some(NaiveDate::from_ymd_opt(2025, 6, 1).unwrap()),
            None,
        );

        assert_eq!(filtered.len(), 2);
        assert!(filtered[0].title.contains("June"));
        assert!(filtered[1].title.contains("April"));
    }

    #[test]
    fn test_filter_by_date_range_to() {
        let base = Url::parse("https://www.goonhammer.com/").unwrap();
        let articles = discover_goonhammer_articles(sample_html(), &base);

        let filtered = filter_by_date_range(
            articles,
            None,
            Some(NaiveDate::from_ymd_opt(2025, 5, 31).unwrap()),
        );

        assert_eq!(filtered.len(), 2);
        assert!(filtered[0].title.contains("May"));
        assert!(filtered[1].title.contains("April"));
    }

    #[test]
    fn test_filter_by_date_range_both() {
        let base = Url::parse("https://www.goonhammer.com/").unwrap();
        let articles = discover_goonhammer_articles(sample_html(), &base);

        let filtered = filter_by_date_range(
            articles,
            Some(NaiveDate::from_ymd_opt(2025, 5, 1).unwrap()),
            Some(NaiveDate::from_ymd_opt(2025, 5, 31).unwrap()),
        );

        assert_eq!(filtered.len(), 2);
    }

    #[test]
    fn test_discover_empty_html() {
        let base = Url::parse("https://www.goonhammer.com/").unwrap();
        let articles = discover_goonhammer_articles("<html><body></body></html>", &base);
        assert!(articles.is_empty());
    }

    #[test]
    fn test_parse_date_formats() {
        assert_eq!(
            parse_date("2025-06-15"),
            Some(NaiveDate::from_ymd_opt(2025, 6, 15).unwrap())
        );
        assert_eq!(
            parse_date("2025-06-15T10:00:00+00:00"),
            Some(NaiveDate::from_ymd_opt(2025, 6, 15).unwrap())
        );
        assert_eq!(parse_date("invalid"), None);
    }

    #[test]
    fn test_discover_from_rss() {
        let rss = r#"<?xml version="1.0"?>
        <rss version="2.0">
        <channel>
            <item>
                <title>Star God Mode pt.1</title>
                <link>https://www.goonhammer.com/star-god-mode-pt-1/</link>
                <pubDate>Wed, 04 Feb 2026 13:00:57 +0000</pubDate>
                <post-id xmlns="com-wordpress:feed-additions:1">231664</post-id>
            </item>
            <item>
                <title>Unsettling C'tan pt.3</title>
                <link>https://www.goonhammer.com/unsettling-ctan-pt-3/</link>
                <pubDate>Sat, 31 Jan 2026 13:00:04 +0000</pubDate>
                <post-id xmlns="com-wordpress:feed-additions:1">231109</post-id>
            </item>
        </channel>
        </rss>"#;

        let articles = discover_from_rss(rss);
        assert_eq!(articles.len(), 2);

        assert_eq!(articles[0].title, "Star God Mode pt.1");
        assert_eq!(
            articles[0].url.as_str(),
            "https://www.goonhammer.com/star-god-mode-pt-1/"
        );
        assert_eq!(
            articles[0].date,
            Some(NaiveDate::from_ymd_opt(2026, 2, 4).unwrap())
        );
        assert_eq!(articles[0].wp_post_id, Some(231664));

        assert_eq!(articles[1].title, "Unsettling C'tan pt.3");
        assert_eq!(articles[1].wp_post_id, Some(231109));
    }

    #[test]
    fn test_extract_text_from_html() {
        let html = r#"
        <div>
            <script>var x = 1;</script>
            <style>.foo { color: red; }</style>
            <h2>Tournament Results</h2>
            <p>The <strong>London GT</strong> had 96 players.</p>
            <ul>
                <li>1st - John Smith (Aeldari) - 5-0</li>
                <li>2nd - Jane Doe (Space Marines) - 4-1</li>
            </ul>
        </div>
        "#;

        let text = extract_text_from_html(html);
        assert!(text.contains("Tournament Results"));
        assert!(text.contains("London GT"));
        assert!(text.contains("96 players"));
        assert!(text.contains("John Smith"));
        assert!(text.contains("Jane Doe"));
        // Scripts and styles should be stripped
        assert!(!text.contains("var x"));
        assert!(!text.contains("color: red"));
    }

    #[test]
    fn test_extract_text_preserves_structure() {
        let html = "<h2>Event</h2><p>Details here</p><p>More info</p>";
        let text = extract_text_from_html(html);

        // Headings and paragraphs should be on separate lines
        let lines: Vec<&str> = text.lines().filter(|l| !l.is_empty()).collect();
        assert!(lines.len() >= 3);
    }

    #[test]
    fn test_parse_rss_date() {
        let date = parse_rss_date("Wed, 04 Feb 2026 13:00:57 +0000");
        assert_eq!(
            date,
            Some(NaiveDate::from_ymd_opt(2026, 2, 4).unwrap())
        );
    }
}
