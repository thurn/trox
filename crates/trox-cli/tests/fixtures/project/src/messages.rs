fn messages(card_count: u32, subtype: TermId) {
    tx("Close deck browser", "Accessible button label that closes the deck browser.");
    txa(
        plural(card_count, [
            exact(0, "No cards remain."),
            one("{card_count} card remains."),
            other("{card_count} cards remain."),
        ]),
        tx_args![card_count],
        "Status text showing how many cards remain in the deck.",
    );
    txa(
        "Create {card_count} {creature_noun}.",
        tx_args![card_count, creature_noun => counted(subtype, card_count)],
        "Creation rule whose runtime noun agrees with the count.",
    );
}
