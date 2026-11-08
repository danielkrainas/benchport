//! Optional RSS/Atom news feed.

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewsItem {
    pub title: String,
    pub link: Option<String>,
    /// RFC 3339 timestamp.
    pub date: Option<String>,
    pub summary: String,
    pub image: Option<String>,
}

pub fn parse(bytes: &[u8]) -> Result<Vec<NewsItem>> {
    let feed = feed_rs::parser::parse(bytes).map_err(|e| Error::msg(format!("Could not read news feed: {e}")))?;
    let mut items: Vec<NewsItem> = feed
        .entries
        .into_iter()
        .map(|e| {
            let html = e
                .summary
                .as_ref()
                .map(|t| t.content.clone())
                .or_else(|| e.content.as_ref().and_then(|c| c.body.clone()))
                .unwrap_or_default();
            let content_html = e.content.as_ref().and_then(|c| c.body.clone()).unwrap_or_default();
            let image = e
                .media
                .iter()
                .flat_map(|m| {
                    m.thumbnails
                        .iter()
                        .map(|t| t.image.uri.clone())
                        .chain(m.content.iter().filter(|c| is_image(c)).filter_map(|c| c.url.as_ref().map(|u| u.to_string())))
                })
                .next()
                .or_else(|| first_img(&html))
                .or_else(|| first_img(&content_html));
            NewsItem {
                title: e.title.map(|t| strip_html(&t.content)).unwrap_or_else(|| "Untitled".into()),
                link: e.links.first().map(|l| l.href.clone()),
                date: e.published.or(e.updated).map(|d| d.to_rfc3339()),
                summary: truncate(&strip_html(&html), 320),
                image,
            }
        })
        .collect();
    items.sort_by(|a, b| b.date.cmp(&a.date));
    Ok(items)
}

fn is_image(c: &feed_rs::model::MediaContent) -> bool {
    match &c.content_type {
        Some(m) => m.to_string().starts_with("image/"),
        None => c.url.as_ref().is_some_and(|u| {
            let p = u.path().to_ascii_lowercase();
            [".png", ".jpg", ".jpeg", ".gif", ".webp"].iter().any(|ext| p.ends_with(ext))
        }),
    }
}

fn first_img(html: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    let start = lower.find("<img")?;
    let rest = &html[start..];
    let src = rest.to_ascii_lowercase().find("src=")?;
    let after = &rest[src + 4..];
    let quote = after.chars().next()?;
    if quote != '"' && quote != '\'' {
        return None;
    }
    let end = after[1..].find(quote)?;
    let url = &after[1..1 + end];
    (url.starts_with("http://") || url.starts_with("https://")).then(|| url.to_string())
}

fn strip_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' if in_tag => {
                in_tag = false;
                out.push(' ');
            }
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    let decoded = out
        .replace("&nbsp;", " ")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&amp;", "&");
    decoded.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let cut: String = s.chars().take(max).collect();
    let cut = cut.rsplit_once(' ').map(|(a, _)| a).unwrap_or(&cut);
    format!("{}…", cut.trim_end_matches(|c: char| !c.is_alphanumeric()))
}

#[cfg(test)]
mod tests {
    #[test]
    fn parses_rss_and_strips_html() {
        let xml = br#"<?xml version="1.0"?><rss version="2.0"><channel><title>t</title>
          <item><title>Old</title><link>https://e.com/1</link><pubDate>Mon, 02 Nov 2026 10:00:00 +0000</pubDate><description>a</description></item>
          <item><title>New &amp; shiny</title><link>https://e.com/2</link><pubDate>Sat, 07 Nov 2026 10:00:00 +0000</pubDate>
            <description>&lt;p&gt;Hello &lt;b&gt;world&lt;/b&gt;&lt;img src="https://e.com/i.png"&gt;&lt;/p&gt;</description></item>
        </channel></rss>"#;
        let items = super::parse(xml).unwrap();
        assert_eq!(items[0].title, "New & shiny");
        assert_eq!(items[0].summary, "Hello world");
        assert_eq!(items[0].image.as_deref(), Some("https://e.com/i.png"));
        assert_eq!(items[1].title, "Old");
    }
}
