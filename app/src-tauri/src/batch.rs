//! Turns pasted text into download links: one per line or space, and number
//! patterns like `https://site/file[01-50].zip`.

const MAX_LINKS: usize = 500;

/// `https://x/a[01-03].zip` -> a01, a02, a03. Zero padding follows the first number.
fn pattern(token: &str) -> Option<Vec<String>> {
    let open = token.find('[')?;
    let close = open + token[open..].find(']')?;
    let (a, b) = token[open + 1..close].split_once('-')?;
    let (lo, hi): (u64, u64) = (a.parse().ok()?, b.parse().ok()?);
    if hi < lo || hi - lo >= MAX_LINKS as u64 {
        return None;
    }
    let width = if a.len() > 1 && a.starts_with('0') { a.len() } else { 0 };
    Some((lo..=hi).map(|n| format!("{}{n:0width$}{}", &token[..open], &token[close + 1..])).collect())
}

/// Every http(s) link in `text`, patterns expanded, at most 500.
pub fn expand_links(text: &str) -> Vec<String> {
    let mut out = vec![];
    for token in text.split_whitespace() {
        if !(token.starts_with("http://") || token.starts_with("https://")) {
            continue;
        }
        match pattern(token) {
            Some(list) => out.extend(list),
            None => out.push(token.to_string()),
        }
        if out.len() >= MAX_LINKS {
            out.truncate(MAX_LINKS);
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::expand_links;

    #[test]
    fn expands_patterns_and_lists() {
        assert_eq!(expand_links("https://x/a[08-10].zip"), ["https://x/a08.zip", "https://x/a09.zip", "https://x/a10.zip"]);
        assert_eq!(expand_links("https://x/p[1-3]"), ["https://x/p1", "https://x/p2", "https://x/p3"]);
        assert_eq!(expand_links("https://a/1\nhttps://b/2 junk  http://c/3"), ["https://a/1", "https://b/2", "http://c/3"]);
        assert_eq!(expand_links("http://[::1]/file").len(), 1);
        assert_eq!(expand_links("https://x/a[1-9999].zip").len(), 1); // too many: left as typed
        assert!(expand_links("no links here").is_empty());
    }
}
