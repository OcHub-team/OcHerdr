//! Ghostty `key = value` document that can update one assignment without
//! rewriting comments or unknown keys.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Newline {
    Lf,
    Crlf,
}

impl Newline {
    fn as_str(self) -> &'static str {
        match self {
            Self::Lf => "\n",
            Self::Crlf => "\r\n",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Line {
    Raw(String),
    Assignment(Assignment),
}

impl Line {
    fn render(&self) -> String {
        match self {
            Self::Raw(raw) => raw.clone(),
            Self::Assignment(assignment) => assignment.render(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Assignment {
    key: String,
    value: String,
    quoted: bool,
    prefix: String,
    suffix: String,
}

impl Assignment {
    fn new(key: &str, value: &str) -> Self {
        if value.is_empty() {
            return Self {
                key: key.to_owned(),
                value: String::new(),
                quoted: false,
                prefix: format!("{key} ="),
                suffix: String::new(),
            };
        }
        if needs_quotes(value) {
            Self {
                key: key.to_owned(),
                value: value.to_owned(),
                quoted: true,
                prefix: format!("{key} = \""),
                suffix: "\"".to_owned(),
            }
        } else {
            Self {
                key: key.to_owned(),
                value: value.to_owned(),
                quoted: false,
                prefix: format!("{key} = "),
                suffix: String::new(),
            }
        }
    }

    fn render(&self) -> String {
        format!("{}{}{}", self.prefix, self.value, self.suffix)
    }

    fn set_value(&mut self, value: String) {
        self.value = value;
        if self.value.is_empty() {
            if self.quoted {
                self.quoted = false;
                if self.prefix.ends_with('"') {
                    self.prefix.pop();
                }
                if self.suffix.starts_with('"') {
                    self.suffix.remove(0);
                }
            }
            return;
        }
        if !self.quoted && needs_quotes(&self.value) {
            self.quoted = true;
            self.prefix.push('"');
            self.suffix.insert(0, '"');
        }
    }
}

fn needs_quotes(value: &str) -> bool {
    !value.is_empty()
        && !value.contains('"')
        && (value.starts_with(' ') || value.ends_with(' ') || value.contains(char::is_whitespace))
}

fn parse_assignment(raw: &str) -> Option<Assignment> {
    if raw.trim_start().starts_with('#') {
        return None;
    }
    let eq = raw.find('=')?;
    let key = raw[..eq].trim();
    if key.is_empty() {
        return None;
    }
    let after_eq = &raw[eq + 1..];
    let leading_ws = after_eq.len() - after_eq.trim_start().len();
    let trimmed_val = after_eq.trim();
    let trailing_ws = after_eq.len() - leading_ws - trimmed_val.len();
    let (quoted, inner) =
        if trimmed_val.len() >= 2 && trimmed_val.starts_with('"') && trimmed_val.ends_with('"') {
            (true, &trimmed_val[1..trimmed_val.len() - 1])
        } else {
            (false, trimmed_val)
        };
    let prefix_end = eq + 1 + leading_ws + usize::from(quoted);
    let suffix_start = raw.len() - trailing_ws - usize::from(quoted);
    Some(Assignment {
        key: key.to_owned(),
        value: inner.to_owned(),
        quoted,
        prefix: raw[..prefix_end].to_owned(),
        suffix: raw[suffix_start..].to_owned(),
    })
}

/// In-memory Ghostty config file. Serialization is a join of the original
/// lines except for assignments that were explicitly updated.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfigDocument {
    lines: Vec<Line>,
    newline: Newline,
    trailing_newline: bool,
}

impl Default for ConfigDocument {
    fn default() -> Self {
        Self::new()
    }
}

impl ConfigDocument {
    pub fn new() -> Self {
        Self {
            lines: Vec::new(),
            newline: Newline::Lf,
            trailing_newline: true,
        }
    }

    pub fn parse(source: &str) -> Self {
        let newline = if source.contains("\r\n") {
            Newline::Crlf
        } else {
            Newline::Lf
        };
        let normalized = source.replace("\r\n", "\n");
        let trailing_newline = normalized.ends_with('\n');
        let body = normalized.strip_suffix('\n').unwrap_or(&normalized);
        if body.is_empty() && !trailing_newline {
            return Self {
                lines: Vec::new(),
                newline,
                trailing_newline: false,
            };
        }
        if body.is_empty() {
            return Self {
                lines: vec![Line::Raw(String::new())],
                newline,
                trailing_newline: true,
            };
        }
        let lines = body
            .split('\n')
            .map(|raw| match parse_assignment(raw) {
                Some(assignment) => Line::Assignment(assignment),
                None => Line::Raw(raw.to_owned()),
            })
            .collect();
        Self {
            lines,
            newline,
            trailing_newline,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
            || self.lines.iter().all(|line| match line {
                Line::Raw(raw) => raw.trim().is_empty(),
                Line::Assignment(_) => false,
            })
    }

    pub fn serialize(&self) -> String {
        if self.lines.is_empty() {
            return String::new();
        }
        let nl = self.newline.as_str();
        let mut out = String::new();
        for (index, line) in self.lines.iter().enumerate() {
            if index > 0 {
                out.push_str(nl);
            }
            out.push_str(&line.render());
        }
        if self.trailing_newline {
            out.push_str(nl);
        }
        out
    }

    /// Last value for `key`, if any. Ghostty last-wins for unique keys.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.assignment_values(key).next_back()
    }

    pub fn get_all(&self, key: &str) -> Vec<&str> {
        self.assignment_values(key).collect()
    }

    fn assignment_values(&self, key: &str) -> impl DoubleEndedIterator<Item = &str> {
        self.lines.iter().filter_map(move |line| match line {
            Line::Assignment(assignment) if assignment.key == key => {
                Some(assignment.value.as_str())
            }
            _ => None,
        })
    }

    /// Update a unique key in place. Repeatable keys should use
    /// [`set_repeatable`](Self::set_repeatable).
    pub fn set(&mut self, key: &str, value: &str) {
        if self.get(key) == Some(value) {
            return;
        }
        if let Some(index) = self.last_assignment_index(key) {
            let Line::Assignment(assignment) = &mut self.lines[index] else {
                return;
            };
            assignment.set_value(value.to_owned());
            return;
        }
        self.append_assignment(Assignment::new(key, value));
    }

    /// Replace every occurrence of a repeatable key (`font-family`,
    /// `font-feature`, `palette`) with `values`, keeping the original
    /// block position when the key already exists.
    pub fn set_repeatable(&mut self, key: &str, values: &[String]) {
        let indexes: Vec<usize> = self
            .lines
            .iter()
            .enumerate()
            .filter_map(|(index, line)| match line {
                Line::Assignment(assignment) if assignment.key == key => Some(index),
                _ => None,
            })
            .collect();
        if indexes.is_empty() {
            for value in values {
                self.append_assignment(Assignment::new(key, value));
            }
            return;
        }
        let shared = indexes.len().min(values.len());
        for (slot, value) in indexes.iter().copied().zip(values.iter()).take(shared) {
            let Line::Assignment(assignment) = &mut self.lines[slot] else {
                continue;
            };
            assignment.set_value(value.clone());
        }
        if values.len() > indexes.len() {
            let insert_at = indexes[indexes.len() - 1] + 1;
            let extra = values[indexes.len()..]
                .iter()
                .map(|value| Line::Assignment(Assignment::new(key, value)))
                .collect::<Vec<_>>();
            let extra_len = extra.len();
            self.lines.splice(insert_at..insert_at, extra);
            if insert_at == self.lines.len() - extra_len && extra_len > 0 {
                self.trailing_newline = true;
            }
            return;
        }
        if values.len() < indexes.len() {
            for index in indexes.iter().skip(values.len()).rev().copied() {
                self.lines.remove(index);
            }
        }
    }

    pub fn assignments(&self) -> impl DoubleEndedIterator<Item = (usize, &str, &str)> {
        self.lines.iter().enumerate().filter_map(|(index, line)| {
            let Line::Assignment(assignment) = line else {
                return None;
            };
            Some((
                index + 1,
                assignment.key.as_str(),
                assignment.value.as_str(),
            ))
        })
    }

    fn last_assignment_index(&self, key: &str) -> Option<usize> {
        self.lines
            .iter()
            .enumerate()
            .rev()
            .find_map(|(index, line)| match line {
                Line::Assignment(assignment) if assignment.key == key => Some(index),
                _ => None,
            })
    }

    fn append_assignment(&mut self, assignment: Assignment) {
        if self.lines.is_empty() {
            self.trailing_newline = true;
        }
        self.lines.push(Line::Assignment(assignment));
        self.trailing_newline = true;
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseWarning {
    pub line: usize,
    pub key: String,
    pub message: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changing_one_key_leaves_comments_and_unknown_keys_byte_identical() {
        let source = "\
# keep this comment
font-size = 13

# also keep this
mystery-option = wow
font-family = \"Maple Mono Normal NF CN\"
palette = 0=#111111
";
        assert!(
            source.contains("# keep this comment"),
            "fixture must include the comment this test claims to preserve"
        );
        assert!(
            source.contains("mystery-option = wow"),
            "fixture must include the unknown key this test claims to preserve"
        );
        let mut document = ConfigDocument::parse(source);
        document.set("font-size", "16");
        let expected = source.replacen("font-size = 13", "font-size = 16", 1);
        assert_eq!(document.serialize(), expected);
        assert!(document.serialize().contains("# keep this comment"));
        assert!(document.serialize().contains("mystery-option = wow"));
        assert!(
            document
                .serialize()
                .contains("font-family = \"Maple Mono Normal NF CN\"")
        );
    }

    #[test]
    fn updating_a_quoted_value_keeps_surrounding_quotes_and_spacing() {
        let source = "font-family = \"Old Face\"\n";
        let mut document = ConfigDocument::parse(source);
        document.set("font-family", "New Face");
        assert_eq!(document.serialize(), "font-family = \"New Face\"\n");
    }

    #[test]
    fn compact_equals_spacing_is_preserved_when_the_value_changes() {
        let source = "font-size=13\n";
        let mut document = ConfigDocument::parse(source);
        document.set("font-size", "14");
        assert_eq!(document.serialize(), "font-size=14\n");
    }

    #[test]
    fn unknown_keys_and_blank_and_comment_lines_round_trip() {
        let source = "\
# header

keybind = ctrl+c=copy
font-size = 13

# footer
";
        let document = ConfigDocument::parse(source);
        assert_eq!(document.serialize(), source);
        assert_eq!(document.get("keybind"), Some("ctrl+c=copy"));
        assert_eq!(document.get("font-size"), Some("13"));
    }

    #[test]
    fn repeatable_keys_append_and_replace_in_place() {
        let source = "\
font-family = One
font-size = 13
font-family = Two
";
        let mut document = ConfigDocument::parse(source);
        document.set_repeatable(
            "font-family",
            &["Alpha".to_owned(), "Beta".to_owned(), "Gamma".to_owned()],
        );
        assert_eq!(
            document.serialize(),
            "\
font-family = Alpha
font-size = 13
font-family = Beta
font-family = Gamma
"
        );
        document.set_repeatable("font-family", &["Only".to_owned()]);
        assert_eq!(
            document.serialize(),
            "\
font-family = Only
font-size = 13
"
        );
    }

    #[test]
    fn empty_value_resets_a_key_without_dropping_the_line() {
        let source = "font-size = 18\n";
        let mut document = ConfigDocument::parse(source);
        document.set("font-size", "");
        assert_eq!(document.serialize(), "font-size = \n");
        assert_eq!(document.get("font-size"), Some(""));
    }

    #[test]
    fn missing_key_is_appended_without_rewriting_the_rest() {
        let source = "# stay\nfont-size = 13\n";
        let mut document = ConfigDocument::parse(source);
        document.set("language", "en");
        assert_eq!(
            document.serialize(),
            "# stay\nfont-size = 13\nlanguage = en\n"
        );
    }

    #[test]
    fn crlf_documents_keep_crlf_when_one_value_changes() {
        let source = "# c\r\nfont-size = 13\r\nunknown = x\r\n";
        let mut document = ConfigDocument::parse(source);
        document.set("font-size", "20");
        assert_eq!(
            document.serialize(),
            "# c\r\nfont-size = 20\r\nunknown = x\r\n"
        );
    }
}
