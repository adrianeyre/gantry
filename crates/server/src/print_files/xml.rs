//! A small XML tag reader, enough for the parts of a `.3mf` Gantry reads:
//! element names and their attributes, in order. Text content, namespaces
//! and DTDs are not needed and are skipped. It never allocates more than one
//! tag at a time, so a mesh of millions of vertices streams through it.

/// One start, empty or end tag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tag<'a> {
    /// The name with any namespace prefix removed, such as `vertex`.
    pub name: &'a str,
    /// `</name>`.
    pub end: bool,
    /// `<name/>`: it has no children.
    pub empty: bool,
    attributes: &'a str,
}

impl<'a> Tag<'a> {
    /// An attribute's value, unescaped, matched on the name without its
    /// namespace prefix (`p:path` matches `path`).
    pub fn attr(&self, wanted: &str) -> Option<String> {
        self.attrs().find(|(k, _)| *k == wanted).map(|(_, v)| v)
    }

    /// Every attribute, in order, unescaped.
    pub fn attrs(&self) -> impl Iterator<Item = (&'a str, String)> + 'a {
        let mut rest = self.attributes;
        std::iter::from_fn(move || {
            let trimmed = rest.trim_start();
            let eq = trimmed.find('=')?;
            let key = trimmed[..eq].trim();
            let after = trimmed[eq + 1..].trim_start();
            let quote = after.chars().next().filter(|c| *c == '"' || *c == '\'')?;
            let close = after[1..].find(quote)?;
            let value = &after[1..=close];
            rest = &after[close + 2..];
            let key = key.rsplit(':').next().unwrap_or(key);
            Some((key, unescape(value)))
        })
    }

    /// A number-valued attribute.
    pub fn number<T: std::str::FromStr>(&self, name: &str) -> Option<T> {
        self.attr(name).and_then(|v| v.trim().parse().ok())
    }
}

fn unescape(value: &str) -> String {
    if !value.contains('&') {
        return value.to_owned();
    }
    let mut out = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        rest = &rest[at..];
        let Some(semi) = rest.find(';') else { break };
        let entity = &rest[1..semi];
        let decoded = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ => entity
                .strip_prefix("#x")
                .and_then(|h| u32::from_str_radix(h, 16).ok())
                .or_else(|| entity.strip_prefix('#').and_then(|d| d.parse().ok()))
                .and_then(char::from_u32),
        };
        match decoded {
            Some(c) => {
                out.push(c);
                rest = &rest[semi + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// Every tag in `xml`, in document order.
pub fn tags(xml: &str) -> impl Iterator<Item = Tag<'_>> {
    let mut rest = xml;
    std::iter::from_fn(move || {
        loop {
            let open = rest.find('<')?;
            rest = &rest[open + 1..];
            if let Some(after) = rest.strip_prefix("!--") {
                let close = after.find("-->")?;
                rest = &after[close + 3..];
                continue;
            }
            if rest.starts_with('?') || rest.starts_with('!') {
                let close = rest.find('>')?;
                rest = &rest[close + 1..];
                continue;
            }
            let close = find_tag_end(rest)?;
            let inner = &rest[..close];
            rest = &rest[close + 1..];
            let (end, inner) = match inner.strip_prefix('/') {
                Some(inner) => (true, inner),
                None => (false, inner),
            };
            let (empty, inner) = match inner.strip_suffix('/') {
                Some(inner) => (true, inner),
                None => (false, inner),
            };
            let name_end = inner
                .find(|c: char| c.is_whitespace())
                .unwrap_or(inner.len());
            let full = &inner[..name_end];
            let name = full.rsplit(':').next().unwrap_or(full);
            return Some(Tag {
                name,
                end,
                empty,
                attributes: &inner[name_end..],
            });
        }
    })
}

/// The `>` that closes a tag, skipping any inside quoted attribute values.
fn find_tag_end(s: &str) -> Option<usize> {
    let mut quote = None;
    for (i, c) in s.char_indices() {
        match (quote, c) {
            (None, '"' | '\'') => quote = Some(c),
            (Some(q), c) if c == q => quote = None,
            (None, '>') => return Some(i),
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_tags_and_attributes() {
        let xml = r#"<?xml version="1.0"?>
            <!-- a comment with <tags> -->
            <model unit="millimeter" xmlns:p="http://x">
              <object id="1" name="A &amp; B" p:path='/3D/x.model'>
                <vertex x="1.5" y="-2" z="3e1"/>
              </object>
            </model>"#;
        let tags: Vec<Tag> = tags(xml).collect();
        let names: Vec<(&str, bool, bool)> =
            tags.iter().map(|t| (t.name, t.end, t.empty)).collect();
        assert_eq!(
            names,
            [
                ("model", false, false),
                ("object", false, false),
                ("vertex", false, true),
                ("object", true, false),
                ("model", true, false),
            ]
        );
        assert_eq!(tags[1].attr("name").as_deref(), Some("A & B"));
        assert_eq!(tags[1].attr("path").as_deref(), Some("/3D/x.model"));
        assert_eq!(tags[2].number::<f32>("z"), Some(30.0));
        assert_eq!(tags[2].number::<f32>("w"), None);
    }

    #[test]
    fn a_greater_than_inside_a_value_does_not_end_the_tag() {
        let tags: Vec<Tag> = tags(r#"<a b="x>y" c="&#65;&#x42;"/>"#).collect();
        assert_eq!(tags.len(), 1);
        assert_eq!(tags[0].attr("b").as_deref(), Some("x>y"));
        assert_eq!(tags[0].attr("c").as_deref(), Some("AB"));
    }
}
