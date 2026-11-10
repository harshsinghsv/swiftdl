//! Site grabber: reads one web page and lists the files it links to.

use serde::Serialize;
use url::Url;

#[derive(Serialize, Debug, PartialEq)]
pub struct Found {
    pub url: String,
    /// The file name at the end of the address.
    pub name: String,
}

/// Values of href, src, data-src and similar attributes, in page order.
fn attribute_values(html: &str) -> Vec<String> {
    let lower = html.to_ascii_lowercase();
    let mut out: Vec<(usize, String)> = vec![];
    for attr in ["href", "src", "data-src", "data-url", "data-href"] {
        let mut from = 0;
        while let Some(i) = lower[from..].find(attr) {
            let at = from + i;
            from = at + attr.len();
            // A whole attribute name: not the end of a longer one like "xhref".
            if at > 0 && !matches!(lower.as_bytes()[at - 1], b' ' | b'\n' | b'\t' | b'\r' | b'"' | b'\'' | b'<') {
                continue;
            }
            let rest = html[from..].trim_start();
            let Some(rest) = rest.strip_prefix('=') else { continue };
            let rest = rest.trim_start();
            let value = match rest.chars().next() {
                Some(q @ ('"' | '\'')) => rest[1..].split(q).next().unwrap_or(""),
                Some(_) => rest.split(|c: char| c.is_whitespace() || c == '>').next().unwrap_or(""),
                None => "",
            };
            if !value.is_empty() {
                out.push((at, value.trim().to_string()));
            }
        }
    }
    out.sort_by_key(|(at, _)| *at);
    out.into_iter().map(|(_, v)| v).collect()
}

/// Every http(s) address a page links to, made absolute and without repeats.
pub fn find_links(html: &str, page: &Url) -> Vec<Found> {
    let mut seen = std::collections::HashSet::new();
    let mut found = vec![];
    for raw in attribute_values(html) {
        let raw = raw.replace("&amp;", "&");
        if raw.starts_with('#') || raw.starts_with("javascript:") || raw.starts_with("mailto:") || raw.starts_with("data:") {
            continue;
        }
        let Ok(mut url) = page.join(&raw) else { continue };
        if !matches!(url.scheme(), "http" | "https") {
            continue;
        }
        url.set_fragment(None);
        if !seen.insert(url.to_string()) {
            continue;
        }
        let name = url.path_segments().and_then(|s| s.filter(|p| !p.is_empty()).last()).map(|n| percent_decode(n)).unwrap_or_default();
        found.push(Found { url: url.to_string(), name });
        if found.len() >= 2000 {
            break;
        }
    }
    found
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 3 <= bytes.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_and_resolves_links() {
        let page = Url::parse("https://example.com/docs/index.html").unwrap();
        let html = r##"<a href="guide.pdf">g</a> <A HREF='/files/My%20Report.docx#p2'>r</A> <img src=pic.png> <a href="#top">t</a>
            <a href="mailto:a@b.c">m</a> <a href="javascript:void(0)">j</a> <a href="guide.pdf">again</a> <a href="https://cdn.example.net/v.mp4?x=1&amp;y=2">v</a>
            <a data-xhref="no.zip">x</a> <script src="/app.js"></script>"##;
        let found = find_links(html, &page);
        let urls: Vec<&str> = found.iter().map(|f| f.url.as_str()).collect();
        assert_eq!(urls, [
            "https://example.com/docs/guide.pdf",
            "https://example.com/files/My%20Report.docx",
            "https://example.com/docs/pic.png",
            "https://cdn.example.net/v.mp4?x=1&y=2",
            "https://example.com/app.js",
        ]);
        assert_eq!(found[1].name, "My Report.docx");
        assert_eq!(found[3].name, "v.mp4");
    }
}
