use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use trox::{
    IdentityDescriptor, NumericBranch, NumericBranchKey, Pattern, PluralCategory,
    SelectIdentityBranch, identity_ids,
};
use unicode_normalization::UnicodeNormalization;

use crate::config::Language;
use crate::diagnostic::{Diagnostic, Span};

mod argument;
mod cursor;
mod lexer;
mod model;

#[cfg(test)]
use argument::classify_argument;
use argument::parse_argument_map;
use cursor::{Cursor, ParseError, ParseResult};
use lexer::{
    SkippedKind, identifier_allows_regex, identifier_end, is_host_numeric_literal, is_ident_start,
    previous_nonspace, punctuation_end_and_regex_allowed, skip_token,
};
pub use model::{ArgumentSchema, ExtractedMessage, ScanResult, SourceLocation};

pub fn scan_file(
    path: &Path,
    language: Language,
    ron_default_description: Option<&str>,
) -> ScanResult {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) => {
            let mut diagnostic = Diagnostic::error("trox.source-read", error.to_string());
            diagnostic.path = Some(path.to_path_buf());
            return ScanResult {
                messages: vec![],
                diagnostics: vec![diagnostic],
                bytes_scanned: 0,
            };
        }
    };
    let source = match std::str::from_utf8(&bytes) {
        Ok(source) => source,
        Err(error) => {
            let mut diagnostic = Diagnostic::error("trox.source-encoding", error.to_string());
            diagnostic.path = Some(path.to_path_buf());
            return ScanResult {
                messages: vec![],
                diagnostics: vec![diagnostic],
                bytes_scanned: bytes.len() as u64,
            };
        }
    };
    let mut scanner = Scanner {
        source,
        path,
        language,
        tsx: language == Language::TypeScript
            && path.extension().is_some_and(|extension| extension == "tsx"),
        ron_default_description,
        line_starts: line_starts(source),
        messages: vec![],
        diagnostics: vec![],
    };
    scanner.scan();
    ScanResult {
        messages: scanner.messages,
        diagnostics: scanner.diagnostics,
        bytes_scanned: bytes.len() as u64,
    }
}

struct Scanner<'a> {
    source: &'a str,
    path: &'a Path,
    language: Language,
    tsx: bool,
    ron_default_description: Option<&'a str>,
    line_starts: Vec<usize>,
    messages: Vec<ExtractedMessage>,
    diagnostics: Vec<Diagnostic>,
}

impl Scanner<'_> {
    fn scan(&mut self) {
        self.scan_range(0, self.source.len());
    }

    fn scan_range(&mut self, start_index: usize, end_index: usize) {
        let bytes = self.source.as_bytes();
        let mut index = start_index;
        let mut regex_allowed = true;
        let mut delimiter_stack = Vec::new();
        let mut brace_is_block = Vec::new();
        let mut closed_parenthesized_expression = false;
        let mut next_brace_is_block = false;
        while index < end_index {
            if self.tsx
                && bytes[index] == b'<'
                && regex_allowed
                && let Some(jsx) = lexer::skip_jsx_element(self.source, index)
            {
                for expression in jsx.expressions {
                    if expression.start < expression.end {
                        self.scan_range(expression.start, expression.end);
                    }
                }
                index = jsx.end.min(end_index);
                regex_allowed = false;
                continue;
            }
            if let Some(skipped) = skip_token(self.source, index, self.language, regex_allowed) {
                for expression in skipped.template_expressions {
                    if expression.start < expression.end {
                        self.scan_range(expression.start, expression.end);
                    }
                }
                if skipped.kind == SkippedKind::Value {
                    regex_allowed = false;
                }
                index = skipped.end.min(end_index);
                continue;
            }
            if is_ident_start(bytes[index]) {
                let start = index;
                index = identifier_end(bytes, index);
                let identifier = &self.source[start..index];
                regex_allowed = identifier_allows_regex(identifier);
                closed_parenthesized_expression = false;
                if self.language == Language::TypeScript
                    && matches!(
                        identifier,
                        "class"
                            | "interface"
                            | "namespace"
                            | "enum"
                            | "function"
                            | "else"
                            | "try"
                            | "finally"
                            | "do"
                    )
                {
                    next_brace_is_block = true;
                }
                let wanted = match self.language {
                    Language::Ron => identifier == "Tx",
                    Language::Rust | Language::TypeScript => {
                        identifier == "tx" || identifier == "txa"
                    }
                };
                if !wanted || matches!(previous_nonspace(bytes, start), Some(b'.' | b'#')) {
                    continue;
                }
                let mut cursor = Cursor::new(self.source, index, self.language);
                cursor.skip_space_comments();
                if !cursor.consume_char('(') {
                    continue;
                }
                let call_content_start = cursor.index;
                let result = if self.language == Language::Ron {
                    parse_ron_tx(&mut cursor, self.ron_default_description)
                } else {
                    parse_code_call(&mut cursor, identifier == "txa")
                };
                match result {
                    Ok(parsed) => {
                        let (line, column) = self.line_column(start);
                        match parsed.finish(self.path, line, column) {
                            Ok(message) => self.messages.push(message),
                            Err(message) => self.push_error(start, "trox.source-contract", message),
                        }
                    }
                    Err(error) => self.push_error(error.offset, error.rule, error.message),
                }
                if self.language != Language::Ron
                    && call_content_start < cursor.index.saturating_sub(1)
                {
                    self.scan_range(call_content_start, cursor.index.saturating_sub(1));
                }
                index = cursor.index.max(index).min(end_index);
                regex_allowed = false;
            } else {
                match bytes[index] {
                    b'(' | b'[' => {
                        delimiter_stack.push(bytes[index]);
                        closed_parenthesized_expression = false;
                    }
                    b')' => {
                        delimiter_stack.pop();
                        closed_parenthesized_expression = true;
                    }
                    b']' => {
                        delimiter_stack.pop();
                        closed_parenthesized_expression = false;
                    }
                    b'{' => {
                        delimiter_stack.push(b'{');
                        brace_is_block.push(closed_parenthesized_expression || next_brace_is_block);
                        closed_parenthesized_expression = false;
                        next_brace_is_block = false;
                    }
                    b'}' => {
                        delimiter_stack.pop();
                        let is_block = brace_is_block.pop().unwrap_or(false);
                        let next = index + 1;
                        regex_allowed = is_block;
                        index = next;
                        closed_parenthesized_expression = false;
                        continue;
                    }
                    b'=' if self.language == Language::TypeScript
                        && bytes.get(index + 1) == Some(&b'>') =>
                    {
                        next_brace_is_block = true;
                    }
                    b';' => next_brace_is_block = false,
                    _ if !bytes[index].is_ascii_whitespace() => {
                        closed_parenthesized_expression = false;
                    }
                    _ => {}
                }
                (index, regex_allowed) = punctuation_end_and_regex_allowed(
                    self.source,
                    index,
                    self.language,
                    regex_allowed,
                );
            }
        }
    }

    fn push_error(&mut self, offset: usize, rule: &str, message: impl Into<String>) {
        let (line, column) = self.line_column(offset);
        self.diagnostics.push(Diagnostic::error(rule, message).at(
            self.path,
            Span {
                line,
                column,
                end_line: line,
                end_column: column + 1,
            },
        ));
    }

    fn line_column(&self, offset: usize) -> (usize, usize) {
        let index = self
            .line_starts
            .partition_point(|start| *start <= offset)
            .saturating_sub(1);
        (
            index + 1,
            offset.saturating_sub(self.line_starts[index]) + 1,
        )
    }
}

struct ParsedCall {
    pattern: Pattern,
    meaning: Option<String>,
    description: Option<String>,
    arguments: BTreeMap<String, ArgumentSchema>,
    term_ids: BTreeMap<String, Option<String>>,
    selector_labels: BTreeMap<Vec<usize>, String>,
    predicate_labels: BTreeMap<Vec<usize>, Vec<String>>,
}

impl ParsedCall {
    fn finish(self, path: &Path, line: usize, column: usize) -> Result<ExtractedMessage, String> {
        let identity = IdentityDescriptor {
            identity_version: 1,
            meaning: self.meaning,
            pattern: self.pattern,
        };
        if let Some(meaning) = &identity.meaning {
            validate_stable_id(meaning, "meaning")?;
        }
        let mut nodes = 0;
        validate_pattern_limits(&identity.pattern, 0, &mut nodes)?;
        let (entry_id, source_signature) =
            identity_ids(&identity).map_err(|error| error.to_string())?;
        Ok(ExtractedMessage {
            identity,
            entry_id,
            source_signature,
            description: self.description,
            arguments: self.arguments,
            term_ids: self.term_ids,
            selector_labels: self.selector_labels,
            predicate_labels: self.predicate_labels,
            location: SourceLocation {
                path: path.to_path_buf(),
                line,
                column,
            },
        })
    }
}

struct ParsedPattern {
    pattern: Pattern,
    meaning: Option<String>,
    selector_labels: BTreeMap<Vec<usize>, String>,
    predicate_labels: BTreeMap<Vec<usize>, Vec<String>>,
}

fn parse_code_call(cursor: &mut Cursor<'_>, has_arguments: bool) -> ParseResult<ParsedCall> {
    let parsed = parse_pattern(cursor)?;
    cursor.expect_char(',')?;
    let parsed_arguments = if has_arguments {
        let arguments = parse_argument_map(cursor)?;
        cursor.expect_char(',')?;
        arguments
    } else {
        argument::ParsedArguments {
            schemas: BTreeMap::new(),
            term_ids: BTreeMap::new(),
        }
    };
    let description = cursor.parse_string()?.decoded;
    cursor.optional_trailing_comma();
    cursor.expect_char(')')?;
    validate_description(&description).map_err(|message| ParseError {
        offset: cursor.index,
        rule: "trox.description",
        message,
    })?;
    validate_placeholder_bindings(&parsed.pattern, &parsed_arguments.schemas, has_arguments)
        .map_err(|message| ParseError {
            offset: cursor.index,
            rule: "trox.argument-mismatch",
            message,
        })?;
    Ok(ParsedCall {
        pattern: parsed.pattern,
        meaning: parsed.meaning,
        description: Some(description),
        arguments: parsed_arguments.schemas,
        term_ids: parsed_arguments.term_ids,
        selector_labels: parsed.selector_labels,
        predicate_labels: parsed.predicate_labels,
    })
}

fn parse_ron_tx(
    cursor: &mut Cursor<'_>,
    default_description: Option<&str>,
) -> ParseResult<ParsedCall> {
    if cursor.peek_literal_start() {
        let text = cursor.parse_string()?.decoded;
        cursor.optional_trailing_comma();
        cursor.expect_char(')')?;
        return finish_ron_tx(cursor, text, None, None, default_description);
    }

    let mut text = None;
    let mut description = None;
    let mut meaning = None;
    let mut seen = BTreeSet::new();
    loop {
        cursor.skip_space_comments();
        if cursor.consume_char(')') {
            break;
        }
        let field = cursor.parse_ident()?;
        if !seen.insert(field.clone()) {
            return cursor.error(
                "trox.ron-duplicate-field",
                format!("duplicate Tx field `{field}`"),
            );
        }
        cursor.expect_char(':')?;
        let value = cursor.parse_string()?.decoded;
        match field.as_str() {
            "text" => text = Some(value),
            "description" => description = Some(value),
            "meaning" => meaning = Some(value),
            _ => {
                return cursor.error(
                    "trox.ron-unknown-field",
                    format!("unknown Tx field `{field}`"),
                );
            }
        }
        cursor.skip_space_comments();
        if cursor.consume_char(',') {
            continue;
        }
        cursor.expect_char(')')?;
        break;
    }
    let text = text.ok_or_else(|| ParseError {
        offset: cursor.index,
        rule: "trox.ron-missing-text",
        message: "Tx requires `text`".into(),
    })?;
    finish_ron_tx(cursor, text, description, meaning, default_description)
}

fn finish_ron_tx(
    cursor: &mut Cursor<'_>,
    text: String,
    description: Option<String>,
    meaning: Option<String>,
    default_description: Option<&str>,
) -> ParseResult<ParsedCall> {
    let placeholders = parse_placeholders(&text).map_err(|message| ParseError {
        offset: cursor.index,
        rule: "trox.invalid-placeholder",
        message,
    })?;
    if !placeholders.is_empty() {
        return cursor.error(
            "trox.ron-placeholder",
            "RON Tx text cannot contain placeholders",
        );
    }
    if let Some(description) = description.as_deref().or(default_description) {
        validate_description(description).map_err(|message| ParseError {
            offset: cursor.index,
            rule: "trox.description",
            message,
        })?;
    }
    Ok(ParsedCall {
        pattern: Pattern::Text { text },
        meaning,
        description: description.or_else(|| default_description.map(str::to_owned)),
        arguments: BTreeMap::new(),
        term_ids: BTreeMap::new(),
        selector_labels: BTreeMap::new(),
        predicate_labels: BTreeMap::new(),
    })
}

fn parse_pattern(cursor: &mut Cursor<'_>) -> ParseResult<ParsedPattern> {
    let mut nodes = 0;
    parse_pattern_inner(cursor, 0, &mut nodes, true)
}

fn parse_pattern_inner(
    cursor: &mut Cursor<'_>,
    selector_depth: usize,
    nodes: &mut usize,
    meaning_allowed: bool,
) -> ParseResult<ParsedPattern> {
    cursor.skip_space_comments();
    if cursor.peek_literal_start() {
        reserve_pattern_node(cursor, selector_depth, nodes, false)?;
        let text = cursor.parse_string()?.decoded;
        parse_placeholders(&text).map_err(|message| ParseError {
            offset: cursor.index,
            rule: "trox.invalid-placeholder",
            message,
        })?;
        return Ok(ParsedPattern {
            pattern: Pattern::Text { text },
            meaning: None,
            selector_labels: BTreeMap::new(),
            predicate_labels: BTreeMap::new(),
        });
    }
    let function = cursor.parse_ident()?;
    match function.as_str() {
        "meaning" => {
            if !meaning_allowed {
                return cursor.error(
                    "trox.duplicate-meaning",
                    "meaning may appear only once around the complete pattern",
                );
            }
            cursor.expect_char('(')?;
            let meaning = cursor.parse_string()?.decoded;
            cursor.expect_char(',')?;
            let mut inner = parse_pattern_inner(cursor, selector_depth, nodes, false)?;
            cursor.optional_trailing_comma();
            cursor.expect_char(')')?;
            if inner.meaning.replace(meaning).is_some() {
                return cursor.error("trox.duplicate-meaning", "meaning may appear only once");
            }
            Ok(inner)
        }
        "plural" | "ordinal" => {
            reserve_pattern_node(cursor, selector_depth, nodes, true)?;
            parse_numeric_pattern(cursor, &function, selector_depth, nodes)
        }
        "select" => {
            reserve_pattern_node(cursor, selector_depth, nodes, true)?;
            parse_select_pattern(cursor, selector_depth, nodes)
        }
        _ => cursor.error(
            "trox.dynamic-pattern",
            format!("pattern must be an inline literal or Trox constructor, found `{function}`"),
        ),
    }
}

fn parse_numeric_pattern(
    cursor: &mut Cursor<'_>,
    kind: &str,
    selector_depth: usize,
    nodes: &mut usize,
) -> ParseResult<ParsedPattern> {
    cursor.expect_char('(')?;
    let selector_label = normalize_expression(&cursor.take_until_top_level(',')?, cursor.language);
    cursor.expect_char(',')?;
    cursor.expect_char('[')?;
    let mut branches = Vec::new();
    let mut selector_labels = BTreeMap::new();
    let mut predicate_labels = BTreeMap::new();
    loop {
        cursor.skip_space_comments();
        if cursor.consume_char(']') {
            break;
        }
        if branches.len() == 256 {
            return cursor.error("trox.branch-limit", "selector exceeds 256 branches");
        }
        let helper = cursor.parse_ident()?;
        cursor.expect_char('(')?;
        let key = if helper == "exact" {
            let value = cursor.parse_u64()?;
            if value > 9_007_199_254_740_991 {
                return cursor.error(
                    "trox.invalid-selector-number",
                    "exact value exceeds 2^53 - 1",
                );
            }
            cursor.expect_char(',')?;
            NumericBranchKey::Exact { exact: value }
        } else {
            let category = parse_category(&helper).ok_or_else(|| ParseError {
                offset: cursor.index,
                rule: "trox.numeric-branch",
                message: format!("unknown numeric branch helper `{helper}`"),
            })?;
            if kind == "plural" {
                NumericBranchKey::Plural { plural: category }
            } else {
                NumericBranchKey::Ordinal { ordinal: category }
            }
        };
        let child = parse_pattern_inner(cursor, selector_depth + 1, nodes, false)?;
        cursor.optional_trailing_comma();
        cursor.expect_char(')')?;
        let index = branches.len();
        merge_prefixed(&mut selector_labels, child.selector_labels, index);
        merge_prefixed(&mut predicate_labels, child.predicate_labels, index);
        branches.push(NumericBranch {
            key,
            pattern: child.pattern,
        });
        cursor.skip_space_comments();
        if cursor.consume_char(',') {
            continue;
        }
        cursor.expect_char(']')?;
        break;
    }
    cursor.optional_trailing_comma();
    cursor.expect_char(')')?;
    validate_numeric_order(&branches, kind).map_err(|message| ParseError {
        offset: cursor.index,
        rule: "trox.branch-order",
        message,
    })?;
    selector_labels.insert(vec![], selector_label);
    Ok(ParsedPattern {
        pattern: if kind == "plural" {
            Pattern::Plural { branches }
        } else {
            Pattern::Ordinal { branches }
        },
        meaning: None,
        selector_labels,
        predicate_labels,
    })
}

fn parse_select_pattern(
    cursor: &mut Cursor<'_>,
    selector_depth: usize,
    nodes: &mut usize,
) -> ParseResult<ParsedPattern> {
    cursor.expect_char('(')?;
    let selector_label = normalize_expression(&cursor.take_until_top_level(',')?, cursor.language);
    if is_host_numeric_literal(&selector_label, cursor.language) {
        return cursor.error(
            "trox.numeric-select",
            "numeric values belong in plural or ordinal, not select",
        );
    }
    cursor.expect_char(',')?;
    cursor.expect_char('[')?;
    let mut branches = Vec::new();
    let mut predicates = Vec::new();
    let mut selector_labels = BTreeMap::new();
    let mut predicate_labels = BTreeMap::new();
    loop {
        cursor.skip_space_comments();
        if cursor.consume_char(']') {
            break;
        }
        if branches.len() == 256 {
            return cursor.error("trox.branch-limit", "selector exceeds 256 branches");
        }
        let helper = cursor.parse_ident()?;
        cursor.expect_char('(')?;
        let predicate = if helper == "when" {
            let expression =
                normalize_expression(&cursor.take_until_top_level(',')?, cursor.language);
            cursor.expect_char(',')?;
            Some(expression)
        } else if helper == "otherwise" {
            None
        } else {
            return cursor.error(
                "trox.select-branch",
                format!("unknown select branch helper `{helper}`"),
            );
        };
        let child = parse_pattern_inner(cursor, selector_depth + 1, nodes, false)?;
        cursor.optional_trailing_comma();
        cursor.expect_char(')')?;
        let index = branches.len();
        merge_prefixed(&mut selector_labels, child.selector_labels, index);
        merge_prefixed(&mut predicate_labels, child.predicate_labels, index);
        if let Some(predicate) = predicate {
            if predicates.contains(&predicate) {
                return cursor.error(
                    "trox.duplicate-selector-key",
                    format!("duplicate select predicate `{predicate}`"),
                );
            }
            predicates.push(predicate);
            branches.push(SelectIdentityBranch::When {
                pattern: child.pattern,
            });
        } else {
            branches.push(SelectIdentityBranch::Otherwise {
                pattern: child.pattern,
            });
        }
        cursor.skip_space_comments();
        if cursor.consume_char(',') {
            continue;
        }
        cursor.expect_char(']')?;
        break;
    }
    cursor.optional_trailing_comma();
    cursor.expect_char(')')?;
    if branches.is_empty()
        || !matches!(
            branches.last(),
            Some(SelectIdentityBranch::Otherwise { .. })
        )
        || branches[..branches.len() - 1]
            .iter()
            .any(|branch| matches!(branch, SelectIdentityBranch::Otherwise { .. }))
    {
        return cursor.error(
            "trox.missing-otherwise",
            "select requires exactly one final otherwise branch",
        );
    }
    selector_labels.insert(vec![], selector_label);
    predicate_labels.insert(vec![], predicates);
    Ok(ParsedPattern {
        pattern: Pattern::Select { branches },
        meaning: None,
        selector_labels,
        predicate_labels,
    })
}

fn validate_placeholder_bindings(
    pattern: &Pattern,
    args: &BTreeMap<String, ArgumentSchema>,
    has_arguments: bool,
) -> Result<(), String> {
    let mut expected = BTreeSet::new();
    collect_placeholders(pattern, &mut expected)?;
    if !has_arguments && !expected.is_empty() {
        return Err("tx cannot bind visible placeholders; use txa".into());
    }
    let actual: BTreeSet<_> = args.keys().cloned().collect();
    if expected != actual {
        return Err(format!(
            "placeholder bindings differ: expected {expected:?}, got {actual:?}"
        ));
    }
    Ok(())
}

fn collect_placeholders(pattern: &Pattern, target: &mut BTreeSet<String>) -> Result<(), String> {
    match pattern {
        Pattern::Text { text } => target.extend(parse_placeholders(text)?),
        Pattern::Plural { branches } | Pattern::Ordinal { branches } => {
            for branch in branches {
                collect_placeholders(&branch.pattern, target)?;
            }
        }
        Pattern::Select { branches } => {
            for branch in branches {
                collect_placeholders(
                    match branch {
                        SelectIdentityBranch::When { pattern }
                        | SelectIdentityBranch::Otherwise { pattern } => pattern,
                    },
                    target,
                )?;
            }
        }
    }
    Ok(())
}

pub fn parse_placeholders(text: &str) -> Result<BTreeSet<String>, String> {
    if text.nfc().collect::<String>() != text {
        return Err("source literal must be NFC".into());
    }
    if text.contains('\r') {
        return Err("source literal must use LF line endings".into());
    }
    let bytes = text.as_bytes();
    let mut index = 0;
    let mut result = BTreeSet::new();
    while index < bytes.len() {
        if bytes[index] == b'{' && bytes.get(index + 1) == Some(&b'{')
            || bytes[index] == b'}' && bytes.get(index + 1) == Some(&b'}')
        {
            index += 2;
            continue;
        }
        if bytes[index] == b'{' {
            let end = text[index + 1..]
                .find('}')
                .ok_or_else(|| "unclosed `{`".to_owned())?;
            let name = &text[index + 1..index + 1 + end];
            if !valid_placeholder_name(name) {
                return Err(format!("invalid placeholder `{{{name}}}`"));
            }
            result.insert(name.to_owned());
            index += end + 2;
            continue;
        }
        if bytes[index] == b'}' {
            return Err("unmatched `}`".into());
        }
        index += 1;
    }
    Ok(result)
}

fn valid_placeholder_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value.is_ascii()
        && value.as_bytes()[0].is_ascii_lowercase()
        && value.split('_').all(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        })
}

fn validate_stable_id(value: &str, label: &str) -> Result<(), String> {
    let valid = !value.is_empty()
        && value.len() <= 96
        && value.is_ascii()
        && value.as_bytes()[0].is_ascii_lowercase()
        && value.split(['.', '-']).all(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        });
    if valid {
        Ok(())
    } else {
        Err(format!("invalid {label} `{value}`"))
    }
}

fn validate_pattern_limits(
    pattern: &Pattern,
    depth: usize,
    nodes: &mut usize,
) -> Result<(), String> {
    *nodes += 1;
    if *nodes > 4096 {
        return Err("pattern exceeds 4,096 nodes".into());
    }
    if depth > 16 {
        return Err("pattern exceeds 16 nested selectors".into());
    }
    match pattern {
        Pattern::Text { .. } => {}
        Pattern::Plural { branches } | Pattern::Ordinal { branches } => {
            if branches.len() > 256 {
                return Err("selector exceeds 256 branches".into());
            }
            for branch in branches {
                validate_pattern_limits(&branch.pattern, depth + 1, nodes)?;
            }
        }
        Pattern::Select { branches } => {
            if branches.len() > 256 {
                return Err("selector exceeds 256 branches".into());
            }
            for branch in branches {
                validate_pattern_limits(
                    match branch {
                        SelectIdentityBranch::When { pattern }
                        | SelectIdentityBranch::Otherwise { pattern } => pattern,
                    },
                    depth + 1,
                    nodes,
                )?;
            }
        }
    }
    Ok(())
}

fn reserve_pattern_node(
    cursor: &Cursor<'_>,
    selector_depth: usize,
    nodes: &mut usize,
    selector: bool,
) -> ParseResult<()> {
    if selector && selector_depth >= 16 {
        return cursor.error("trox.pattern-depth", "pattern exceeds 16 nested selectors");
    }
    if selector_depth > 16 {
        return cursor.error("trox.pattern-depth", "pattern exceeds 16 nested selectors");
    }
    *nodes += 1;
    if *nodes > 4096 {
        return cursor.error("trox.pattern-node-limit", "pattern exceeds 4,096 nodes");
    }
    Ok(())
}

fn validate_description(value: &str) -> Result<(), String> {
    if value.trim().is_empty() {
        return Err("description must not be empty".into());
    }
    if value.nfc().collect::<String>() != value || value.contains('\r') {
        return Err("description must be NFC and use LF line endings".into());
    }
    Ok(())
}

fn validate_numeric_order(branches: &[NumericBranch], kind: &str) -> Result<(), String> {
    let mut previous = None;
    let mut has_other = false;
    for branch in branches {
        let order = match &branch.key {
            NumericBranchKey::Exact { exact } => (0, *exact),
            NumericBranchKey::Plural { plural } if kind == "plural" => {
                has_other |= *plural == PluralCategory::Other;
                (1, category_index(*plural) as u64)
            }
            NumericBranchKey::Ordinal { ordinal } if kind == "ordinal" => {
                has_other |= *ordinal == PluralCategory::Other;
                (1, category_index(*ordinal) as u64)
            }
            _ => return Err("branch key kind does not match selector".into()),
        };
        if previous.is_some_and(|prior| prior >= order) {
            return Err(
                "numeric branches must be exact ascending, then categories in canonical order"
                    .into(),
            );
        }
        previous = Some(order);
    }
    if !has_other {
        return Err("numeric selector requires other".into());
    }
    Ok(())
}

fn parse_category(value: &str) -> Option<PluralCategory> {
    Some(match value {
        "zero" => PluralCategory::Zero,
        "one" => PluralCategory::One,
        "two" => PluralCategory::Two,
        "few" => PluralCategory::Few,
        "many" => PluralCategory::Many,
        "other" => PluralCategory::Other,
        _ => return None,
    })
}
fn category_index(value: PluralCategory) -> usize {
    [
        PluralCategory::Zero,
        PluralCategory::One,
        PluralCategory::Two,
        PluralCategory::Few,
        PluralCategory::Many,
        PluralCategory::Other,
    ]
    .iter()
    .position(|item| *item == value)
    .unwrap()
}

fn merge_prefixed<T>(
    target: &mut BTreeMap<Vec<usize>, T>,
    source: BTreeMap<Vec<usize>, T>,
    prefix: usize,
) {
    for (mut path, value) in source {
        path.insert(0, prefix);
        target.insert(path, value);
    }
}

fn normalize_expression(value: &str, language: Language) -> String {
    let bytes = value.as_bytes();
    let mut output = String::new();
    let mut index = 0;
    let mut pending_space = false;
    let mut regex_allowed = true;
    while index < bytes.len() {
        if let Some(skipped) = skip_token(value, index, language, regex_allowed) {
            if skipped.kind == SkippedKind::Comment {
                pending_space = !output.is_empty();
            } else {
                if pending_space && !output.is_empty() {
                    output.push(' ');
                }
                output.push_str(&value[index..skipped.end]);
                pending_space = false;
                regex_allowed = false;
            }
            index = skipped.end;
            continue;
        }
        if bytes[index].is_ascii_whitespace() {
            pending_space = !output.is_empty();
            index += 1;
            continue;
        }
        if pending_space && !output.is_empty() {
            output.push(' ');
        }
        pending_space = false;
        if is_ident_start(bytes[index]) {
            let end = identifier_end(bytes, index);
            output.push_str(&value[index..end]);
            regex_allowed = identifier_allows_regex(&value[index..end]);
            index = end;
        } else {
            let start = index;
            (index, regex_allowed) =
                punctuation_end_and_regex_allowed(value, index, language, regex_allowed);
            output.push_str(&value[start..index]);
        }
    }
    output
}
fn line_starts(source: &str) -> Vec<usize> {
    let mut starts = Vec::with_capacity(source.len() / 80 + 1);
    starts.push(0);
    starts.extend(source.match_indices('\n').map(|(index, _)| index + 1));
    starts
}

#[cfg(test)]
mod tests;
