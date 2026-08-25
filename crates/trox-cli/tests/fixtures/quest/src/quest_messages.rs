// Rust-side equivalence cases based on the same Quest player surfaces.
fn static_messages() {
    tx("Dreamwell History", "Heading for the shared-draw history panel.");
    tx("No Dreamwell cards drawn yet.", "Empty state for shared-draw history.");
    tx("Choose a Target", "Battle tutorial target-selection heading.");
    tx("Select a highlighted legal target.", "Battle tutorial target-selection instruction.");
    tx("No eligible cards are available.", "Exploration picker empty state.");
}

fn count_messages(selected_count: u32, required_count: u32) {
    txa(
        "{selected_count}/{required_count} selected",
        tx_args![selected_count, required_count],
        "Battle card-picker selected and required counts.",
    );
    txa(
        plural(selected_count, [one("{selected_count} copy"), other("{selected_count} copies")]),
        tx_args![selected_count],
        "Number of copies in an Augury card choice.",
    );
}
