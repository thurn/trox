import { blake3 } from "@noble/hashes/blake3.js";
import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

import {
  bundleFromCanonicalJSON,
  canonicalJson,
  formatNumber,
  Localizer,
  meaning,
  one,
  other,
  otherwise,
  plural,
  select,
  SourceCatalog,
  term,
  termId,
  TroxDeserializeError,
  TroxValueError,
  tx,
  txa,
  when,
  type ArgumentInput,
  type ArgumentSchema,
  type Bundle,
  type BundleEntry,
  type BundleTerm,
  type LocalizedString,
} from "./index.js";

describe("robust deserialization", () => {
  it("reports null bundle containers as TroxDeserializeError", () => {
    const malformed = { ...testBundle("en-US"), entries: null };

    expect(() => bundleFromCanonicalJSON(canonicalJson(malformed)))
      .toThrow(TroxDeserializeError);
  });

  it("reports a null localized wire value as TroxDeserializeError", () => {
    const catalog = new SourceCatalog(testBundle("en-US"));

    expect(() => catalog.localizedStringFromJSON("null"))
      .toThrow(TroxDeserializeError);
  });

  it("rejects a numeric bundle-row translation", () => {
    const value = tx("Hello", "Greeting.");
    const target = targetBundleWithRow(value, "Hola");
    const row = Object.values(target.entries[value.entryId]!.rows)[0]!;
    (row as unknown as { translation: unknown }).translation = 123;

    expect(() => bundleFromCanonicalJSON(canonicalJson(target)))
      .toThrow(TroxDeserializeError);
  });

  it("rejects empty bundle rows and term surfaces", () => {
    const value = tx("Hello", "Greeting.");
    const emptyRow = targetBundleWithRow(value, "");
    expect(() => bundleFromCanonicalJSON(canonicalJson(emptyRow)))
      .toThrow(/nonempty string/);

    const emptyScalar = testBundle("es", {}, {
      card: {
        facets: {},
        forms: { $default: { kind: "scalar", origin_locale: "es", text: "" } },
      },
    });
    expect(() => bundleFromCanonicalJSON(canonicalJson(emptyScalar)))
      .toThrow(/nonempty string/);

    const emptyNumber = testBundle("es", {}, {
      card: {
        facets: {},
        forms: {
          counted: {
            kind: "number",
            values: { other: { origin_locale: "es", text: "" } },
          },
        },
      },
    });
    expect(() => bundleFromCanonicalJSON(canonicalJson(emptyNumber)))
      .toThrow(/nonempty string/);
  });

  it("rejects target entry IDs with noncanonical unused base32 bits", () => {
    const target = testBundle("es", {
      [`tx1_${"a".repeat(25)}b`]: {
        rows: {},
        source_signature: "0".repeat(64),
      },
    });

    expect(() => bundleFromCanonicalJSON(canonicalJson(target)))
      .toThrow(/malformed entry ID/);
  });

  it("rejects a non-boolean value in a boolean wire argument", () => {
    const value = txa("Enabled: {enabled}", { enabled: true }, "Status.");
    const catalog = catalogFor(value);
    const wire = structuredClone(value.wireValue());
    (wire.arguments.enabled as unknown as { value: unknown }).value = "true";

    expect(() => catalog.localizedStringFromJSON(canonicalJson(wire)))
      .toThrow(TroxDeserializeError);
  });

  it("rejects numeric select keys and values in localized wire data", () => {
    const value = tx(
      select("enabled", [when("enabled", "Yes"), otherwise("No")]),
      "Status.",
    );
    const catalog = catalogFor(value);
    const wire = structuredClone(value.wireValue());
    const selector = wire.selectors[0] as unknown as {
      branch_keys: unknown[];
      value: unknown;
    };
    selector.branch_keys = [1];
    selector.value = 1;

    expect(() => catalog.localizedStringFromJSON(canonicalJson(wire)))
      .toThrow(TroxDeserializeError);
  });

  it("rejects a non-string stable ID in otherwise authorized wire data", () => {
    const value = txa(
      "Create {noun}.",
      { noun: term(termId("true")).finish() },
      "Creation result.",
    );
    const catalog = catalogFor(value, {
      true: {
        facets: {},
        forms: { $default: { kind: "scalar", origin_locale: "en-US", text: "value" } },
      },
    });
    const wire = structuredClone(value.wireValue());
    (wire.arguments.noun as unknown as { term_id: unknown }).term_id = true;

    expect(() => catalog.localizedStringFromJSON(canonicalJson(wire)))
      .toThrow(TroxDeserializeError);
  });
});

describe("robust argument construction", () => {
  it.each([
    ["invalid term ID", { kind: "term", term_id: "Not valid" }],
    ["invalid form ID", { kind: "term", term_id: "card", form: "Not valid" }],
    ["negative term number", { kind: "term", term_id: "card", number: -1 }],
  ])("rejects a forged %s", (_label, forged) => {
    expect(() => txa(
      "Create {noun}.",
      { noun: forged as ArgumentInput },
      "Creation result.",
    )).toThrow(TroxValueError);
  });

  it("rejects a forged non-atomic opaque value", () => {
    const nested = txa("Hello {name}", { name: "Ada" }, "Greeting.");
    const forged = {
      kind: "opaque",
      value: nested.wireValue(),
    } as ArgumentInput;

    expect(() => txa("Value: {value}", { value: forged }, "Opaque value."))
      .toThrow(TroxValueError);
  });

  it("rejects non-string stable IDs instead of coercing them", () => {
    expect(() => txa(
      "Create {noun}.",
      { noun: { kind: "term", term_id: true } as unknown as ArgumentInput },
      "Creation result.",
    )).toThrow(TroxValueError);
  });

  it("matches Rust by rejecting nested meaning and duplicate term forms", () => {
    expect(() => plural(1, [
      one(meaning("singular-card", "card")),
      other("cards"),
    ])).toThrow(/top-level pattern/);
    expect(() => term(termId("card")).form("indefinite").form("counted"))
      .toThrow(/duplicate term form/);
  });

  it("rejects more than 256 visible arguments", () => {
    const allowedNames = Array.from({ length: 256 }, (_, index) => `arg${index}`);
    const allowedPattern = allowedNames.map((name) => `{${name}}`).join(" ");
    const allowedArguments = Object.fromEntries(allowedNames.map((name) => [name, name]));
    expect(() => txa(allowedPattern, allowedArguments, "Argument boundary."))
      .not.toThrow();

    const names = Array.from({ length: 257 }, (_, index) => `arg${index}`);
    const pattern = names.map((name) => `{${name}}`).join(" ");
    const argumentsByName = Object.fromEntries(names.map((name) => [name, name]));

    expect(() => txa(pattern, argumentsByName, "Argument limit."))
      .toThrow(TroxValueError);
  });

  it("rejects a catalog-authorized wire value with 257 arguments", () => {
    const names = Array.from({ length: 257 }, (_, index) => `arg${index}`);
    const identity = {
      identity_version: 1 as const,
      meaning: null,
      pattern: { kind: "text" as const, text: names.map((name) => `{${name}}`).join(" ") },
    };
    const digest = blake3(new TextEncoder().encode(canonicalJson(identity)));
    const entryId = `tx1_${base32(digest.slice(0, 16))}`;
    const source = testBundle("en-US", {
      [entryId]: {
        arguments: Object.fromEntries(names.map((name) => [name, { kind: "scalar" as const }])),
        identity,
        rows: {},
        source_signature: hex(digest),
      },
    });
    const wire = {
      arguments: Object.fromEntries(names.map((name) => [name, { kind: "text", value: name }])),
      entry_id: entryId,
      format: "trox-localized-string",
      identity,
      selectors: [],
      source_signature: hex(digest),
      version: { major: 1, minor: 0 },
    };

    expect(() => new SourceCatalog(source).localizedStringFromJSON(canonicalJson(wire)))
      .toThrow(TroxDeserializeError);
  });
});

describe("source catalog authorization", () => {
  it("accepts the shared counted-term catalog contract", () => {
    const sourceJson = readFileSync(
      new URL("../../../conformance/source-term-contract.json", import.meta.url),
      "utf8",
    ).trimEnd();
    const wireJson = readFileSync(
      new URL("../../../conformance/localized-counted-term.json", import.meta.url),
      "utf8",
    ).trimEnd();
    const source = bundleFromCanonicalJSON(sourceJson);

    expect(new SourceCatalog(source).localizedStringFromJSON(wireJson).toCanonicalJSON())
      .toBe(wireJson);
  });

  it.each([
    ["scalar", { kind: "text", value: "cards" }],
    ["opaque", { kind: "opaque", value: tx("Card", "Card label.").wireValue() }],
    ["different form", { kind: "term", form: "indefinite", term_id: "unit.card" }],
    ["different number policy", { kind: "term", form: "counted", term_id: "unit.card" }],
  ])("rejects a %s substitution before global term authorization", (_label, replacement) => {
    const sourceJson = readFileSync(
      new URL("../../../conformance/source-term-contract.json", import.meta.url),
      "utf8",
    ).trimEnd();
    const wireJson = readFileSync(
      new URL("../../../conformance/localized-counted-term.json", import.meta.url),
      "utf8",
    ).trimEnd();
    const source = bundleFromCanonicalJSON(sourceJson);
    const wire = JSON.parse(wireJson) as { arguments: Record<string, unknown> };
    wire.arguments.noun = replacement;

    expect(() => new SourceCatalog(source).localizedStringFromJSON(canonicalJson(wire)))
      .toThrow(/source entry schema/);
  });

  it("enforces source-only argument schema maps with exact placeholder keys", () => {
    const value = txa("Hello {name}", { name: "Ada" }, "Greeting.");
    const source = sourceBundleFor(value);
    const missing = structuredClone(source);
    delete (missing.entries[value.entryId] as { arguments?: unknown }).arguments;
    expect(() => new SourceCatalog(missing)).toThrow(/requires arguments/);

    const mismatched = structuredClone(source);
    (mismatched.entries[value.entryId] as { arguments?: unknown }).arguments = {
      different_name: { kind: "scalar" },
    };
    expect(() => new SourceCatalog(mismatched)).toThrow(/differ from its placeholders/);

    const target = targetBundleWithRow(value, "Hola {name}");
    (target.entries[value.entryId] as { arguments?: unknown }).arguments = {
      name: { kind: "scalar" },
    };
    expect(() => new Localizer(target, source)).toThrow(/must not contain arguments/);
  });

  it("does not authorize a missing term through Object.prototype", () => {
    const value = txa(
      "Create {noun}.",
      { noun: term(termId("constructor")).finish() },
      "Creation result.",
    );
    const catalog = catalogFor(value);

    expect(() => catalog.localizedStringFromJSON(value.toCanonicalJSON()))
      .toThrow(TroxDeserializeError);
  });

  it("rejects a term form absent from the source catalog", () => {
    const value = txa(
      "Create {noun}.",
      { noun: term(termId("card")).form("missing").finish() },
      "Creation result.",
    );
    const terms: Record<string, BundleTerm> = {
      card: {
        facets: {},
        forms: {
          $default: { kind: "scalar", origin_locale: "en-US", text: "card" },
        },
      },
    };
    const catalog = catalogFor(value, terms);

    expect(() => catalog.localizedStringFromJSON(value.toCanonicalJSON()))
      .toThrow(TroxDeserializeError);
  });
});

describe("reliable recovery", () => {
  it("contains exceptions thrown by the diagnostic hook", () => {
    const value = tx("Source fallback", "Fallback text.");
    const source = sourceBundleFor(value);
    const target = testBundle("es");
    const localizer = new Localizer(target, source, {
      diagnostic: () => {
        throw new Error("diagnostic sink failed");
      },
    });

    expect(localizer.resolve(value)).toBe("Source fallback");
  });

  it("recovers a missing target term form inside the translated row", () => {
    const value = txa(
      "Create {noun} now.",
      { noun: term(termId("card")).form("indefinite").finish() },
      "Creation result.",
    );
    const source = sourceBundleFor(value, {
      card: {
        facets: {},
        forms: {
          $default: { kind: "scalar", origin_locale: "en-US", text: "card" },
          indefinite: { kind: "scalar", origin_locale: "en-US", text: "a card" },
        },
      },
    });
    const target = {
      ...targetBundleWithRow(value, "Crea {noun} ahora."),
      terms: {
        card: {
          facets: {},
          forms: { $default: { kind: "scalar" as const, origin_locale: "es", text: "tarjeta" } },
        },
      },
    };
    const diagnostics: string[] = [];
    const localizer = new Localizer(target, source, {
      diagnostic: (diagnostic) => diagnostics.push(diagnostic.code),
    });

    expect(() => localizer.resolveChecked(value)).toThrow(/missing term form/);
    expect(localizer.resolve(value)).toBe("Crea \u2068card\u2069 ahora.");
    expect(diagnostics).toEqual(["trox.missing-term-form"]);
  });
});

describe("bundle/runtime conformance", () => {
  it("matches Rust minimum-grouping behavior for larger magnitudes", () => {
    const format = {
      decimal: ".", digits: "0123456789", exponent: "E", group: ",",
      grouping: [3, 3] as const, minimum_grouping_digits: 2, minus: "-", plus: "+",
    };
    expect(formatNumber(9_999, format)).toBe("9999");
    expect(formatNumber(10_000, format)).toBe("10,000");
    expect(formatNumber(1_000_000, format)).toBe("1,000,000");
  });

  it("rejects duplicate number digits and noncanonical BCP-47 locales", () => {
    const duplicateDigits = testBundle("en-US");
    (duplicateDigits.number_format as { digits: string }).digits = "0012345678";
    expect(() => bundleFromCanonicalJSON(canonicalJson(duplicateDigits)))
      .toThrow(/distinct Unicode scalars/);

    const badLocale = { ...testBundle("en-US"), locale: "en-US-US", source_locale: "en-US-US" };
    expect(() => bundleFromCanonicalJSON(canonicalJson(badLocale)))
      .toThrow(/noncanonical locale/);
  });

  it("rejects malformed expansion paths before source compatibility checks", () => {
    const target = testBundle("es");
    const expansion = { entry_signature: "1".repeat(64), path: ["bogus"] };
    const rowId = hashId("row1", expansion);
    (target.entries as Record<string, BundleEntry>)[`tx1_${"a".repeat(26)}`] = {
      rows: { [rowId]: { expansion, origin_locale: "es", translation: "unused" } },
      source_signature: "1".repeat(64),
    };

    expect(() => bundleFromCanonicalJSON(canonicalJson(target)))
      .toThrow(/expansion step/);
  });

  it("rejects semantically unreachable message-facet rows", () => {
    const value = txa(
      "Use {noun}.",
      { noun: term(termId("card")).finish() },
      "Instruction using a card.",
    );
    const source = sourceBundleFor(value, {
      card: {
        facets: {},
        forms: { $default: { kind: "scalar", origin_locale: "en-US", text: "card" } },
      },
    });
    const expansion = {
      entry_signature: value.sourceSignature,
      path: [{ argument: "noun", facet: "gender", kind: "facet", value: "ghost" }],
    };
    const rowId = hashId("row1", expansion);
    const target = testBundle("es", {
      [value.entryId]: {
        rows: { [rowId]: { expansion, origin_locale: "es", translation: "Usa {noun}." } },
        source_signature: value.sourceSignature,
      },
    }, {
      card: {
        facets: { gender: "masculine" },
        forms: { $default: { kind: "scalar", origin_locale: "es", text: "tarjeta" } },
      },
    });
    (target.message_facets as string[]).push("gender");

    expect(() => new Localizer(target, source)).toThrow(/not reachable/);
  });

  it("does not hash successful row lookups after Localizer construction", () => {
    const value = tx("Snapshot", "Compiled row lookup test.");
    const localizer = new Localizer(targetBundleWithRow(value, "Instantánea"), sourceBundleFor(value));
    const original = TextEncoder.prototype.encode;
    let calls = 0;
    TextEncoder.prototype.encode = function encode(input?: string): Uint8Array {
      calls += 1;
      return original.call(this, input);
    };
    try {
      expect(localizer.resolve(value)).toBe("Instantánea");
      expect(localizer.resolveChecked(value)).toBe("Instantánea");
    } finally {
      TextEncoder.prototype.encode = original;
    }
    expect(calls).toBe(0);
  });
});

describe("bundle snapshot boundaries", () => {
  it("defers and memoizes identity hashing until an ID is requested", () => {
    const original = TextEncoder.prototype.encode;
    let calls = 0;
    TextEncoder.prototype.encode = function encode(input?: string): Uint8Array {
      calls += 1;
      return original.call(this, input);
    };
    try {
      const value = tx("Lazy identity", "Lazy identity test.");
      expect(calls).toBe(0);
      expect(value.identity.pattern.kind).toBe("text");
      expect(calls).toBe(0);
      void value.entryId;
      expect(calls).toBe(1);
      void value.sourceSignature;
      expect(calls).toBe(1);
    } finally {
      TextEncoder.prototype.encode = original;
    }
  });

  it("clones each validated localizer bundle exactly once", () => {
    const value = tx("Snapshot", "Snapshot test.");
    const source = sourceBundleFor(value);
    const target = testBundle("es");
    const original = globalThis.structuredClone;
    let calls = 0;
    globalThis.structuredClone = ((input: unknown) => {
      calls += 1;
      return original(input);
    }) as typeof structuredClone;
    try {
      new Localizer(target, source);
    } finally {
      globalThis.structuredClone = original;
    }
    expect(calls).toBe(2);
  });
});

describe("Unicode validity", () => {
  it("rejects lone UTF-16 surrogates in canonical JSON and authored text", () => {
    expect(() => canonicalJson("\uD800")).toThrow(TroxValueError);
    expect(() => tx("\uD800", "Invalid source text."))
      .toThrow(TroxValueError);
  });

  it("rejects non-NFC text in a bundle row", () => {
    const value = tx("Hello", "Greeting.");
    const target = targetBundleWithRow(value, "e\u0301");

    expect(() => bundleFromCanonicalJSON(canonicalJson(target)))
      .toThrow(TroxDeserializeError);
  });
});

function catalogFor(
  value: LocalizedString,
  terms: Readonly<Record<string, BundleTerm>> = {},
): SourceCatalog {
  return new SourceCatalog(sourceBundleFor(value, terms));
}

function sourceBundleFor(
  value: LocalizedString,
  terms: Readonly<Record<string, BundleTerm>> = {},
): Bundle {
  return testBundle(
    "en-US",
    {
      [value.entryId]: {
        arguments: argumentSchemasFor(value),
        identity: value.identity,
        rows: {},
        source_signature: value.sourceSignature,
      },
    },
    terms,
  );
}

function argumentSchemasFor(value: LocalizedString): Record<string, ArgumentSchema> {
  return Object.fromEntries(Object.entries(value.arguments).map(([name, argument]) => {
    switch (argument.kind) {
      case "text":
      case "number":
      case "boolean":
        return [name, { kind: "scalar" }];
      case "opaque":
        return [name, { kind: "opaque" }];
      case "term":
        return [name, {
          ...(argument.form === undefined ? {} : { form: argument.form }),
          kind: "term",
          number: argument.number !== undefined,
        }];
    }
  }));
}

function targetBundleWithRow(value: LocalizedString, translation: string): Bundle {
  const expansion = { entry_signature: value.sourceSignature, path: [] };
  const rowId = hashId("row1", expansion);
  const entry: BundleEntry = {
    rows: {
      [rowId]: { expansion, origin_locale: "es", translation },
    },
    source_signature: value.sourceSignature,
  };
  return testBundle("es", { [value.entryId]: entry });
}

function testBundle(
  locale: string,
  entries: Bundle["entries"] = {},
  terms: Bundle["terms"] = {},
): Bundle {
  return {
    cldr_version: "48",
    direction: "ltr",
    entries,
    fallback_chain: locale === "en-US" ? [] : ["en-US"],
    fallbacks_flattened: true,
    format: "trox-bundle",
    isolation: "isolate",
    locale,
    message_facets: [],
    number_format: {
      decimal: ".",
      digits: "0123456789",
      exponent: "E",
      group: ",",
      grouping: [3, 3],
      minimum_grouping_digits: 1,
      minus: "-",
      plus: "+",
    },
    plural_rules: {
      cardinal: { one: "i = 1 and v = 0", other: "" },
      ordinal: { other: "" },
    },
    source_catalog_fingerprint: "0".repeat(64),
    source_locale: "en-US",
    terms,
    version: { major: 1, minor: 0 },
  };
}

function hashId(prefix: string, value: unknown): string {
  const digest = blake3(new TextEncoder().encode(canonicalJson(value)));
  return `${prefix}_${base32(digest.slice(0, 16))}`;
}

function base32(bytes: Uint8Array): string {
  const alphabet = "abcdefghijklmnopqrstuvwxyz234567";
  let accumulator = 0;
  let bits = 0;
  let output = "";
  for (const byte of bytes) {
    accumulator = (accumulator << 8) | byte;
    bits += 8;
    while (bits >= 5) {
      bits -= 5;
      output += alphabet[(accumulator >>> bits) & 31];
    }
  }
  if (bits > 0) output += alphabet[(accumulator << (5 - bits)) & 31];
  return output;
}

function hex(bytes: Uint8Array): string {
  return [...bytes].map((byte) => byte.toString(16).padStart(2, "0")).join("");
}
