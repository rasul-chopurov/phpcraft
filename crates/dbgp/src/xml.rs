//! Just enough XML for DBGp responses: elements, attributes, text and CDATA.

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Element {
    pub name: String,
    pub attrs: Vec<(String, String)>,
    pub children: Vec<Element>,
    pub text: String,
}

impl Element {
    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attrs.iter().find(|(k, _)| k == name || k.rsplit(':').next() == Some(name)).map(|(_, v)| v.as_str())
    }

    /// The first child named `name` (namespace prefixes ignored).
    pub fn child(&self, name: &str) -> Option<&Element> {
        self.children.iter().find(|c| local(&c.name) == name)
    }

    pub fn children_named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Element> + 'a {
        self.children.iter().filter(move |c| local(&c.name) == name)
    }
}

fn local(name: &str) -> &str {
    name.rsplit(':').next().unwrap_or(name)
}

fn unescape(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        let Some(end) = tail.find(';') else {
            out.push_str(tail);
            return out;
        };
        let entity = &tail[1..end];
        match entity {
            "lt" => out.push('<'),
            "gt" => out.push('>'),
            "amp" => out.push('&'),
            "quot" => out.push('"'),
            "apos" => out.push('\''),
            e if e.starts_with("#x") => out.extend(u32::from_str_radix(&e[2..], 16).ok().and_then(char::from_u32)),
            e if e.starts_with('#') => out.extend(e[1..].parse().ok().and_then(char::from_u32)),
            _ => out.push_str(&tail[..=end]),
        }
        rest = &tail[end + 1..];
    }
    out.push_str(rest);
    out
}

/// Parses a document into its root element.
pub fn parse(source: &str) -> Option<Element> {
    let mut stack: Vec<Element> = Vec::new();
    let mut root = None;
    let mut rest = source;
    while !rest.is_empty() {
        if let Some(stripped) = rest.strip_prefix("<?") {
            rest = &stripped[stripped.find("?>")? + 2..];
        } else if let Some(stripped) = rest.strip_prefix("<!--") {
            rest = &stripped[stripped.find("-->")? + 3..];
        } else if let Some(stripped) = rest.strip_prefix("<![CDATA[") {
            let end = stripped.find("]]>")?;
            if let Some(top) = stack.last_mut() {
                top.text.push_str(&stripped[..end]);
            }
            rest = &stripped[end + 3..];
        } else if let Some(stripped) = rest.strip_prefix("</") {
            let end = stripped.find('>')?;
            let done = stack.pop()?;
            match stack.last_mut() {
                Some(parent) => parent.children.push(done),
                None => root = Some(done),
            }
            rest = &stripped[end + 1..];
        } else if let Some(stripped) = rest.strip_prefix('<') {
            let end = tag_end(stripped)?;
            let tag = &stripped[..end];
            let self_closing = tag.ends_with('/');
            let tag = tag.trim_end_matches('/');
            let (name, attrs) = parse_tag(tag);
            let element = Element { name, attrs, ..Default::default() };
            if self_closing {
                match stack.last_mut() {
                    Some(parent) => parent.children.push(element),
                    None => root = Some(element),
                }
            } else {
                stack.push(element);
            }
            rest = &stripped[end + 1..];
        } else {
            let end = rest.find('<').unwrap_or(rest.len());
            if let Some(top) = stack.last_mut() {
                top.text.push_str(&unescape(&rest[..end]));
            }
            rest = &rest[end..];
        }
    }
    root
}

/// End of a tag, skipping `>` inside quoted attribute values.
fn tag_end(s: &str) -> Option<usize> {
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

fn parse_tag(tag: &str) -> (String, Vec<(String, String)>) {
    let tag = tag.trim();
    let name_end = tag.find(char::is_whitespace).unwrap_or(tag.len());
    let name = tag[..name_end].to_string();
    let mut attrs = Vec::new();
    let mut rest = tag[name_end..].trim_start();
    while let Some(eq) = rest.find('=') {
        let key = rest[..eq].trim().to_string();
        let after = rest[eq + 1..].trim_start();
        let Some(quote) = after.chars().next().filter(|c| *c == '"' || *c == '\'') else { break };
        let Some(close) = after[1..].find(quote) else { break };
        attrs.push((key, unescape(&after[1..1 + close])));
        rest = after[close + 2..].trim_start();
    }
    (name, attrs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_dbgp_responses() {
        let xml = r#"<?xml version="1.0" encoding="iso-8859-1"?>
<response xmlns="urn:debugger_protocol_v1" xmlns:xdebug="https://xdebug.org/dbgp/xdebug" command="stack_get" transaction_id="3"><stack where="{main}" level="0" type="file" filename="file:///app/a.php" lineno="4"></stack><xdebug:message filename="file:///app/a.php" lineno="4"/><property name="$s" type="string" encoding="base64"><![CDATA[aGk=]]></property><error code="5"><message>a &amp; b &lt; c</message></error></response>"#;
        let root = parse(xml).unwrap();
        assert_eq!(root.attr("command"), Some("stack_get"));
        assert_eq!(root.child("stack").unwrap().attr("lineno"), Some("4"));
        assert_eq!(root.child("message").unwrap().attr("filename"), Some("file:///app/a.php"));
        assert_eq!(root.child("property").unwrap().text, "aGk=");
        assert_eq!(root.child("error").unwrap().child("message").unwrap().text, "a & b < c");
    }
}
