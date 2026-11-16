//! What a search can say besides the words to look for: a size range, typed as `size:>100MB`,
//! `size:<2GB` or `size:10MB..1GB`, or set apart (the web app's size filter, the `min_size` and
//! `max_size` of the search endpoint and of the agents' search tool). Sizes count in steps of
//! 1024, as OwnNAS shows them: 1 KB is 1024 bytes.

/// Sizes in bytes a file may have, both ends included; either end may be open.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SizeRange {
    pub min: Option<u64>,
    pub max: Option<u64>,
}

impl SizeRange {
    pub fn new(min: Option<u64>, max: Option<u64>) -> Self {
        Self { min, max }
    }

    /// Whether the range limits anything (a search with one leaves folders out).
    pub fn is_set(&self) -> bool {
        self.min.is_some() || self.max.is_some()
    }

    pub fn contains(&self, size: u64) -> bool {
        self.min.is_none_or(|min| size >= min) && self.max.is_none_or(|max| size <= max)
    }

    /// The sizes in both ranges.
    pub fn and(self, other: SizeRange) -> SizeRange {
        let pick = |a: Option<u64>, b: Option<u64>, f: fn(u64, u64) -> u64| match (a, b) {
            (Some(a), Some(b)) => Some(f(a, b)),
            (a, b) => a.or(b),
        };
        SizeRange { min: pick(self.min, other.min, u64::max), max: pick(self.max, other.max, u64::min) }
    }

    /// No size fits it.
    pub fn is_empty(&self) -> bool {
        matches!((self.min, self.max), (Some(min), Some(max)) if min > max)
    }
}

/// A search taken apart: the words to look for (the rest of the search, as typed) and the
/// size range the `size:` terms in it set.
#[derive(Debug, PartialEq, Eq)]
pub struct Terms {
    pub words: String,
    pub size: SizeRange,
}

const HOW: &str = "Write sizes like size:>100MB, size:<2GB or size:10MB..1GB";

/// Takes the `size:` terms out of `raw`. A term with no size after it, or one that cannot be
/// read, is an error saying how to write one.
pub fn parse(raw: &str) -> Result<Terms, String> {
    let tokens: Vec<&str> = raw.split_whitespace().collect();
    if !tokens.iter().any(|token| is_size_term(token)) {
        return Ok(Terms { words: raw.trim().to_string(), size: SizeRange::default() });
    }
    let mut words = Vec::new();
    let mut size = SizeRange::default();
    let mut tokens = tokens.into_iter();
    while let Some(token) = tokens.next() {
        if !is_size_term(token) {
            words.push(token);
            continue;
        }
        let mut value = &token[5..];
        // "size: >100MB", with a space after the colon.
        if value.is_empty() {
            value = tokens.next().ok_or_else(|| HOW.to_string())?;
        }
        size = size.and(parse_range(value).ok_or_else(|| format!("Could not read the size “{value}”. {HOW}."))?);
    }
    if size.is_empty() {
        return Err("No file can be that size: the smallest size is above the largest.".to_string());
    }
    Ok(Terms { words: words.join(" "), size })
}

fn is_size_term(token: &str) -> bool {
    token.get(..5).is_some_and(|prefix| prefix.eq_ignore_ascii_case("size:"))
}

/// `>X`, `>=X`, `<X`, `<=X`, `X..Y`, `X..` or `..Y`.
fn parse_range(value: &str) -> Option<SizeRange> {
    if let Some((low, high)) = value.split_once("..") {
        let low = if low.is_empty() { None } else { Some(parse_size(low)?) };
        let high = if high.is_empty() { None } else { Some(parse_size(high)?) };
        return (low.is_some() || high.is_some()).then_some(SizeRange::new(low, high));
    }
    if let Some(rest) = value.strip_prefix(">=") {
        return Some(SizeRange::new(Some(parse_size(rest)?), None));
    }
    if let Some(rest) = value.strip_prefix("<=") {
        return Some(SizeRange::new(None, Some(parse_size(rest)?)));
    }
    if let Some(rest) = value.strip_prefix('>') {
        return Some(SizeRange::new(Some(parse_size(rest)?.saturating_add(1)), None));
    }
    if let Some(rest) = value.strip_prefix('<') {
        // Below nothing: an empty range, which `parse` reports.
        return Some(match parse_size(rest)?.checked_sub(1) {
            Some(max) => SizeRange::new(None, Some(max)),
            None => SizeRange::new(Some(1), Some(0)),
        });
    }
    None
}

/// A size such as `500`, `1.5GB`, `100 mb` or `2g`, in bytes (steps of 1024).
pub fn parse_size(text: &str) -> Option<u64> {
    let text = text.trim();
    let split = text.find(|c: char| !(c.is_ascii_digit() || c == '.' || c == ',')).unwrap_or(text.len());
    let (number, unit) = text.split_at(split);
    let number: f64 = number.replace(',', ".").parse().ok()?;
    if !number.is_finite() || number < 0.0 {
        return None;
    }
    let power = match unit.trim().to_ascii_lowercase().as_str() {
        "" | "b" | "byte" | "bytes" => 0,
        "k" | "kb" | "kib" => 1,
        "m" | "mb" | "mib" => 2,
        "g" | "gb" | "gib" => 3,
        "t" | "tb" | "tib" => 4,
        _ => return None,
    };
    let bytes = (number * 1024f64.powi(power)).round();
    (bytes < u64::MAX as f64).then_some(bytes as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MB: u64 = 1024 * 1024;
    const GB: u64 = 1024 * MB;

    fn range(raw: &str) -> SizeRange {
        parse(raw).unwrap().size
    }

    #[test]
    fn reads_sizes() {
        assert_eq!(parse_size("500"), Some(500));
        assert_eq!(parse_size("100MB"), Some(100 * MB));
        assert_eq!(parse_size("100 mb"), Some(100 * MB));
        assert_eq!(parse_size("1.5GB"), Some(3 * GB / 2));
        assert_eq!(parse_size("1,5g"), Some(3 * GB / 2));
        assert_eq!(parse_size("2KiB"), Some(2048));
        assert_eq!(parse_size("1TB"), Some(1024 * GB));
        assert_eq!(parse_size("12B"), Some(12));
        assert_eq!(parse_size(""), None);
        assert_eq!(parse_size("MB"), None);
        assert_eq!(parse_size("10XB"), None);
        assert_eq!(parse_size("-5MB"), None);
    }

    #[test]
    fn reads_ranges() {
        assert_eq!(range("size:>100MB"), SizeRange::new(Some(100 * MB + 1), None));
        assert_eq!(range("size:>=100MB"), SizeRange::new(Some(100 * MB), None));
        assert_eq!(range("size:<2GB"), SizeRange::new(None, Some(2 * GB - 1)));
        assert_eq!(range("size:<=2GB"), SizeRange::new(None, Some(2 * GB)));
        assert_eq!(range("size:10MB..1GB"), SizeRange::new(Some(10 * MB), Some(GB)));
        assert_eq!(range("size:10MB.."), SizeRange::new(Some(10 * MB), None));
        assert_eq!(range("size:..1GB"), SizeRange::new(None, Some(GB)));
        assert_eq!(range("SIZE:>1k"), SizeRange::new(Some(1025), None));
        assert_eq!(range("size: >1k"), SizeRange::new(Some(1025), None), "a space after the colon");
    }

    #[test]
    fn keeps_the_words_and_combines_terms() {
        let terms = parse("holiday  size:>500MB video size:<2GB").unwrap();
        assert_eq!(terms.words, "holiday video");
        assert_eq!(terms.size, SizeRange::new(Some(500 * MB + 1), Some(2 * GB - 1)));
        // Without a size term the search is left as typed.
        assert_eq!(parse("  tag:family  trip ").unwrap(), Terms { words: "tag:family  trip".into(), size: SizeRange::default() });
        assert_eq!(parse("size:>1GB").unwrap().words, "");
        assert_eq!(parse("tag:big size:>1GB").unwrap().words, "tag:big");
        // "sizes:" and "filesize:" are words, not terms.
        assert_eq!(parse("filesize:big").unwrap().size, SizeRange::default());
    }

    #[test]
    fn says_what_is_wrong() {
        assert!(parse("size:100MB").is_err(), "a size alone is neither a lower nor an upper end");
        assert!(parse("size:").is_err());
        assert!(parse("size:>lots").is_err());
        assert!(parse("size:..").is_err());
        assert!(parse("size:<0").is_err());
        assert!(parse("size:>2GB size:<1GB").is_err());
        assert!(parse("size:2GB..1GB").is_err());
    }

    #[test]
    fn ranges_hold_and_meet() {
        let range = SizeRange::new(Some(10), Some(20));
        assert!(range.contains(10) && range.contains(20) && !range.contains(9) && !range.contains(21));
        assert!(SizeRange::default().contains(0) && !SizeRange::default().is_set());
        assert_eq!(range.and(SizeRange::new(Some(15), None)), SizeRange::new(Some(15), Some(20)));
        assert_eq!(SizeRange::new(None, Some(5)).and(SizeRange::default()), SizeRange::new(None, Some(5)));
    }
}
