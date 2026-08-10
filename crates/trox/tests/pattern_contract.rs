use std::collections::BTreeMap;

use trox::prelude::*;
use trox::{Argument, TermArgument};

#[test]
#[should_panic(expected = "term number exceeds 2^53 - 1")]
fn numbered_term_rejects_values_above_the_safe_integer_limit() {
    let term = TermArgument {
        term_id: TermId::new("card-subtype"),
        form: Some("counted".to_owned()),
        number: Some(9_007_199_254_740_992),
    };
    let arguments = BTreeMap::from([("term".to_owned(), Argument::from_borrowed(&term))]);

    let _ = txa("{term}", arguments, "A counted term.");
}

#[test]
#[should_panic(expected = "meaning may only wrap a complete top-level pattern")]
fn plural_arm_rejects_nested_meaning() {
    let _ = plural(
        1_u32,
        [one(meaning("singular-card", "card")), other("cards")],
    );
}

#[test]
#[should_panic(expected = "meaning may only wrap a complete top-level pattern")]
fn select_arm_rejects_nested_meaning() {
    let _ = select(
        true,
        [
            when(true, meaning("enabled-state", "Enabled")),
            otherwise("Disabled"),
        ],
    );
}

#[test]
#[should_panic(expected = "duplicate term form")]
fn term_builder_rejects_a_second_form() {
    let _ = term(TermId::new("card-subtype"))
        .form("indefinite")
        .form("counted");
}

#[test]
fn owned_text_remains_available_through_the_adapter_boundary() {
    let value = trox::tx_owned("Loaded at runtime".to_owned(), None).unwrap();

    assert!(value.is_atomic());
    assert!(
        value
            .to_canonical_json()
            .unwrap()
            .contains("Loaded at runtime")
    );
}
