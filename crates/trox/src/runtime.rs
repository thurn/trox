use crate::bundle::{NumberFormat, PluralRules};
use crate::{DeserializeError, PluralCategory};

#[derive(Debug)]
pub(crate) struct CompiledPluralRules {
    cardinal: Vec<(PluralCategory, Condition)>,
    ordinal: Vec<(PluralCategory, Condition)>,
}

impl CompiledPluralRules {
    pub(crate) fn compile(rules: &PluralRules) -> Result<Self, DeserializeError> {
        Ok(Self {
            cardinal: compile_group("cardinal", &rules.cardinal)?,
            ordinal: compile_group("ordinal", &rules.ordinal)?,
        })
    }

    pub(crate) fn category(&self, ordinal: bool, value: u64) -> PluralCategory {
        let rules = if ordinal {
            &self.ordinal
        } else {
            &self.cardinal
        };
        rules
            .iter()
            .find_map(|(category, condition)| condition.matches(value).then_some(*category))
            .unwrap_or(PluralCategory::Other)
    }
}

#[derive(Debug)]
struct Condition(Vec<Vec<Relation>>);

impl Condition {
    fn matches(&self, value: u64) -> bool {
        self.0
            .iter()
            .any(|clause| clause.iter().all(|relation| relation.matches(value)))
    }
}

#[derive(Debug)]
struct Relation {
    operand: Operand,
    modulus: Option<u64>,
    negate: bool,
    ranges: Vec<(u64, u64)>,
}

impl Relation {
    fn matches(&self, value: u64) -> bool {
        let mut operand = match self.operand {
            Operand::Number | Operand::Integer => value,
            Operand::Fractional => 0,
        };
        if let Some(modulus) = self.modulus {
            operand %= modulus;
        }
        let contained = self
            .ranges
            .iter()
            .any(|(start, end)| (*start..=*end).contains(&operand));
        contained != self.negate
    }
}

#[derive(Debug, Clone, Copy)]
enum Operand {
    Number,
    Integer,
    Fractional,
}

fn compile_group(
    label: &str,
    rules: &std::collections::BTreeMap<PluralCategory, String>,
) -> Result<Vec<(PluralCategory, Condition)>, DeserializeError> {
    let mut compiled = Vec::new();
    for category in [
        PluralCategory::Zero,
        PluralCategory::One,
        PluralCategory::Two,
        PluralCategory::Few,
        PluralCategory::Many,
    ] {
        if let Some(rule) = rules.get(&category) {
            compiled.push((
                category,
                compile_condition(rule).ok_or_else(|| {
                    DeserializeError::InvalidBundle(format!(
                        "invalid {label} plural rule for `{}`",
                        category.as_str()
                    ))
                })?,
            ));
        }
    }
    Ok(compiled)
}

pub(crate) fn validate_plural_rules(rules: &PluralRules) -> Result<(), DeserializeError> {
    for (label, group) in [("cardinal", &rules.cardinal), ("ordinal", &rules.ordinal)] {
        if group.get(&PluralCategory::Other).map(String::as_str) != Some("") {
            return Err(DeserializeError::InvalidBundle(format!(
                "{label} plural rules require an empty `other` condition"
            )));
        }
        for (category, rule) in group {
            if *category != PluralCategory::Other && compile_condition(rule).is_none() {
                return Err(DeserializeError::InvalidBundle(format!(
                    "invalid {label} plural rule for `{}`",
                    category.as_str()
                )));
            }
        }
    }
    Ok(())
}

fn compile_condition(rule: &str) -> Option<Condition> {
    let condition = rule.split('@').next()?.trim();
    if condition.is_empty() {
        return None;
    }
    let clauses = condition
        .split(" or ")
        .map(|clause| {
            if clause.is_empty() {
                return None;
            }
            clause
                .split(" and ")
                .map(|relation| compile_relation(relation.trim()))
                .collect::<Option<Vec<_>>>()
        })
        .collect::<Option<Vec<_>>>()?;
    Some(Condition(clauses))
}

fn compile_relation(relation: &str) -> Option<Relation> {
    let normalized = relation
        .replace(" is not ", " != ")
        .replace(" is ", " = ")
        .replace(" not in ", " != ")
        .replace(" not within ", " != ")
        .replace(" in ", " = ")
        .replace(" within ", " = ")
        .replace(" mod ", " % ");
    let (left, negate, right) = if let Some((left, right)) = normalized.split_once(" != ") {
        (left, true, right)
    } else if let Some((left, right)) = normalized.split_once(" = ") {
        (left, false, right)
    } else {
        return None;
    };
    let left: Vec<_> = left.split_whitespace().collect();
    let operand = match left.first().copied()? {
        "n" => Operand::Number,
        "i" => Operand::Integer,
        "v" | "w" | "f" | "t" | "c" | "e" => Operand::Fractional,
        _ => return None,
    };
    let modulus = match left.as_slice() {
        [_] => None,
        [_, "%", modulus] => modulus.parse::<u64>().ok().filter(|value| *value > 0),
        _ => return None,
    };
    if left.len() == 3 && modulus.is_none() {
        return None;
    }
    let ranges = right
        .split(',')
        .map(|part| {
            let part = part.trim();
            if let Some((start, end)) = part.split_once("..") {
                let start = start.parse::<u64>().ok()?;
                let end = end.parse::<u64>().ok()?;
                (start <= end).then_some((start, end))
            } else {
                let value = part.parse::<u64>().ok()?;
                Some((value, value))
            }
        })
        .collect::<Option<Vec<_>>>()?;
    (!ranges.is_empty()).then_some(Relation {
        operand,
        modulus,
        negate,
        ranges,
    })
}

pub(crate) fn format_number(value: f64, format: &NumberFormat) -> String {
    let normalized = if value == 0.0 { 0.0 } else { value };
    let ascii = serde_json_canonicalizer::to_string(&normalized).unwrap_or_else(|_| "0".into());
    let (mantissa, exponent) = ascii
        .split_once(['e', 'E'])
        .map_or((ascii.as_str(), None), |(left, right)| (left, Some(right)));
    let negative = mantissa.starts_with('-');
    let unsigned = mantissa.trim_start_matches('-');
    let (integer, fraction) = unsigned
        .split_once('.')
        .map_or((unsigned, None), |(left, right)| (left, Some(right)));
    let grouped = if exponent.is_none() {
        group_digits(integer, format)
    } else {
        integer.to_owned()
    };
    let digits: Vec<_> = format.digits.chars().collect();
    let mut output = String::new();
    if negative {
        output.push_str(&format.minus);
    }
    map_digits_into(&grouped, &digits, &mut output);
    if let Some(fraction) = fraction {
        output.push_str(&format.decimal);
        map_digits_into(fraction, &digits, &mut output);
    }
    if let Some(exponent) = exponent {
        output.push_str(&format.exponent);
        let exponent = if let Some(rest) = exponent.strip_prefix('+') {
            output.push_str(&format.plus);
            rest
        } else if let Some(rest) = exponent.strip_prefix('-') {
            output.push_str(&format.minus);
            rest
        } else {
            exponent
        };
        map_digits_into(exponent, &digits, &mut output);
    }
    output
}

fn group_digits(integer: &str, format: &NumberFormat) -> String {
    let primary = format.grouping[0];
    let secondary = format.grouping[1];
    let minimum_length = primary.saturating_add(format.minimum_grouping_digits);
    if primary == 0 || integer.len() < minimum_length {
        return integer.into();
    }
    let mut groups = Vec::new();
    let mut end = integer.len();
    let mut width = primary;
    while end > width {
        groups.push(&integer[end - width..end]);
        end -= width;
        width = secondary.max(1);
    }
    groups.push(&integer[..end]);
    groups.reverse();
    groups.join(&format.group)
}

fn map_digits_into(input: &str, digits: &[char], output: &mut String) {
    output.extend(
        input
            .chars()
            .map(|ch| ch.to_digit(10).map_or(ch, |index| digits[index as usize])),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evaluates_russian_integer_rules() {
        let rule = compile_condition("v = 0 and i % 10 = 1 and i % 100 != 11").unwrap();
        assert!(rule.matches(21));
        assert!(!rule.matches(11));
        let rule = compile_condition("v = 0 and i % 10 = 2..4 and i % 100 != 12..14").unwrap();
        assert!(rule.matches(23));
    }

    #[test]
    fn minimum_grouping_digits_uses_the_cldr_threshold() {
        let minimum_two = NumberFormat {
            minimum_grouping_digits: 2,
            ..NumberFormat::default()
        };
        assert_eq!(format_number(9_999.0, &minimum_two), "9999");
        assert_eq!(format_number(10_000.0, &minimum_two), "10,000");
        assert_eq!(format_number(1_234_567.0, &minimum_two), "1,234,567");
    }
}
