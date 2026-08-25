// Restated from data/locales/en-US/battle.ftl, sites.ftl, cards.ftl, and
// accessibility.ftl in quest_prototype.

const subtypeTerms = {
  character: termId("card-subtype.character"),
  event: termId("card-subtype.event"),
  warrior: termId("card-subtype.warrior"),
};

export function battleZoneTitle(zone: "deck" | "void" | "banished") {
  return tx(select(zone, [
    when("deck", "Your Deck"),
    when("void", "Your Void"),
    when("banished", "Banished Cards"),
    otherwise("Your Cards"),
  ]), "Battle zone browser heading for the viewer's selected zone.");
}

export function battleZoneCount(count: number) {
  return txa(plural(count, [
    exact(0, "No Cards."),
    one("{count} Card"),
    other("{count} Cards"),
  ]), { count }, "Total cards in the currently open battle zone.");
}

export function battleVictorySummary(opponent_name: string, player_score: number, opponent_score: number, turn_count: number) {
  return txa(plural(turn_count, [
    one("Defeated {opponent_name} · {player_score}–{opponent_score} · {turn_count} turn"),
    other("Defeated {opponent_name} · {player_score}–{opponent_score} · {turn_count} turns"),
  ]), { opponent_name, player_score, opponent_score, turn_count }, "Victory summary with opponent, score, and turn count.");
}

export function turnAnnouncement(owner: "viewer" | "opponent") {
  return tx(select(owner, [
    when("viewer", "Your Turn"),
    when("opponent", "Opponent Turn"),
    otherwise("Player Turn"),
  ]), "Battle turn announcement selected by semantic owner.");
}

export function cardPickerCaption(highlighted: boolean, owner: "viewer" | "opponent", zone: "deck" | "hand" | "void") {
  return tx(select(highlighted, [
    when(true, select(owner, [
      when("viewer", select(zone, [when("deck", "Your deck"), when("hand", "Your hand"), when("void", "Your void"), otherwise("Your cards")])),
      when("opponent", select(zone, [when("deck", "Opponent deck"), when("hand", "Opponent hand"), when("void", "Opponent void"), otherwise("Opponent cards")])),
      otherwise("Player cards"),
    ])),
    otherwise("Choose a highlighted legal target."),
  ]), "Accessible card-picker caption combining highlight, owner, and zone.");
}

export function battleResult(outcome: "victory" | "defeat" | "draw") {
  return tx(select(outcome, [
    when("victory", "Victory"), when("defeat", "Defeat"), when("draw", "Draw"), otherwise("Battle Complete"),
  ]), "Battle result title selected by the semantic outcome.");
}

export function explorationModifier(modifier: "opening-hand" | "starting-energy", amount: number) {
  return txa(select(modifier, [
    when("opening-hand", plural(amount, [one("+{amount} card in your opening hand"), other("+{amount} cards in your opening hand")])),
    when("starting-energy", "+{amount} starting Energy"),
    otherwise("+{amount} for the next battle"),
  ]), { amount }, "Announcement for a semantic next-battle modifier.");
}

export function subtypeCreation(count: number, subtype: keyof typeof subtypeTerms) {
  return txa("Create {count} {subtype_noun}.", {
    count,
    subtype_noun: counted(subtypeTerms[subtype], count),
  }, "Rules text creating a runtime-selected card subtype that agrees with count.");
}

export function subtypeChange(card_name: LocalizedString, subtype: keyof typeof subtypeTerms) {
  return txa("Change {card_name} to become {subtype_noun}.", {
    card_name: opaque(card_name),
    subtype_noun: indefinite(subtypeTerms[subtype]),
  }, "Card transformation; card name is atomic and subtype is indefinite.");
}

export function ordinalPack(pack_number: number) {
  return txa(ordinal(pack_number, [
    one("{pack_number}st pack"), two("{pack_number}nd pack"), few("{pack_number}rd pack"), other("{pack_number}th pack"),
  ]), { pack_number }, "Ordinal position of a draft pack.");
}
