//! Lossless iCalendar content model (RFC 5545 §3.1).
//!
//! Every property, parameter and component is kept in its original order
//! with its raw value, so objects can be edited and written back without
//! dropping anything this application does not understand.

use std::fmt::Write as _;

use crate::IcalError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Param {
    /// Upper-cased parameter name.
    pub name: String,
    /// Unquoted, RFC 6868-decoded values.
    pub values: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Property {
    /// Upper-cased property name.
    pub name: String,
    pub params: Vec<Param>,
    /// Raw value (still text-escaped where the value type is TEXT).
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Component {
    /// Upper-cased component name, e.g. `VEVENT`.
    pub name: String,
    pub properties: Vec<Property>,
    pub components: Vec<Component>,
}

impl Property {
    pub fn new(name: &str, value: impl Into<String>) -> Self {
        Self {
            name: name.to_ascii_uppercase(),
            params: Vec::new(),
            value: value.into(),
        }
    }

    pub fn with_param(mut self, name: &str, value: impl Into<String>) -> Self {
        self.set_param(name, value);
        self
    }

    pub fn param(&self, name: &str) -> Option<&str> {
        self.params
            .iter()
            .find(|p| p.name.eq_ignore_ascii_case(name))
            .and_then(|p| p.values.first())
            .map(String::as_str)
    }

    pub fn set_param(&mut self, name: &str, value: impl Into<String>) {
        let value = value.into();
        match self
            .params
            .iter_mut()
            .find(|p| p.name.eq_ignore_ascii_case(name))
        {
            Some(p) => p.values = vec![value],
            None => self.params.push(Param {
                name: name.to_ascii_uppercase(),
                values: vec![value],
            }),
        }
    }

    pub fn remove_param(&mut self, name: &str) {
        self.params.retain(|p| !p.name.eq_ignore_ascii_case(name));
    }
}

impl Component {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_ascii_uppercase(),
            ..Self::default()
        }
    }

    pub fn property(&self, name: &str) -> Option<&Property> {
        self.properties
            .iter()
            .find(|p| p.name.eq_ignore_ascii_case(name))
    }

    pub fn properties_named<'a>(
        &'a self,
        name: &'a str,
    ) -> impl Iterator<Item = &'a Property> + 'a {
        self.properties
            .iter()
            .filter(move |p| p.name.eq_ignore_ascii_case(name))
    }

    /// Replaces all properties named like `property` with it, keeping the
    /// position of the first one (or appending).
    pub fn set(&mut self, property: Property) {
        match self
            .properties
            .iter()
            .position(|p| p.name.eq_ignore_ascii_case(&property.name))
        {
            Some(i) => {
                let name = property.name.clone();
                self.properties[i] = property;
                let mut seen = false;
                self.properties.retain(|p| {
                    if !p.name.eq_ignore_ascii_case(&name) {
                        return true;
                    }
                    let keep = !seen;
                    seen = true;
                    keep
                });
            }
            None => self.properties.push(property),
        }
    }

    /// Replaces all properties of this name with `properties` (in place of the first).
    pub fn set_all(&mut self, name: &str, properties: Vec<Property>) {
        let at = self
            .properties
            .iter()
            .position(|p| p.name.eq_ignore_ascii_case(name))
            .unwrap_or(self.properties.len());
        self.remove(name);
        let at = at.min(self.properties.len());
        for (i, p) in properties.into_iter().enumerate() {
            self.properties.insert(at + i, p);
        }
    }

    pub fn remove(&mut self, name: &str) {
        self.properties
            .retain(|p| !p.name.eq_ignore_ascii_case(name));
    }

    pub fn children<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Component> + 'a {
        self.components
            .iter()
            .filter(move |c| c.name.eq_ignore_ascii_case(name))
    }

    /// Serializes with CRLF line endings, folding lines at 75 octets.
    pub fn to_ics(&self) -> String {
        let mut out = String::new();
        self.write(&mut out);
        out
    }

    fn write(&self, out: &mut String) {
        write_line(out, &format!("BEGIN:{}", self.name));
        for p in &self.properties {
            let mut line = p.name.clone();
            for param in &p.params {
                line.push(';');
                line.push_str(&param.name);
                line.push('=');
                for (i, v) in param.values.iter().enumerate() {
                    if i > 0 {
                        line.push(',');
                    }
                    line.push_str(&quote_param(v));
                }
            }
            line.push(':');
            line.push_str(&p.value);
            write_line(out, &line);
        }
        for c in &self.components {
            c.write(out);
        }
        write_line(out, &format!("END:{}", self.name));
    }
}

fn quote_param(value: &str) -> String {
    // RFC 6868 caret encoding for characters that cannot appear otherwise.
    let encoded = value
        .replace('^', "^^")
        .replace('\n', "^n")
        .replace('"', "^'");
    if encoded.contains([':', ';', ',']) {
        format!("\"{encoded}\"")
    } else {
        encoded
    }
}

fn write_line(out: &mut String, line: &str) {
    // Fold at 75 octets without splitting UTF-8 characters.
    let mut width = 0;
    for ch in line.chars() {
        let len = ch.len_utf8();
        if width + len > 75 {
            out.push_str("\r\n ");
            width = 1;
        }
        out.push(ch);
        width += len;
    }
    let _ = out.write_str("\r\n");
}

/// Joins folded lines (RFC 5545 §3.1). Tolerates LF-only line endings.
fn unfold(text: &str) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for raw in text.split('\n') {
        let raw = raw.strip_suffix('\r').unwrap_or(raw);
        if let Some(rest) = raw.strip_prefix([' ', '\t'])
            && let Some(last) = lines.last_mut()
        {
            last.push_str(rest);
            continue;
        }
        if !raw.is_empty() {
            lines.push(raw.to_owned());
        }
    }
    lines
}

fn decode_param(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '^' {
            match chars.peek() {
                Some('n') => {
                    out.push('\n');
                    chars.next();
                }
                Some('^') => {
                    out.push('^');
                    chars.next();
                }
                Some('\'') => {
                    out.push('"');
                    chars.next();
                }
                _ => out.push('^'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn parse_line(line: &str, number: usize) -> Result<Property, IcalError> {
    let err = |what: &str| IcalError::Syntax(format!("line {number}: {what}"));
    // The name ends at the first ';' or ':'.
    let name_end = line.find([';', ':']).ok_or_else(|| err("missing ':'"))?;
    let name = &line[..name_end];
    if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return Err(err("invalid property name"));
    }
    let mut params = Vec::new();
    let mut rest = &line[name_end..];
    while let Some(after) = rest.strip_prefix(';') {
        let eq = after
            .find('=')
            .ok_or_else(|| err("parameter without '='"))?;
        let pname = after[..eq].to_ascii_uppercase();
        let mut values = Vec::new();
        let mut s = &after[eq + 1..];
        loop {
            if let Some(q) = s.strip_prefix('"') {
                let close = q
                    .find('"')
                    .ok_or_else(|| err("unterminated quoted parameter"))?;
                values.push(decode_param(&q[..close]));
                s = &q[close + 1..];
            } else {
                let end = s.find([',', ';', ':']).ok_or_else(|| err("missing ':'"))?;
                values.push(decode_param(&s[..end]));
                s = &s[end..];
            }
            match s.strip_prefix(',') {
                Some(more) => s = more,
                None => break,
            }
        }
        params.push(Param {
            name: pname,
            values,
        });
        rest = s;
    }
    let value = rest.strip_prefix(':').ok_or_else(|| err("missing ':'"))?;
    Ok(Property {
        name: name.to_ascii_uppercase(),
        params,
        value: value.to_owned(),
    })
}

/// Parses one iCalendar stream whose outermost component is `VCALENDAR`.
pub fn parse(text: &str) -> Result<Component, IcalError> {
    let mut stack: Vec<Component> = Vec::new();
    let mut root: Option<Component> = None;
    for (i, line) in unfold(text).iter().enumerate() {
        let prop = parse_line(line, i + 1)?;
        match prop.name.as_str() {
            "BEGIN" => stack.push(Component::new(&prop.value)),
            "END" => {
                let done = stack
                    .pop()
                    .ok_or_else(|| IcalError::Syntax(format!("unexpected END:{}", prop.value)))?;
                if !done.name.eq_ignore_ascii_case(&prop.value) {
                    return Err(IcalError::Syntax(format!(
                        "END:{} does not close BEGIN:{}",
                        prop.value, done.name
                    )));
                }
                match stack.last_mut() {
                    Some(parent) => parent.components.push(done),
                    None if root.is_none() => root = Some(done),
                    None => {
                        return Err(IcalError::Syntax(
                            "more than one top-level component".into(),
                        ));
                    }
                }
            }
            _ => match stack.last_mut() {
                Some(c) => c.properties.push(prop),
                None => return Err(IcalError::Syntax("property outside a component".into())),
            },
        }
    }
    if let Some(open) = stack.last() {
        return Err(IcalError::Syntax(format!(
            "BEGIN:{} is never closed",
            open.name
        )));
    }
    let root = root.ok_or_else(|| IcalError::Syntax("empty calendar".into()))?;
    if root.name != "VCALENDAR" {
        return Err(IcalError::Syntax(format!(
            "expected VCALENDAR, found {}",
            root.name
        )));
    }
    Ok(root)
}

/// Unescapes a TEXT value (RFC 5545 §3.3.11).
pub fn unescape_text(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n' | 'N') => out.push('\n'),
                Some(other) => out.push(other),
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

pub fn escape_text(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            ';' => out.push_str("\\;"),
            ',' => out.push_str("\\,"),
            '\n' => out.push_str("\\n"),
            '\r' => {}
            _ => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unfolds_parses_and_round_trips() {
        let text = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:abc\r\nDESCRIPTION:A long line that is\r\n  folded\r\nATTENDEE;CN=\"Doe, Jane\";ROLE=REQ-PARTICIPANT:mailto:jane@example.com\r\nX-VENDOR;X-P=1:keep me\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
        let cal = parse(text).unwrap();
        let ev = &cal.components[0];
        assert_eq!(
            ev.property("DESCRIPTION").unwrap().value,
            "A long line that is folded"
        );
        let att = ev.property("ATTENDEE").unwrap();
        assert_eq!(att.param("CN"), Some("Doe, Jane"));
        assert_eq!(att.value, "mailto:jane@example.com");
        // Writing and re-parsing yields the same model.
        assert_eq!(parse(&cal.to_ics()).unwrap(), cal);
        assert!(cal.to_ics().contains("X-VENDOR;X-P=1:keep me\r\n"));
    }

    #[test]
    fn folds_long_lines_on_char_boundaries() {
        let mut cal = Component::new("VCALENDAR");
        cal.properties
            .push(Property::new("SUMMARY", "ü".repeat(100)));
        let ics = cal.to_ics();
        assert!(ics.split("\r\n").all(|l| l.len() <= 75));
        assert_eq!(parse(&ics).unwrap(), cal);
    }

    #[test]
    fn text_escaping_round_trips() {
        let s = "a;b,c\\d\nnew line";
        assert_eq!(unescape_text(&escape_text(s)), s);
        assert_eq!(escape_text("x,y"), "x\\,y");
    }

    #[test]
    fn caret_encoding_in_parameters() {
        let p = parse_line("X-A;LABEL=\"say ^'hi^' ^^ ^n\":v", 1).unwrap();
        assert_eq!(p.param("LABEL"), Some("say \"hi\" ^ \n"));
    }

    #[test]
    fn rejects_malformed_input() {
        for bad in [
            "",
            "BEGIN:VCALENDAR\r\n",
            "BEGIN:VCALENDAR\r\nEND:VEVENT\r\n",
            "BEGIN:VEVENT\r\nEND:VEVENT\r\n",
            "BEGIN:VCALENDAR\r\nNOCOLON\r\nEND:VCALENDAR\r\n",
        ] {
            assert!(parse(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn set_and_set_all_keep_position() {
        let mut c = Component::new("VEVENT");
        c.properties.push(Property::new("UID", "1"));
        c.properties.push(Property::new("EXDATE", "a"));
        c.properties.push(Property::new("EXDATE", "b"));
        c.properties.push(Property::new("SUMMARY", "s"));
        c.set_all("EXDATE", vec![Property::new("EXDATE", "c")]);
        let names: Vec<_> = c.properties.iter().map(|p| p.value.as_str()).collect();
        assert_eq!(names, vec!["1", "c", "s"]);
        c.set(Property::new("SUMMARY", "t"));
        assert_eq!(c.property("SUMMARY").unwrap().value, "t");
    }
}
