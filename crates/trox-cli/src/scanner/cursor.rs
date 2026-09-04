//! Cursor primitives shared by the restricted pattern and argument parsers.

use crate::config::Language;

use super::lexer::{
    SkippedKind, decode_literal_prefix, identifier_allows_regex, identifier_end, is_ident_continue,
    is_ident_start, punctuation_end_and_regex_allowed, skip_token,
};

pub(super) struct StringToken {
    pub(super) decoded: String,
}

#[derive(Debug)]
pub(super) struct ParseError {
    pub(super) offset: usize,
    pub(super) rule: &'static str,
    pub(super) message: String,
}

pub(super) type ParseResult<T> = Result<T, ParseError>;

pub(super) struct Cursor<'a> {
    pub(super) source: &'a str,
    pub(super) index: usize,
    pub(super) language: Language,
}

impl<'a> Cursor<'a> {
    pub(super) fn new(source: &'a str, index: usize, language: Language) -> Self {
        Self {
            source,
            index,
            language,
        }
    }

    pub(super) fn error<T>(
        &self,
        rule: &'static str,
        message: impl Into<String>,
    ) -> ParseResult<T> {
        Err(ParseError {
            offset: self.index,
            rule,
            message: message.into(),
        })
    }

    pub(super) fn skip_space_comments(&mut self) {
        loop {
            while self
                .source
                .as_bytes()
                .get(self.index)
                .is_some_and(u8::is_ascii_whitespace)
            {
                self.index += 1;
            }
            if let Some(skipped) = skip_token(self.source, self.index, self.language, false)
                && skipped.kind == SkippedKind::Comment
            {
                self.index = skipped.end;
                continue;
            }
            break;
        }
    }

    pub(super) fn consume_char(&mut self, value: char) -> bool {
        self.skip_space_comments();
        if self.source[self.index..].starts_with(value) {
            self.index += value.len_utf8();
            true
        } else {
            false
        }
    }

    pub(super) fn expect_char(&mut self, value: char) -> ParseResult<()> {
        if self.consume_char(value) {
            Ok(())
        } else {
            self.error("trox.syntax", format!("expected `{value}`"))
        }
    }

    pub(super) fn optional_trailing_comma(&mut self) {
        self.consume_char(',');
    }

    pub(super) fn is_at_end(&mut self) -> bool {
        self.skip_space_comments();
        self.index == self.source.len()
    }

    pub(super) fn expect_end(&mut self) -> ParseResult<()> {
        if self.is_at_end() {
            Ok(())
        } else {
            self.error("trox.argument-map", "unexpected trailing argument syntax")
        }
    }

    pub(super) fn parse_ident(&mut self) -> ParseResult<String> {
        self.skip_space_comments();
        let start = self.index;
        let bytes = self.source.as_bytes();
        if !bytes
            .get(self.index)
            .is_some_and(|byte| is_ident_start(*byte))
        {
            return self.error("trox.syntax", "expected identifier");
        }
        self.index += 1;
        while bytes
            .get(self.index)
            .is_some_and(|byte| is_ident_continue(*byte))
        {
            self.index += 1;
        }
        Ok(self.source[start..self.index].to_owned())
    }

    pub(super) fn parse_u64(&mut self) -> ParseResult<u64> {
        self.skip_space_comments();
        let start = self.index;
        while self
            .source
            .as_bytes()
            .get(self.index)
            .is_some_and(|byte| byte.is_ascii_digit() || *byte == b'_')
        {
            self.index += 1;
        }
        self.source[start..self.index]
            .replace('_', "")
            .parse()
            .map_err(|_| ParseError {
                offset: start,
                rule: "trox.invalid-selector-number",
                message: "expected nonnegative integer literal".into(),
            })
    }

    pub(super) fn peek_literal_start(&mut self) -> bool {
        self.skip_space_comments();
        let rest = &self.source[self.index..];
        rest.starts_with('"')
            || rest.starts_with('\'')
            || rest.starts_with('`')
            || (self.language != Language::TypeScript
                && (rest.starts_with('r') || rest.starts_with("br")))
    }

    pub(super) fn parse_string(&mut self) -> ParseResult<StringToken> {
        self.skip_space_comments();
        let Some((decoded, consumed)) =
            decode_literal_prefix(&self.source[self.index..], self.language)
        else {
            return self.error("trox.literal-required", "expected one literal string token");
        };
        self.index += consumed;
        Ok(StringToken { decoded })
    }

    pub(super) fn take_until_top_level(&mut self, delimiter: char) -> ParseResult<String> {
        self.skip_space_comments();
        let start = self.index;
        let mut stack = Vec::new();
        let mut regex_allowed = true;
        let bytes = self.source.as_bytes();
        while self.index < bytes.len() {
            if let Some(skipped) = skip_token(self.source, self.index, self.language, regex_allowed)
            {
                if skipped.kind == SkippedKind::Value {
                    regex_allowed = false;
                }
                self.index = skipped.end;
                continue;
            }
            if is_ident_start(bytes[self.index]) {
                let end = identifier_end(bytes, self.index);
                regex_allowed = identifier_allows_regex(&self.source[self.index..end]);
                self.index = end;
                continue;
            }
            let ch = bytes[self.index] as char;
            if stack.is_empty() && ch == delimiter {
                return Ok(self.source[start..self.index].to_owned());
            }
            match ch {
                '(' | '[' | '{' => stack.push(ch),
                ')' | ']' | '}' if stack.pop().is_none() => {
                    return self.error("trox.syntax", "unbalanced expression");
                }
                ')' | ']' | '}' => {}
                _ => {}
            }
            (self.index, regex_allowed) = punctuation_end_and_regex_allowed(
                self.source,
                self.index,
                self.language,
                regex_allowed,
            );
        }
        self.error("trox.syntax", format!("expected `{delimiter}`"))
    }

    pub(super) fn take_balanced_content(&mut self, close: char) -> ParseResult<String> {
        let start = self.index;
        let mut stack = Vec::new();
        let mut regex_allowed = true;
        let bytes = self.source.as_bytes();
        while self.index < bytes.len() {
            if let Some(skipped) = skip_token(self.source, self.index, self.language, regex_allowed)
            {
                if skipped.kind == SkippedKind::Value {
                    regex_allowed = false;
                }
                self.index = skipped.end;
                continue;
            }
            if is_ident_start(bytes[self.index]) {
                let end = identifier_end(bytes, self.index);
                regex_allowed = identifier_allows_regex(&self.source[self.index..end]);
                self.index = end;
                continue;
            }
            let ch = bytes[self.index] as char;
            if stack.is_empty() && ch == close {
                let content = self.source[start..self.index].to_owned();
                self.index += 1;
                return Ok(content);
            }
            match ch {
                '(' | '[' | '{' => stack.push(ch),
                ')' | ']' | '}' => {
                    stack.pop();
                }
                _ => {}
            }
            (self.index, regex_allowed) = punctuation_end_and_regex_allowed(
                self.source,
                self.index,
                self.language,
                regex_allowed,
            );
        }
        self.error(
            "trox.syntax",
            format!("unclosed argument map, expected `{close}`"),
        )
    }
}
