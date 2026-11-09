//! Debian control-file syntax (RFC 822-ish paragraphs) and relationship fields.

use std::collections::BTreeMap;

/// One control paragraph with lower-cased field names.
#[derive(Debug, Clone, Default)]
pub struct Control {
    fields: BTreeMap<String, String>,
}

impl Control {
    pub fn parse(text: &str) -> Self {
        let mut fields = BTreeMap::new();
        for_each_field(text, |k, v| {
            fields.insert(k.to_ascii_lowercase(), v);
        });
        Self { fields }
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.fields.get(&key.to_ascii_lowercase()).map(String::as_str)
    }

    pub fn package(&self) -> Option<&str> {
        self.get("package")
    }
    pub fn version(&self) -> Option<&str> {
        self.get("version")
    }
    pub fn architecture(&self) -> Option<&str> {
        self.get("architecture")
    }

    /// `Pre-Depends` followed by `Depends`, as groups of alternatives.
    pub fn depends(&self) -> Vec<Vec<String>> {
        let mut out = parse_relations(self.get("pre-depends").unwrap_or(""));
        out.extend(parse_relations(self.get("depends").unwrap_or("")));
        out
    }
}

/// Call `f(field, value)` for each field of a single paragraph. Continuation
/// lines (starting with whitespace) are folded into the previous value.
pub fn for_each_field(paragraph: &str, mut f: impl FnMut(&str, String)) {
    let mut current: Option<(String, String)> = None;
    for line in paragraph.lines() {
        if line.starts_with(' ') || line.starts_with('\t') {
            if let Some((_, v)) = current.as_mut() {
                v.push('\n');
                v.push_str(line.trim());
            }
            continue;
        }
        if let Some((k, v)) = current.take() {
            f(&k, v);
        }
        if let Some((k, v)) = line.split_once(':') {
            current = Some((k.trim().to_owned(), v.trim().to_owned()));
        }
    }
    if let Some((k, v)) = current {
        f(&k, v);
    }
}

/// Split a Packages file into paragraphs.
pub fn paragraphs(text: &str) -> impl Iterator<Item = &str> {
    text.split("\n\n").map(str::trim).filter(|p| !p.is_empty())
}

/// Parse `a (>= 1), b | c:any [amd64] <!nocheck>` into `[[a], [b, c]]`.
pub fn parse_relations(s: &str) -> Vec<Vec<String>> {
    s.split(',')
        .filter_map(|group| {
            let alts: Vec<String> = group
                .split('|')
                .filter_map(|alt| {
                    let name =
                        alt.trim().split(|c: char| c.is_whitespace() || c == '(' || c == '[' || c == '<').next()?;
                    let name = name.split(':').next()?.trim();
                    (!name.is_empty()).then(|| name.to_owned())
                })
                .collect();
            (!alts.is_empty()).then_some(alts)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_relations() {
        let r =
            parse_relations("libjq1 (= 1.7.1-6), libc6 (>= 2.34), python3:any | python3-minimal [amd64] <!nocheck>");
        assert_eq!(r, vec![vec!["libjq1"], vec!["libc6"], vec!["python3", "python3-minimal"]]);
        assert!(parse_relations("").is_empty());
    }

    #[test]
    fn parses_paragraph_with_continuations() {
        let c =
            Control::parse("Package: jq\nVersion: 1.7.1-6\nDescription: lightweight\n and flexible\nDepends: libjq1\n");
        assert_eq!(c.package(), Some("jq"));
        assert_eq!(c.get("description"), Some("lightweight\nand flexible"));
        assert_eq!(c.depends(), vec![vec!["libjq1".to_string()]]);
    }
}
