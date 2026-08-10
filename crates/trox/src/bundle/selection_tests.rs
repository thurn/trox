use super::*;

#[test]
fn selector_lookup_handles_the_pattern_node_boundary_without_an_index() {
    let selectors: Vec<_> = (0..16)
        .flat_map(|outer| {
            (0..256).map(move |inner| SelectorRecord::Plural {
                path: vec![outer, inner],
                value: 0,
            })
        })
        .collect();

    assert_eq!(
        selector_at_path(&selectors, &[0, 0]).unwrap().path(),
        [0, 0]
    );
    assert_eq!(
        selector_at_path(&selectors, &[8, 128]).unwrap().path(),
        [8, 128]
    );
    assert_eq!(
        selector_at_path(&selectors, &[15, 255]).unwrap().path(),
        [15, 255]
    );
    assert!(selector_at_path(&selectors, &[16, 0]).is_none());
}
