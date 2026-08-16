import { blake3 } from "@noble/hashes/blake3.js";
import { describe, expect, it } from "vitest";

import {
  Localizer,
  canonicalJson,
  counted,
  opaque,
  termId,
  tx,
  txa,
  type ArgumentSchema,
  type Bundle,
  type BundleEntry,
  type LocalizedString,
  type ResolvedLocalizedPart,
} from "./index.js";

describe("structured placeholder resolution", () => {
  it("preserves source literals, escaped braces, annotations, and unannotated placeholders", () => {
    const value = txa(
      "Gain {{shown}} {card} for {count} turns ({plain}).",
      { card: "Radiant Echo", count: 2, plain: true },
      "Effect granting a displayed card.",
    );
    const annotation = { kind: "entity", entityId: "card-7" } as const;
    const annotated = value.annotate({ card: annotation });
    const diagnostics: string[] = [];
    const localizer = new Localizer(bundle("es"), sourceBundle([value]), {
      diagnostic: (diagnostic) => diagnostics.push(diagnostic.code),
    });

    const outcome = localizer.resolvePartsOutcome(annotated);
    expect(outcome.usedSourceFallback).toBe(true);
    expect(outcome.parts).toEqual([
      { kind: "literal", value: "Gain {shown} " },
      { kind: "placeholder", name: "card", value: "Radiant Echo", annotation },
      { kind: "literal", value: " for " },
      { kind: "placeholder", name: "count", value: "2" },
      { kind: "literal", value: " turns (" },
      { kind: "placeholder", name: "plain", value: "true" },
      { kind: "literal", value: ")." },
    ]);
    expect(join(outcome.parts)).toBe(localizer.resolve(value));
    expect(diagnostics).toEqual(["trox.missing-message", "trox.missing-message"]);
  });

  it("follows target reordering, repetition, and omission exactly", () => {
    const value = txa(
      "Offer {card} to {player} for {count} turns.",
      { card: "Echo", player: "Ada", count: 3 },
      "Offer summary.",
    );
    const annotation = { kind: "entity", entityId: "echo" } as const;
    const annotated = value.annotate({ card: annotation });
    const source = sourceBundle([value]);
    const target = bundle("es", [[value, "{player}: {card} / {card}"]]);
    const localizer = new Localizer(target, source);

    const parts = localizer.resolvePartsChecked(annotated);
    expect(parts).toEqual([
      { kind: "placeholder", name: "player", value: "Ada" },
      { kind: "literal", value: ": " },
      { kind: "placeholder", name: "card", value: "Echo", annotation },
      { kind: "literal", value: " / " },
      { kind: "placeholder", name: "card", value: "Echo", annotation },
    ]);
    expect(parts[2]?.kind === "placeholder" && parts[2].annotation).toBe(annotation);
    expect(parts[4]?.kind === "placeholder" && parts[4].annotation).toBe(annotation);
    expect(parts.some((part) => part.kind === "placeholder" && part.name === "count")).toBe(false);
    expect(join(parts)).toBe(localizer.resolveChecked(value));
  });

  it("keeps text, number, term, nested opaque, and bidi behavior aligned with string resolution", () => {
    const nested = tx("Radiant Echo", "Card name.");
    const value = txa(
      "{text} / {number} / {noun} / {nested}",
      {
        text: "Ada",
        number: 12_345,
        noun: counted(termId("card"), 2),
        nested: opaque(nested),
      },
      "Mixed placeholder surfaces.",
    );
    const annotated = value.annotate({
      text: { kind: "text" },
      nested: { kind: "entity" },
    });
    const source = sourceBundle([value, nested]);
    source.terms.card = {
      facets: {},
      forms: {
        $default: { kind: "scalar", origin_locale: "en-US", text: "card" },
        counted: { kind: "number", values: { other: { origin_locale: "en-US", text: "cards" } } },
      },
    };
    const target = bundle("es", [[value, "{nested} · {noun} · {number} · {text}"], [nested, "Eco radiante"]], true);
    target.terms.card = {
      facets: {},
      forms: { counted: { kind: "number", values: { other: { origin_locale: "es", text: "cartas" } } } },
    };
    const localizer = new Localizer(target, source);

    const parts = localizer.resolvePartsChecked(annotated);
    expect(parts.filter((part) => part.kind === "placeholder").map((part) => [part.name, part.value])).toEqual([
      ["nested", "\u2068Eco radiante\u2069"],
      ["noun", "\u2068cartas\u2069"],
      ["number", "\u206812,345\u2069"],
      ["text", "\u2068Ada\u2069"],
    ]);
    expect(join(parts)).toBe(localizer.resolveChecked(value));
  });

  it("preserves recovering diagnostics and rejects malformed translations and unknown annotations", () => {
    const value = txa("Create {noun}.", { noun: counted(termId("missing"), 2) }, "Creation result.");
    const annotated = value.annotate({ noun: { kind: "term" } });
    const diagnostics: string[] = [];
    const localizer = new Localizer(bundle("es", [[value, "Crear {noun}."]]), sourceBundle([value]), {
      diagnostic: (diagnostic) => diagnostics.push(diagnostic.code),
    });

    expect(join(localizer.resolveParts(annotated))).toBe(localizer.resolve(value));
    expect(diagnostics).toEqual(["trox.unknown-term", "trox.unknown-term"]);
    expect(() => value.annotate({ missing: { kind: "entity" } })).toThrow(/declared placeholder/);
    expect(() => JSON.stringify(annotated)).toThrow(/not serializable/);

    const explicitlyUndefined = value.annotate({ noun: undefined });
    const undefinedPart = localizer.resolveParts(explicitlyUndefined)
      .find((part) => part.kind === "placeholder");
    expect(undefinedPart).toHaveProperty("annotation");
    expect(undefinedPart?.kind === "placeholder" && undefinedPart.annotation).toBeUndefined();

    expect(() => new Localizer(bundle("es", [[value, "Crear {noun"]]), sourceBundle([value])))
      .toThrow(/unclosed/);
  });
});

function join<T>(parts: readonly ResolvedLocalizedPart<T>[]): string {
  return parts.map((part) => part.value).join("");
}

function sourceBundle(values: readonly LocalizedString[]): Bundle {
  const source = bundle("en-US");
  for (const value of values) {
    source.entries[value.entryId] = {
      arguments: argumentSchemas(value),
      identity: value.identity,
      rows: {},
      source_signature: value.sourceSignature,
    };
  }
  return source;
}

function argumentSchemas(value: LocalizedString): Record<string, ArgumentSchema> {
  return Object.fromEntries(Object.entries(value.arguments).map(([name, argument]) => [name,
    argument.kind === "term"
      ? { kind: "term", ...(argument.form === undefined ? {} : { form: argument.form }), number: argument.number !== undefined }
      : argument.kind === "opaque" ? { kind: "opaque" } : { kind: "scalar" },
  ]));
}

function bundle(
  locale: string,
  rows: readonly (readonly [LocalizedString, string])[] = [],
  isolate = false,
): Bundle {
  const entries: Record<string, BundleEntry> = {};
  for (const [value, translation] of rows) {
    const expansion = { entry_signature: value.sourceSignature, path: [] };
    const digest = blake3(new TextEncoder().encode(canonicalJson(expansion)));
    entries[value.entryId] = {
      rows: {
        [`row1_${base32(digest.slice(0, 16))}`]: { expansion, origin_locale: locale, translation },
      },
      source_signature: value.sourceSignature,
    };
  }
  return {
    cldr_version: "48", direction: "ltr", entries,
    fallback_chain: locale === "en-US" ? [] : ["en-US"], fallbacks_flattened: true,
    format: "trox-bundle", isolation: isolate ? "isolate" : "none", locale, message_facets: [],
    number_format: { decimal: ".", digits: "0123456789", exponent: "E", group: ",", grouping: [3, 3], minimum_grouping_digits: 1, minus: "-", plus: "+" },
    plural_rules: { cardinal: { one: "i = 1 and v = 0", other: "" }, ordinal: { other: "" } },
    source_catalog_fingerprint: "0".repeat(64), source_locale: "en-US", terms: {}, version: { major: 1, minor: 0 },
  };
}

function base32(bytes: Uint8Array): string {
  const alphabet = "abcdefghijklmnopqrstuvwxyz234567"; let bits = 0; let accumulator = 0; let output = "";
  for (const byte of bytes) { accumulator = (accumulator << 8) | byte; bits += 8; while (bits >= 5) { bits -= 5; output += alphabet[(accumulator >>> bits) & 31]; } }
  if (bits > 0) output += alphabet[(accumulator << (5 - bits)) & 31]; return output;
}
