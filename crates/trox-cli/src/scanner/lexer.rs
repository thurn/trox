//! Lightweight host-language lexical support for the restricted Trox scanner.
//!
//! This is deliberately not a host-language parser. It owns only the boundary
//! rules needed to skip non-code tokens safely and to decode one literal token
//! with the same value the host runtime observes.

use std::ops::Range;

use crate::config::Language;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SkippedKind {
    Comment,
    Value,
}

#[derive(Debug)]
pub(super) struct SkippedToken {
    pub end: usize,
    pub kind: SkippedKind,
    /// Code ranges inside a TypeScript template literal's `${...}` sections.
    pub template_expressions: Vec<Range<usize>>,
}

pub(super) struct SkippedJsx {
    pub(super) end: usize,
    pub(super) expressions: Vec<Range<usize>>,
}

/// Skips one complete TSX element while returning only its executable `{...}`
/// ranges for ordinary TypeScript scanning. Returning `None` leaves `<` to the
/// normal expression lexer, which avoids mistaking comparisons and generics
/// for JSX.
pub(super) fn skip_jsx_element(source: &str, index: usize) -> Option<SkippedJsx> {
    let opening = parse_jsx_tag(source, index, false)?;
    let mut expressions = opening.expressions;
    if opening.self_closing {
        return Some(SkippedJsx {
            end: opening.end,
            expressions,
        });
    }

    let mut stack = vec![opening.name];
    let mut cursor = opening.end;
    while cursor < source.len() {
        let bytes = source.as_bytes();
        if bytes[cursor] == b'{' {
            let end = typescript_brace_expression_end(source, cursor + 1)?;
            expressions.push(cursor + 1..end - 1);
            cursor = end;
            continue;
        }
        if bytes[cursor] != b'<' {
            cursor = char_end(source, cursor);
            continue;
        }
        if bytes.get(cursor + 1) == Some(&b'/') {
            let closing = parse_jsx_tag(source, cursor, true)?;
            if stack.pop().as_deref() != Some(closing.name.as_str()) {
                return None;
            }
            cursor = closing.end;
            if stack.is_empty() {
                return Some(SkippedJsx {
                    end: cursor,
                    expressions,
                });
            }
            continue;
        }
        let child = parse_jsx_tag(source, cursor, false)?;
        expressions.extend(child.expressions);
        cursor = child.end;
        if !child.self_closing {
            stack.push(child.name);
        }
    }
    None
}

struct JsxTag {
    name: String,
    end: usize,
    self_closing: bool,
    expressions: Vec<Range<usize>>,
}

fn parse_jsx_tag(source: &str, index: usize, closing: bool) -> Option<JsxTag> {
    let bytes = source.as_bytes();
    if bytes.get(index) != Some(&b'<') {
        return None;
    }
    let mut cursor = index + 1;
    if closing {
        if bytes.get(cursor) != Some(&b'/') {
            return None;
        }
        cursor += 1;
    } else if bytes.get(cursor) == Some(&b'/') {
        return None;
    }

    let name_start = cursor;
    while let Some(byte) = bytes.get(cursor) {
        if byte.is_ascii_whitespace() || matches!(*byte, b'/' | b'>') {
            break;
        }
        if !(byte.is_ascii_alphanumeric()
            || matches!(*byte, b'_' | b'-' | b'.' | b':' | b'$')
            || *byte >= 0x80)
        {
            return None;
        }
        cursor += 1;
    }
    let name = source.get(name_start..cursor)?.to_owned();
    if name.is_empty() && bytes.get(cursor) != Some(&b'>') {
        return None;
    }
    if !name.is_empty()
        && !name
            .as_bytes()
            .first()
            .is_some_and(|byte| is_ident_start(*byte))
    {
        return None;
    }

    let mut expressions = Vec::new();
    loop {
        while bytes.get(cursor).is_some_and(u8::is_ascii_whitespace) {
            cursor += 1;
        }
        if closing {
            if bytes.get(cursor) != Some(&b'>') {
                return None;
            }
            return Some(JsxTag {
                name,
                end: cursor + 1,
                self_closing: false,
                expressions,
            });
        }
        if bytes.get(cursor..cursor + 2) == Some(b"/>") {
            return Some(JsxTag {
                name,
                end: cursor + 2,
                self_closing: true,
                expressions,
            });
        }
        if bytes.get(cursor) == Some(&b'>') {
            return Some(JsxTag {
                name,
                end: cursor + 1,
                self_closing: false,
                expressions,
            });
        }
        match bytes.get(cursor).copied()? {
            b'\'' | b'"' => cursor = quoted_end(source, cursor, bytes[cursor])?,
            b'{' => {
                let end = typescript_brace_expression_end(source, cursor + 1)?;
                expressions.push(cursor + 1..end - 1);
                cursor = end;
            }
            _ => cursor = char_end(source, cursor),
        }
    }
}

pub(super) fn skip_token(
    source: &str,
    index: usize,
    language: Language,
    regex_allowed: bool,
) -> Option<SkippedToken> {
    let bytes = source.as_bytes();
    if bytes.get(index..index + 2) == Some(b"//") {
        let end = bytes[index..]
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(bytes.len(), |offset| index + offset + 1);
        return Some(SkippedToken {
            end,
            kind: SkippedKind::Comment,
            template_expressions: vec![],
        });
    }
    if bytes.get(index..index + 2) == Some(b"/*") {
        return Some(SkippedToken {
            end: block_comment_end(source, index, language).unwrap_or(bytes.len()),
            kind: SkippedKind::Comment,
            template_expressions: vec![],
        });
    }

    match language {
        Language::TypeScript => match bytes.get(index).copied()? {
            b'\'' | b'"' => Some(SkippedToken {
                end: quoted_end(source, index, bytes[index]).unwrap_or(bytes.len()),
                kind: SkippedKind::Value,
                template_expressions: vec![],
            }),
            b'`' => {
                let (end, template_expressions) = template_end(source, index);
                Some(SkippedToken {
                    end,
                    kind: SkippedKind::Value,
                    template_expressions,
                })
            }
            b'/' if regex_allowed => regex_end(source, index).map(|end| SkippedToken {
                end,
                kind: SkippedKind::Value,
                template_expressions: vec![],
            }),
            _ => None,
        },
        Language::Rust | Language::Ron => {
            if let Some(end) = rust_raw_end(source, index, true) {
                return Some(SkippedToken {
                    end,
                    kind: SkippedKind::Value,
                    template_expressions: vec![],
                });
            }
            let (prefix, quote) = match bytes.get(index..index + 2) {
                Some(b"b\"") => (1, b'"'),
                Some(b"b'") if language == Language::Rust => (1, b'\''),
                _ => (0, *bytes.get(index)?),
            };
            if quote == b'"' || quote == b'\'' && language == Language::Rust {
                let start = index + prefix;
                if quote == b'\'' && !looks_like_rust_char(source, start) {
                    return None;
                }
                return Some(SkippedToken {
                    end: quoted_end(source, start, quote).unwrap_or(bytes.len()),
                    kind: SkippedKind::Value,
                    template_expressions: vec![],
                });
            }
            None
        }
    }
}

pub(super) fn decode_literal_prefix(input: &str, language: Language) -> Option<(String, usize)> {
    match language {
        Language::TypeScript => decode_typescript_string(input),
        Language::Rust | Language::Ron => decode_rust_string(input),
    }
}

pub(super) fn char_end(source: &str, index: usize) -> usize {
    index
        + source
            .get(index..)
            .and_then(|rest| rest.chars().next())
            .map_or(1, char::len_utf8)
}

pub(super) fn identifier_end(bytes: &[u8], mut index: usize) -> usize {
    while bytes.get(index).is_some_and(|byte| {
        byte.is_ascii_alphanumeric() || matches!(*byte, b'_' | b'$') || *byte >= 0x80
    }) {
        index += 1;
    }
    index
}

pub(super) fn is_ident_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || matches!(byte, b'_' | b'$') || byte >= 0x80
}

pub(super) fn is_ident_continue(byte: u8) -> bool {
    is_ident_start(byte) || byte.is_ascii_digit()
}

pub(super) fn previous_nonspace(bytes: &[u8], mut index: usize) -> Option<u8> {
    while index > 0 {
        index -= 1;
        if !bytes[index].is_ascii_whitespace() {
            return Some(bytes[index]);
        }
    }
    None
}

pub(super) fn is_host_numeric_literal(value: &str, language: Language) -> bool {
    let mut value = value.trim();
    if let Some(rest) = value.strip_prefix(['+', '-']) {
        value = rest.trim_start();
    }
    if value.is_empty() {
        return false;
    }

    value = match language {
        Language::TypeScript => value.strip_suffix('n').unwrap_or(value),
        Language::Rust | Language::Ron => strip_rust_numeric_suffix(value),
    };
    let compact = value.replace('_', "");
    let digits =
        |rest: &str, radix: u32| !rest.is_empty() && rest.chars().all(|ch| ch.is_digit(radix));
    if let Some(rest) = compact
        .strip_prefix("0x")
        .or_else(|| compact.strip_prefix("0X"))
    {
        return digits(rest, 16);
    }
    if let Some(rest) = compact
        .strip_prefix("0o")
        .or_else(|| compact.strip_prefix("0O"))
    {
        return digits(rest, 8);
    }
    if let Some(rest) = compact
        .strip_prefix("0b")
        .or_else(|| compact.strip_prefix("0B"))
    {
        return digits(rest, 2);
    }
    compact.parse::<f64>().is_ok()
}

fn strip_rust_numeric_suffix(value: &str) -> &str {
    const SUFFIXES: [&str; 14] = [
        "isize", "usize", "i128", "u128", "i64", "u64", "f64", "i32", "u32", "f32", "i16", "u16",
        "i8", "u8",
    ];
    SUFFIXES
        .iter()
        .find_map(|suffix| value.strip_suffix(suffix))
        .unwrap_or(value)
}

pub(super) fn identifier_allows_regex(identifier: &str) -> bool {
    matches!(
        identifier,
        "await"
            | "case"
            | "delete"
            | "do"
            | "else"
            | "in"
            | "instanceof"
            | "new"
            | "of"
            | "return"
            | "throw"
            | "typeof"
            | "void"
            | "yield"
    )
}

pub(super) fn regex_allowed_after_byte(byte: u8, previous: bool) -> bool {
    if byte.is_ascii_whitespace() {
        previous
    } else if byte.is_ascii_alphanumeric() || byte == b'_' {
        false
    } else {
        !matches!(byte, b')' | b']' | b'}' | b'.')
    }
}

/// Advances one punctuation token while preserving the expression-prefix state
/// needed to distinguish TypeScript regular expressions from division.
pub(super) fn punctuation_end_and_regex_allowed(
    source: &str,
    index: usize,
    language: Language,
    previous: bool,
) -> (usize, bool) {
    let bytes = source.as_bytes();
    if language == Language::TypeScript
        && matches!(bytes.get(index..index + 2), Some(b"++" | b"--"))
    {
        // Prefix ++/-- still expects an expression; postfix ++/-- completes one.
        return (index + 2, previous);
    }
    if language == Language::TypeScript && bytes.get(index) == Some(&b'!') {
        // `!value` is prefix when an expression is expected; `value!` is the
        // TypeScript non-null postfix operator when one has just completed.
        return (index + 1, previous);
    }
    let byte = bytes[index];
    if byte.is_ascii() {
        (index + 1, regex_allowed_after_byte(byte, previous))
    } else {
        (char_end(source, index), false)
    }
}

pub(super) fn split_top_level(input: &str, delimiter: char, language: Language) -> Vec<&str> {
    top_level_items(input, delimiter, language).collect()
}

pub(super) fn top_level_items(
    input: &str,
    delimiter: char,
    language: Language,
) -> TopLevelItems<'_> {
    TopLevelItems {
        input,
        delimiter,
        language,
        start: 0,
        index: 0,
        stack: Vec::new(),
        regex_allowed: true,
        finished: false,
    }
}

pub(super) struct TopLevelItems<'a> {
    input: &'a str,
    delimiter: char,
    language: Language,
    start: usize,
    index: usize,
    stack: Vec<char>,
    regex_allowed: bool,
    finished: bool,
}

impl<'a> Iterator for TopLevelItems<'a> {
    type Item = &'a str;

    fn next(&mut self) -> Option<Self::Item> {
        if self.finished {
            return None;
        }
        let bytes = self.input.as_bytes();
        while self.index < bytes.len() {
            if let Some(skipped) =
                skip_token(self.input, self.index, self.language, self.regex_allowed)
            {
                if skipped.kind == SkippedKind::Value {
                    self.regex_allowed = false;
                }
                self.index = skipped.end;
                continue;
            }
            if is_ident_start(bytes[self.index]) {
                let end = identifier_end(bytes, self.index);
                self.regex_allowed = identifier_allows_regex(&self.input[self.index..end]);
                self.index = end;
                continue;
            }
            let ch = bytes[self.index] as char;
            if self.stack.is_empty() && ch == self.delimiter {
                let item = &self.input[self.start..self.index];
                self.index += self.delimiter.len_utf8();
                self.start = self.index;
                self.regex_allowed = true;
                return Some(item);
            }
            match ch {
                '(' | '[' | '{' => self.stack.push(ch),
                ')' | ']' | '}' => {
                    self.stack.pop();
                }
                _ => {}
            }
            (self.index, self.regex_allowed) = punctuation_end_and_regex_allowed(
                self.input,
                self.index,
                self.language,
                self.regex_allowed,
            );
        }
        self.finished = true;
        Some(&self.input[self.start..])
    }
}

pub(super) fn split_top_level_once<'a>(
    input: &'a str,
    delimiter: &str,
    language: Language,
) -> Option<(&'a str, &'a str)> {
    let mut stack = Vec::new();
    let mut index = 0;
    let mut regex_allowed = true;
    let bytes = input.as_bytes();
    while index < bytes.len() {
        if let Some(skipped) = skip_token(input, index, language, regex_allowed) {
            if skipped.kind == SkippedKind::Value {
                regex_allowed = false;
            }
            index = skipped.end;
            continue;
        }
        if bytes[index].is_ascii_alphabetic() || bytes[index] == b'_' {
            let end = identifier_end(bytes, index);
            regex_allowed = identifier_allows_regex(&input[index..end]);
            index = end;
            continue;
        }
        if stack.is_empty() && input[index..].starts_with(delimiter) {
            return Some((&input[..index], &input[index + delimiter.len()..]));
        }
        match bytes[index] as char {
            '(' | '[' | '{' => stack.push(bytes[index]),
            ')' | ']' | '}' => {
                stack.pop();
            }
            _ => {}
        }
        (index, regex_allowed) =
            punctuation_end_and_regex_allowed(input, index, language, regex_allowed);
    }
    None
}

fn block_comment_end(source: &str, index: usize, language: Language) -> Option<usize> {
    let bytes = source.as_bytes();
    let nested = language != Language::TypeScript;
    let mut cursor = index + 2;
    let mut depth = 1usize;
    while cursor + 1 < bytes.len() {
        match &bytes[cursor..cursor + 2] {
            b"/*" if nested => {
                depth += 1;
                cursor += 2;
            }
            b"*/" => {
                depth -= 1;
                cursor += 2;
                if depth == 0 {
                    return Some(cursor);
                }
            }
            _ => cursor += 1,
        }
    }
    None
}

fn quoted_end(source: &str, index: usize, quote: u8) -> Option<usize> {
    let bytes = source.as_bytes();
    let mut cursor = index + 1;
    while cursor < bytes.len() {
        match bytes[cursor] {
            value if value == quote => return Some(cursor + 1),
            b'\\' => {
                cursor += 1;
                if bytes.get(cursor) == Some(&b'\r') && bytes.get(cursor + 1) == Some(&b'\n') {
                    cursor += 2;
                } else {
                    cursor += 1;
                }
            }
            _ => cursor += 1,
        }
    }
    None
}

fn looks_like_rust_char(source: &str, index: usize) -> bool {
    let bytes = source.as_bytes();
    let mut cursor = index + 1;
    if bytes.get(cursor) == Some(&b'\\') {
        cursor += 1;
        match bytes.get(cursor).copied() {
            Some(b'u') => {
                cursor += 1;
                if bytes.get(cursor) != Some(&b'{') {
                    return false;
                }
                cursor += 1;
                while bytes.get(cursor).is_some_and(u8::is_ascii_hexdigit) {
                    cursor += 1;
                }
                if bytes.get(cursor) != Some(&b'}') {
                    return false;
                }
                cursor += 1;
            }
            Some(b'x') => cursor += 3,
            Some(_) => cursor += 1,
            None => return false,
        }
    } else if cursor < bytes.len() {
        cursor = char_end(source, cursor);
    }
    bytes.get(cursor) == Some(&b'\'')
}

fn rust_raw_end(input: &str, index: usize, include_bytes: bool) -> Option<usize> {
    let bytes = input.as_bytes();
    let prefix = if include_bytes && bytes.get(index..index + 2) == Some(b"br") {
        2
    } else if bytes.get(index) == Some(&b'r') {
        1
    } else {
        return None;
    };
    let mut hashes = 0;
    while bytes.get(index + prefix + hashes) == Some(&b'#') {
        hashes += 1;
    }
    if bytes.get(index + prefix + hashes) != Some(&b'"') {
        return None;
    }
    let content = index + prefix + hashes + 1;
    let mut cursor = content;
    while cursor < bytes.len() {
        if bytes[cursor] == b'"'
            && bytes.get(cursor + 1..cursor + 1 + hashes)
                == Some(&bytes[index + prefix..index + prefix + hashes])
        {
            return Some(cursor + 1 + hashes);
        }
        cursor += 1;
    }
    None
}

fn decode_rust_string(input: &str) -> Option<(String, usize)> {
    if input.starts_with('r') {
        let end = rust_raw_end(input, 0, false)?;
        let bytes = input.as_bytes();
        let hashes = bytes[1..].iter().take_while(|byte| **byte == b'#').count();
        let content = 1 + hashes + 1;
        return Some((input[content..end - hashes - 1].to_owned(), end));
    }
    if !input.starts_with('"') {
        return None;
    }
    let bytes = input.as_bytes();
    let mut output = String::new();
    let mut index = 1;
    while index < bytes.len() {
        match bytes[index] {
            b'"' => return Some((output, index + 1)),
            b'\\' => {
                index += 1;
                match *bytes.get(index)? {
                    b'n' => {
                        output.push('\n');
                        index += 1;
                    }
                    b'r' => {
                        output.push('\r');
                        index += 1;
                    }
                    b't' => {
                        output.push('\t');
                        index += 1;
                    }
                    b'0' => {
                        output.push('\0');
                        index += 1;
                    }
                    b'\\' => {
                        output.push('\\');
                        index += 1;
                    }
                    b'"' => {
                        output.push('"');
                        index += 1;
                    }
                    b'\'' => {
                        output.push('\'');
                        index += 1;
                    }
                    b'x' => {
                        let value =
                            u8::from_str_radix(input.get(index + 1..index + 3)?, 16).ok()?;
                        if value > 0x7f {
                            return None;
                        }
                        output.push(value as char);
                        index += 3;
                    }
                    b'u' => {
                        if bytes.get(index + 1) != Some(&b'{') {
                            return None;
                        }
                        let end =
                            bytes[index + 2..].iter().position(|byte| *byte == b'}')? + index + 2;
                        let digits = input[index + 2..end].replace('_', "");
                        let scalar = u32::from_str_radix(&digits, 16).ok()?;
                        output.push(char::from_u32(scalar)?);
                        index = end + 1;
                    }
                    b'\n' => {
                        index += 1;
                        while bytes.get(index).is_some_and(u8::is_ascii_whitespace) {
                            index += 1;
                        }
                    }
                    b'\r' if bytes.get(index + 1) == Some(&b'\n') => {
                        index += 2;
                        while bytes.get(index).is_some_and(u8::is_ascii_whitespace) {
                            index += 1;
                        }
                    }
                    _ => return None,
                }
            }
            _ => {
                let end = char_end(input, index);
                output.push_str(&input[index..end]);
                index = end;
            }
        }
    }
    None
}

fn decode_typescript_string(input: &str) -> Option<(String, usize)> {
    let quote = *input.as_bytes().first()?;
    if !matches!(quote, b'"' | b'\'' | b'`') {
        return None;
    }
    let bytes = input.as_bytes();
    let mut output = String::new();
    let mut index = 1;
    while index < bytes.len() {
        if bytes[index] == quote {
            return Some((output, index + 1));
        }
        if quote == b'`' && bytes.get(index..index + 2) == Some(b"${") {
            return None;
        }
        if bytes[index] != b'\\' {
            let end = char_end(input, index);
            output.push_str(&input[index..end]);
            index = end;
            continue;
        }
        index += 1;
        match *bytes.get(index)? {
            b'n' => output.push('\n'),
            b'r' => output.push('\r'),
            b't' => output.push('\t'),
            b'b' => output.push('\u{0008}'),
            b'f' => output.push('\u{000c}'),
            b'v' => output.push('\u{000b}'),
            b'0' if !bytes.get(index + 1).is_some_and(u8::is_ascii_digit) => output.push('\0'),
            b'\\' => output.push('\\'),
            b'"' => output.push('"'),
            b'\'' => output.push('\''),
            b'`' => output.push('`'),
            b'x' => {
                let scalar = u8::from_str_radix(input.get(index + 1..index + 3)?, 16).ok()?;
                output.push(scalar as char);
                index += 2;
            }
            b'u' => {
                let (first, consumed) = decode_typescript_unicode_escape(input, index)?;
                index += consumed - 1;
                if (0xd800..=0xdbff).contains(&first) {
                    if bytes.get(index + 1..index + 3) != Some(b"\\u") {
                        return None;
                    }
                    let (second, low_consumed) =
                        decode_typescript_unicode_escape(input, index + 2)?;
                    if !(0xdc00..=0xdfff).contains(&second) {
                        return None;
                    }
                    let scalar = 0x1_0000 + ((first - 0xd800) << 10) + (second - 0xdc00);
                    output.push(char::from_u32(scalar)?);
                    index += low_consumed + 1;
                } else {
                    output.push(char::from_u32(first)?);
                }
            }
            b'\n' => {}
            b'\r' => {
                if bytes.get(index + 1) == Some(&b'\n') {
                    index += 1;
                }
            }
            other if other.is_ascii() => output.push(other as char),
            _ => {
                let end = char_end(input, index);
                output.push_str(&input[index..end]);
                index = end;
                continue;
            }
        }
        index += 1;
    }
    None
}

/// Returns the scalar/code-unit and bytes consumed starting at the `u`.
fn decode_typescript_unicode_escape(input: &str, index: usize) -> Option<(u32, usize)> {
    let bytes = input.as_bytes();
    if bytes.get(index + 1) == Some(&b'{') {
        let end = bytes[index + 2..].iter().position(|byte| *byte == b'}')? + index + 2;
        let value = u32::from_str_radix(input.get(index + 2..end)?, 16).ok()?;
        (value <= 0x10ffff).then_some((value, end - index + 1))
    } else {
        let value = u32::from_str_radix(input.get(index + 1..index + 5)?, 16).ok()?;
        Some((value, 5))
    }
}

fn regex_end(source: &str, index: usize) -> Option<usize> {
    let bytes = source.as_bytes();
    let mut cursor = index + 1;
    let mut in_class = false;
    while cursor < bytes.len() {
        match bytes[cursor] {
            b'\\' => cursor += 2,
            b'[' => {
                in_class = true;
                cursor += 1;
            }
            b']' => {
                in_class = false;
                cursor += 1;
            }
            b'/' if !in_class => {
                cursor += 1;
                while bytes
                    .get(cursor)
                    .is_some_and(|byte| byte.is_ascii_alphabetic())
                {
                    cursor += 1;
                }
                return Some(cursor);
            }
            b'\n' | b'\r' => return None,
            _ => cursor += 1,
        }
    }
    None
}

fn template_end(source: &str, index: usize) -> (usize, Vec<Range<usize>>) {
    let bytes = source.as_bytes();
    let mut cursor = index + 1;
    let mut expressions = Vec::new();
    while cursor < bytes.len() {
        match bytes[cursor] {
            b'\\' => cursor = (cursor + 2).min(bytes.len()),
            b'`' => return (cursor + 1, expressions),
            b'$' if bytes.get(cursor + 1) == Some(&b'{') => {
                let start = cursor + 2;
                let end = typescript_brace_expression_end(source, start).unwrap_or(bytes.len());
                expressions.push(start..end.saturating_sub(1));
                cursor = end;
            }
            _ => cursor += 1,
        }
    }
    (bytes.len(), expressions)
}

fn typescript_brace_expression_end(source: &str, start: usize) -> Option<usize> {
    let bytes = source.as_bytes();
    let mut cursor = start;
    let mut depth = 1usize;
    let mut regex_allowed = true;
    while cursor < bytes.len() {
        if let Some(skipped) = skip_token(source, cursor, Language::TypeScript, regex_allowed) {
            if skipped.kind == SkippedKind::Value {
                regex_allowed = false;
            }
            cursor = skipped.end;
            continue;
        }
        if bytes[cursor].is_ascii_alphabetic() || bytes[cursor] == b'_' {
            let end = identifier_end(bytes, cursor);
            regex_allowed = identifier_allows_regex(&source[cursor..end]);
            cursor = end;
            continue;
        }
        match bytes[cursor] {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(cursor + 1);
                }
            }
            _ => {}
        }
        (cursor, regex_allowed) =
            punctuation_end_and_regex_allowed(source, cursor, Language::TypeScript, regex_allowed);
    }
    None
}
