export function deckName(deck_name: string) {
  return txa(
    "Deck: {deck_name}",
    { deck_name },
    "Label followed by the user-defined deck name.",
  );
}

export function ownerLabel(owner: "player" | "opponent") {
  return tx(
    select(owner, [
      when("player", "Your cards"),
      when("opponent", "Opponent cards"),
      otherwise("Player cards"),
    ]),
    "Heading for cards grouped by semantic owner.",
  );
}
