//! Parsing and classification for inline Rust and TypeScript argument maps.

use std::collections::BTreeMap;

use crate::config::Language;

use super::lexer::{decode_literal_prefix, split_top_level, split_top_level_once, top_level_items};
use super::{
    ArgumentSchema, Cursor, ParseError, ParseResult, valid_placeholder_name, validate_stable_id,
};

pub(super) struct ParsedArguments {
    pub(super) schemas: BTreeMap<String, ArgumentSchema>,
    pub(super) term_ids: BTreeMap<String, Option<String>>,
}

struct ClassifiedArgument {
    schema: ArgumentSchema,
    static_term_id: Option<String>,
}

pub(super) fn parse_argument_map(cursor: &mut Cursor<'_>) -> ParseResult<ParsedArguments> {
    cursor.skip_space_comments();
    let (open, close) = match cursor.language {
        Language::Rust => {
            let name = cursor.parse_ident()?;
            if name != "tx_args" {
                return cursor.error(
                    "trox.argument-map",
                    "Rust txa requires inline tx_args![...]",
                );
            }
            cursor.skip_space_comments();
            cursor.expect_char('!')?;
            ('[', ']')
        }
        Language::TypeScript => ('{', '}'),
        Language::Ron => unreachable!(),
    };
    cursor.expect_char(open)?;
    let content_start = cursor.index;
    let content = cursor.take_balanced_content(close)?;
    let mut arguments = BTreeMap::new();
    let mut term_ids = BTreeMap::new();
    for raw_item in top_level_items(&content, ',', cursor.language) {
        let item_start = raw_item.as_ptr() as usize - content.as_ptr() as usize;
        let item_leading = raw_item.len() - raw_item.trim_start().len();
        let item = raw_item.trim();
        if item.is_empty() {
            continue;
        }
        if item.starts_with("...") {
            return cursor.error(
                "trox.argument-spread",
                "argument spreads are not extractable",
            );
        }
        let (name, expression) = if cursor.language == Language::Rust {
            split_top_level_once(item, "=>", cursor.language)
                .map_or((item.trim(), item.trim()), |(left, right)| {
                    (left.trim(), right.trim())
                })
        } else {
            split_top_level_once(item, ":", cursor.language)
                .map_or((item.trim(), item.trim()), |(left, right)| {
                    (left.trim(), right.trim())
                })
        };
        if !valid_placeholder_name(name) {
            return cursor.error(
                "trox.invalid-placeholder",
                format!("invalid argument name `{name}`"),
            );
        }
        let expression_start = expression.as_ptr() as usize - item.as_ptr() as usize;
        let classified =
            classify_argument_value(expression, cursor.language).map_err(|mut error| {
                error.offset += content_start + item_start + item_leading + expression_start;
                error
            })?;
        if matches!(classified.schema, ArgumentSchema::Term { .. }) {
            term_ids.insert(name.to_owned(), classified.static_term_id);
        }
        if arguments
            .insert(name.to_owned(), classified.schema)
            .is_some()
        {
            return cursor.error(
                "trox.duplicate-argument",
                format!("duplicate argument `{name}`"),
            );
        }
        if arguments.len() > 256 {
            return cursor.error(
                "trox.argument-limit",
                "message exceeds 256 visible arguments",
            );
        }
    }
    Ok(ParsedArguments {
        schemas: arguments,
        term_ids,
    })
}

#[cfg(test)]
pub(super) fn classify_argument(
    expression: &str,
    language: Language,
) -> ParseResult<ArgumentSchema> {
    classify_argument_value(expression, language).map(|classified| classified.schema)
}

fn classify_argument_value(
    expression: &str,
    language: Language,
) -> ParseResult<ClassifiedArgument> {
    let compact = expression.trim();
    let mut parser = Cursor::new(compact, 0, language);
    let Ok(helper) = parser.parse_ident() else {
        return Ok(ClassifiedArgument {
            schema: ArgumentSchema::Scalar,
            static_term_id: None,
        });
    };
    if !matches!(
        helper.as_str(),
        "opaque" | "indefinite" | "counted" | "term"
    ) {
        return Ok(ClassifiedArgument {
            schema: ArgumentSchema::Scalar,
            static_term_id: None,
        });
    }
    parser.expect_char('(')?;
    let arguments = parser.take_balanced_content(')')?;
    let parts: Vec<_> = split_top_level(&arguments, ',', language)
        .into_iter()
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect();

    if helper == "opaque" {
        expect_helper_arity(&parser, &helper, &parts, 1)?;
        parser.expect_end()?;
        return Ok(ClassifiedArgument {
            schema: ArgumentSchema::Opaque,
            static_term_id: None,
        });
    }

    let expected = if helper == "counted" { 2 } else { 1 };
    expect_helper_arity(&parser, &helper, &parts, expected)?;
    let static_term_id = parse_static_term_id(parts[0], language)?;
    if let Some(term_id) = &static_term_id {
        validate_stable_id(term_id, "term ID").map_err(|message| ParseError {
            offset: 0,
            rule: "trox.invalid-term-id",
            message,
        })?;
    }
    if helper == "indefinite" || helper == "counted" {
        parser.expect_end()?;
        return Ok(ClassifiedArgument {
            schema: ArgumentSchema::Term {
                form: Some(helper),
                number: expected == 2,
            },
            static_term_id,
        });
    }

    let mut form = None;
    let mut number = false;
    let mut finished = false;
    while !parser.is_at_end() {
        parser.expect_char('.')?;
        let method = parser.parse_ident()?;
        parser.expect_char('(')?;
        let content = parser.take_balanced_content(')')?;
        let method_parts: Vec<_> = split_top_level(&content, ',', language)
            .into_iter()
            .map(str::trim)
            .filter(|part| !part.is_empty())
            .collect();
        match method.as_str() {
            "form" if form.is_none() && !number && !finished => {
                expect_helper_arity(&parser, "term(...).form", &method_parts, 1)?;
                form = Some(
                    parse_literal_expression(method_parts[0], language).ok_or_else(|| {
                        ParseError {
                            offset: parser.index,
                            rule: "trox.literal-required",
                            message: "term form must be one literal string token".into(),
                        }
                    })?,
                );
                validate_stable_id(form.as_deref().unwrap(), "term form ID").map_err(
                    |message| ParseError {
                        offset: parser.index,
                        rule: "trox.invalid-term-form",
                        message,
                    },
                )?;
            }
            "number" if !number && !finished => {
                expect_helper_arity(&parser, "term(...).number", &method_parts, 1)?;
                number = true;
            }
            "finish" if !finished => {
                expect_helper_arity(&parser, "term(...).finish", &method_parts, 0)?;
                finished = true;
            }
            _ => {
                return parser.error(
                    "trox.argument-map",
                    format!("unsupported or repeated term builder method `{method}`"),
                );
            }
        }
        if finished && !parser.is_at_end() {
            return parser.error(
                "trox.argument-map",
                "term finish() must be the final method",
            );
        }
    }
    Ok(ClassifiedArgument {
        schema: ArgumentSchema::Term { form, number },
        static_term_id,
    })
}

fn expect_helper_arity(
    cursor: &Cursor<'_>,
    helper: &str,
    parts: &[&str],
    expected: usize,
) -> ParseResult<()> {
    if parts.len() == expected {
        Ok(())
    } else {
        cursor.error(
            "trox.argument-map",
            format!("{helper} requires exactly {expected} argument(s)"),
        )
    }
}

fn parse_literal_expression(expression: &str, language: Language) -> Option<String> {
    let expression = expression.trim();
    let (value, consumed) = decode_literal_prefix(expression, language)?;
    (expression[consumed..].trim().is_empty()).then_some(value)
}

fn parse_static_term_id(expression: &str, language: Language) -> ParseResult<Option<String>> {
    let expression = expression.trim();
    let mut parser = Cursor::new(expression, 0, language);
    let Ok(identifier) = parser.parse_ident() else {
        return Ok(None);
    };
    match language {
        Language::Rust if identifier == "TermId" => {
            parser.expect_char(':')?;
            parser.expect_char(':')?;
            if parser.parse_ident()? != "new" {
                return Ok(None);
            }
        }
        Language::TypeScript if identifier == "termId" => {}
        _ => return Ok(None),
    }
    parser.expect_char('(')?;
    let content = parser.take_balanced_content(')')?;
    parser.expect_end()?;
    let parts: Vec<_> = split_top_level(&content, ',', language)
        .into_iter()
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect();
    expect_helper_arity(&parser, "term ID constructor", &parts, 1)?;
    Ok(parse_literal_expression(parts[0], language))
}
