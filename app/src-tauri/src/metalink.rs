//! Metalink files (.meta4 and .metalink): one file, several mirrors, and the
//! file's checksum. Linux distributions and many projects publish them.

use anyhow::{Context, Result, bail};

#[derive(Debug, PartialEq)]
pub struct Metalink {
    pub name: String,
    pub size: Option<u64>,
    /// SHA-256 as hex, when the file lists one.
    pub sha256: Option<String>,
    /// Best first.
    pub urls: Vec<String>,
}

pub fn is_metalink(url: &str) -> bool {
    let path = url.split(['?', '#']).next().unwrap_or("").to_ascii_lowercase();
    path.ends_with(".meta4") || path.ends_with(".metalink")
}

pub fn parse(xml: &str) -> Result<Metalink> {
    let doc = roxmltree::Document::parse(xml).context("not a valid Metalink file")?;
    let file = doc.descendants().find(|n| n.has_tag_name("file")).context("the Metalink lists no file")?;
    let name = file.attribute("name").context("the Metalink file has no name")?.to_string();
    let text = |tag: &str| file.descendants().find(|n| n.has_tag_name(tag)).and_then(|n| n.text()).map(|t| t.trim().to_string());
    let size = text("size").and_then(|s| s.parse().ok());
    let sha256 = file
        .descendants()
        .filter(|n| n.has_tag_name("hash"))
        .find(|n| n.attribute("type").is_some_and(|t| t.eq_ignore_ascii_case("sha-256") || t.eq_ignore_ascii_case("sha256")))
        .and_then(|n| n.text())
        .map(|h| h.trim().to_lowercase());
    // v4: `priority` (1 is best). v3: `preference` (100 is best).
    let mut urls: Vec<(i64, String)> = file
        .descendants()
        .filter(|n| n.has_tag_name("url"))
        .filter_map(|n| {
            let u = n.text()?.trim().to_string();
            if !(u.starts_with("http://") || u.starts_with("https://")) {
                return None;
            }
            let rank = match (n.attribute("priority"), n.attribute("preference")) {
                (Some(p), _) => p.parse::<i64>().unwrap_or(999_999),
                (None, Some(p)) => 1000 - p.parse::<i64>().unwrap_or(0),
                _ => 999_999,
            };
            Some((rank, u))
        })
        .collect();
    if urls.is_empty() {
        bail!("the Metalink has no web addresses SwiftDL can use");
    }
    urls.sort_by_key(|(rank, _)| *rank); // stable: equal ranks keep their order
    Ok(Metalink { name, size, sha256, urls: urls.into_iter().map(|(_, u)| u).collect() })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_version_4() {
        let m = parse(r#"<metalink xmlns="urn:ietf:params:xml:ns:metalink"><file name="x.iso"><size>123</size>
            <hash type="sha-1">aa</hash><hash type="sha-256">ABCD</hash>
            <url priority="2">https://b.example/x.iso</url><url priority="1">https://a.example/x.iso</url><url>ftp://c.example/x.iso</url></file></metalink>"#).unwrap();
        assert_eq!(m, Metalink { name: "x.iso".into(), size: Some(123), sha256: Some("abcd".into()), urls: vec!["https://a.example/x.iso".into(), "https://b.example/x.iso".into()] });
    }

    #[test]
    fn reads_version_3() {
        let m = parse(r#"<metalink version="3.0" xmlns="http://www.metalinker.org/"><files><file name="y.zip"><size>9</size>
            <verification><hash type="sha256">ff00</hash></verification>
            <resources><url type="http" preference="50">http://slow.example/y.zip</url><url type="https" preference="100">https://fast.example/y.zip</url></resources></file></files></metalink>"#).unwrap();
        assert_eq!(m.urls[0], "https://fast.example/y.zip");
        assert_eq!(m.sha256.as_deref(), Some("ff00"));
        assert_eq!(m.size, Some(9));
    }

    #[test]
    fn spots_metalink_links() {
        assert!(is_metalink("https://x/a.iso.meta4?mirror=1") && is_metalink("https://x/a.METALINK") && !is_metalink("https://x/a.iso"));
        assert!(parse("<metalink/>").is_err());
    }
}
