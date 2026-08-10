import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import { blake3 } from "@noble/hashes/blake3.js";
import {
  bundleFromCanonicalJSON, canonicalJson, counted, exact, Localizer, meaning, one, ordinal, other, plural, select, SourceCatalog, termId, tx, txa, when, otherwise,
  type Bundle,
} from "./index.js";

describe("authoring", () => {
  it("constructs deterministic identities", () => {
    const first = tx("Close deck browser", "Accessible close button label.");
    const second = tx("Close deck browser", "A different description.");
    expect(first.entryId).toBe(second.entryId);
    expect(first.entryId).toBe("tx1_5ij3qqw6xjvvxamd3nyt23flsm");
  });

  it("builds nested selector paths", () => {
    const owner: "player" | "opponent" = "player";
    const count = 3;
    const value = txa(select(owner, [
      when("player", plural(count, [one("You have {count} card."), other("You have {count} cards.")])),
      when("opponent", plural(count, [one("Your opponent has {count} card."), other("Your opponent has {count} cards.")])),
      otherwise(plural(count, [one("That player has {count} card."), other("That player has {count} cards.")])),
    ]), { count }, "Battle status showing cards owned by one participant.");
    expect(value.selectors.map((record) => record.path)).toEqual([[], [0], [1], [2]]);
    const fixture = readFileSync(new URL("../../../conformance/localized-nested-owner-count.json", import.meta.url), "utf8").trimEnd();
    expect(value.toCanonicalJSON()).toBe(fixture);
    const source = testBundle("en-US", { [value.entryId]: { arguments: { count: { kind: "scalar" } }, identity: value.identity, rows: {}, source_signature: value.sourceSignature } });
    const corrupted = JSON.parse(fixture) as { selectors: unknown[] }; corrupted.selectors.reverse();
    expect(() => new SourceCatalog(source).localizedStringFromJSON(canonicalJson(corrupted))).toThrow(/canonical pattern order/);
  });

  it("matches the shared exact-zero wire fixture byte for byte", () => {
    const card_count = 3;
    const value = txa(plural(card_count, [
      exact(0, "No cards remain."),
      one("{card_count} card remains."), other("{card_count} cards remain."),
    ]), { card_count }, "Status.");
    const fixture = readFileSync(new URL("../../../conformance/localized-card-count.json", import.meta.url), "utf8").trimEnd();
    expect(value.toCanonicalJSON()).toBe(fixture);
  });

  it("meaning disambiguates identical text", () => {
    expect(tx(meaning("open-action", "Open"), "Action.").entryId)
      .not.toBe(tx(meaning("open-state", "Open"), "State.").entryId);
  });

  it("rejects implicit string conversion", () => {
    const value = tx("Cards", "Collection heading.");
    expect(() => `${value}`).toThrow(/must be resolved/);
  });

  it("supports ordinals", () => {
    const value = txa(ordinal(2, [one("{turn}st"), other("{turn}th")]), { turn: 2 }, "Turn ordinal.");
    expect(value.selectors[0]?.kind).toBe("ordinal");
  });

  it("uses RFC 8785 number spelling", () => {
    expect(txa("{n}", { n: 1e20 }, "Number.").toCanonicalJSON())
      .toContain('"value":100000000000000000000');
    expect(txa("{n}", { n: 1e-7 }, "Number.").toCanonicalJSON())
      .toContain('"value":1e-7');
  });

  it("bundle parsing rejects noncanonical and unknown fields", () => {
    const bundle = {
      cldr_version: "48", direction: "ltr", entries: {}, fallback_chain: [], fallbacks_flattened: true,
      format: "trox-bundle", isolation: "isolate", locale: "en-US", message_facets: [],
      number_format: { decimal: ".", digits: "0123456789", exponent: "E", group: ",", grouping: [3, 3], minimum_grouping_digits: 1, minus: "-", plus: "+" },
      plural_rules: { cardinal: { one: "i = 1 and v = 0", other: "" }, ordinal: { other: "" } },
      source_catalog_fingerprint: "0".repeat(64), source_locale: "en-US", terms: {}, version: { major: 1, minor: 0 },
    };
    expect(bundleFromCanonicalJSON(canonicalJson(bundle)).locale).toBe("en-US");
    expect(() => bundleFromCanonicalJSON(JSON.stringify(bundle, null, 2))).toThrow(/canonical/);
    expect(() => bundleFromCanonicalJSON(canonicalJson({ ...bundle, surprise: true }))).toThrow(/unknown field/);
  });

  it("resolves checked target rows, isolates substitutions, and authorizes wire values", () => {
    const name = "Ada";
    const value = txa("Hello {name}", { name }, "Greeting shown to another player.");
    const expansion = { entry_signature: value.sourceSignature, path: [] };
    const rowId = `row1_${base32ForTest(blake3(new TextEncoder().encode(canonicalJson(expansion))).slice(0, 16))}`;
    const source = testBundle("en-US", {
      [value.entryId]: { arguments: { name: { kind: "scalar" } }, identity: value.identity, rows: {}, source_signature: value.sourceSignature },
    });
    const target = testBundle("es", {
      [value.entryId]: {
        rows: { [rowId]: { expansion, origin_locale: "es", translation: "Hola {name}" } },
        source_signature: value.sourceSignature,
      },
    });
    const localizer = new Localizer(target, source, { strict: true });
    expect(localizer.resolveChecked(value)).toBe("Hola \u2068Ada\u2069");
    expect(localizer.localizedStringFromJSON(value.toCanonicalJSON()).toCanonicalJSON()).toBe(value.toCanonicalJSON());
    expect(() => localizer.localizedStringFromJSON(value.toCanonicalJSON().replace(value.entryId, "tx1_aaaaaaaaaaaaaaaaaaaaaaaaaa"))).toThrow(/hash mismatch/);
    const badSignature = JSON.parse(value.toCanonicalJSON()) as Record<string, unknown>; badSignature.source_signature = "0".repeat(64);
    expect(() => localizer.localizedStringFromJSON(canonicalJson(badSignature))).toThrow(/hash mismatch/);
    const corruptTarget = structuredClone(target) as Bundle; const mutable = corruptTarget.entries[value.entryId]!.rows[rowId]!.expansion.path as unknown[]; mutable.push({ branch: 0, kind: "select" });
    expect(() => new Localizer(corruptTarget, source)).toThrow(/row hash mismatch/);
  });

  it("visibly recovers unknown terms and emits structured diagnostics", () => {
    const noun = counted(termId("card-subtype.unknown"), 2);
    const value = txa("Create {noun}.", { noun }, "Runtime-selected noun.");
    const source = testBundle("en-US", {
      [value.entryId]: { arguments: { noun: { form: "counted", kind: "term", number: true } }, identity: value.identity, rows: {}, source_signature: value.sourceSignature },
    });
    const target = testBundle("es", {}); const diagnostics: string[] = [];
    const localizer = new Localizer(target, source, { diagnostic: (diagnostic) => diagnostics.push(diagnostic.code) });
    expect(localizer.resolve(value)).toBe("Create \u2068⟦term:card-subtype.unknown⟧\u2069.");
    expect(diagnostics).toEqual(["trox.missing-message", "trox.unknown-term"]);
  });

  it("rejects incompatible source and target bundle shapes", () => {
    const value = tx("Authorized", "Static label.");
    const source = testBundle("en-US", { [value.entryId]: { arguments: {}, identity: value.identity, rows: {}, source_signature: value.sourceSignature } });
    expect(() => new Localizer({ ...source, cldr_version: "49" }, source)).toThrow(/CLDR/);
    const target = testBundle("es", { [value.entryId]: { identity: value.identity, rows: {}, source_signature: value.sourceSignature } });
    expect(() => new Localizer(target, source)).toThrow(/must not contain arguments or identity/);
    expect(() => bundleFromCanonicalJSON(canonicalJson({ ...source, fallback_chain: ["en-US"] }))).toThrow(/fallback_chain/);
  });
});

function testBundle(locale: string, entries: Bundle["entries"]): Bundle {
  return {
    cldr_version: "48", direction: "ltr", entries,
    fallback_chain: locale === "en-US" ? [] : ["en-US"], fallbacks_flattened: true,
    format: "trox-bundle", isolation: "isolate", locale, message_facets: [],
    number_format: { decimal: ".", digits: "0123456789", exponent: "E", group: ",", grouping: [3, 3], minimum_grouping_digits: 1, minus: "-", plus: "+" },
    plural_rules: { cardinal: { one: "i = 1 and v = 0", other: "" }, ordinal: { other: "" } },
    source_catalog_fingerprint: "0".repeat(64), source_locale: "en-US", terms: {}, version: { major: 1, minor: 0 },
  };
}

function base32ForTest(bytes: Uint8Array): string {
  const alphabet = "abcdefghijklmnopqrstuvwxyz234567"; let bits = 0; let accumulator = 0; let output = "";
  for (const byte of bytes) { accumulator = (accumulator << 8) | byte; bits += 8; while (bits >= 5) { bits -= 5; output += alphabet[(accumulator >>> bits) & 31]; } }
  if (bits > 0) output += alphabet[(accumulator << (5 - bits)) & 31]; return output;
}
