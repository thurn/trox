import { blake3 } from "@noble/hashes/blake3.js";
import { TroxDeserializeError, TroxResolveError, TroxValueError } from "./errors.js";
import { formatNumber, type NumberFormat } from "./number-format.js";
import { base32, canonicalJson, deepFreeze, hex } from "./canonical-json.js";
import { CompiledPluralRules, compilePluralCondition } from "./plural-evaluation.js";
import { assertDictionary, assertObjectKeys, deserializeBoundary, ownValue, parseCanonicalJson } from "./deserialization.js";
import {
  CATEGORIES,
  CONSTRUCTION_TOKEN,
  LOCALIZATION_TODO_MEANING,
  LocalizedString,
  localizationTodoPattern,
  PLACEHOLDER,
  assertNfc,
  assertSelectorInteger,
  assertStableId,
  collectPatternPlaceholders,
  parsePlaceholders,
  validateArgumentMap,
  validateIdentity,
  validateSelectorRecords,
  type Argument,
  type IdentityDescriptor,
  type LocalizedStringWire,
  type NumericIdentityBranch,
  type Pattern,
  type PluralCategory,
  type SelectorRecord,
  type TermArgument,
} from "./authoring.js";

export { TroxDeserializeError, TroxResolveError, TroxValueError } from "./errors.js";
export { formatNumber, type NumberFormat } from "./number-format.js";
export { canonicalJson } from "./canonical-json.js";

function assertNfcForDeserialize(value: string, label: string): void {
  try {
    assertNfc(value, label);
  } catch (error) {
    if (error instanceof TroxValueError) {
      const prefix = `${error.code}: `;
      throw new TroxDeserializeError(error.code, error.message.startsWith(prefix) ? error.message.slice(prefix.length) : error.message);
    }
    throw error;
  }
}

export interface BundleTermSurface { readonly origin_locale: string; readonly text: string }
export type BundleTermForm =
  | { readonly kind: "scalar"; readonly origin_locale: string; readonly text: string }
  | { readonly kind: "number"; readonly values: Partial<Record<PluralCategory, BundleTermSurface>> };
export interface BundleTerm { readonly facets: Readonly<Record<string, string>>; readonly forms: Readonly<Record<string, BundleTermForm>> }
export interface ExpansionDescriptor { readonly entry_signature: string; readonly path: readonly unknown[] }
export interface BundleRow { readonly expansion: ExpansionDescriptor; readonly origin_locale: string; readonly translation: string }
export type ArgumentSchema =
  | { readonly kind: "scalar" }
  | { readonly kind: "opaque" }
  | { readonly kind: "term"; readonly form?: string; readonly number: boolean };
export interface BundleEntry {
  readonly arguments?: Readonly<Record<string, ArgumentSchema>>;
  readonly source_signature: string;
  readonly rows: Readonly<Record<string, BundleRow>>;
  readonly identity?: IdentityDescriptor;
}
export interface Bundle {
  readonly cldr_version: string; readonly direction: "ltr" | "rtl";
  readonly entries: Readonly<Record<string, BundleEntry>>; readonly fallback_chain: readonly string[];
  readonly fallbacks_flattened: true; readonly format: "trox-bundle";
  readonly isolation: "isolate" | "none"; readonly locale: string;
  readonly message_facets: readonly string[]; readonly number_format: NumberFormat;
  readonly plural_rules: { readonly cardinal: Partial<Record<PluralCategory, string>>; readonly ordinal: Partial<Record<PluralCategory, string>> };
  readonly source_catalog_fingerprint: string; readonly source_locale: string;
  readonly terms: Readonly<Record<string, BundleTerm>>; readonly version: { readonly major: 1; readonly minor: 0 };
}

export interface Diagnostic { readonly code: string; readonly entry_id?: string; readonly message: string }

class CompiledBundle {
  readonly wire: Bundle;
  readonly cardinal: CompiledPluralRules;
  readonly ordinal: CompiledPluralRules;
  readonly #rowIndex = new Map<string, ReadonlyMap<string, BundleRow>>();

  constructor(wire: Bundle) {
    this.wire = wire;
    this.cardinal = CompiledPluralRules.compile(wire.plural_rules.cardinal)!;
    this.ordinal = CompiledPluralRules.compile(wire.plural_rules.ordinal)!;
    for (const [entryId, entry] of Object.entries(wire.entries)) {
      const rows = new Map<string, BundleRow>();
      for (const row of Object.values(entry.rows)) rows.set(expansionPathKey(row.expansion.path), row);
      this.#rowIndex.set(entryId, rows);
    }
  }

  row(entryId: string, pathKey: string): BundleRow | undefined {
    return this.#rowIndex.get(entryId)?.get(pathKey);
  }
}

const VALIDATED_SOURCE_TOKEN = Symbol("trox-validated-source");

interface CatalogEntry {
  readonly arguments: Readonly<Record<string, ArgumentSchema>>;
  readonly identity: IdentityDescriptor;
  readonly source_signature: string;
}

export class SourceCatalog {
  readonly #entries: Readonly<Record<string, CatalogEntry>>;
  readonly #terms: Readonly<Record<string, BundleTerm>>;
  readonly fingerprint: string;
  constructor(source: Bundle);
  /** @internal */
  constructor(source: Bundle, internalToken: symbol);
  constructor(source: Bundle, internalToken?: symbol) {
    if (internalToken !== VALIDATED_SOURCE_TOKEN) deserializeBoundary(() => validateBundle(source));
    if (source.locale !== source.source_locale) throw new TroxDeserializeError("trox.invalid-source-bundle", "source locale bundle required");
    const sourceEntries = internalToken === VALIDATED_SOURCE_TOKEN ? source.entries : structuredClone(source.entries);
    this.#entries = deepFreeze(Object.fromEntries(Object.entries(sourceEntries).map(([entryId, entry]) => [
      entryId,
      {
        arguments: entry.arguments!,
        identity: entry.identity!,
        source_signature: entry.source_signature,
      },
    ])));
    if (internalToken === VALIDATED_SOURCE_TOKEN) {
      this.#terms = source.terms;
    } else {
      this.#terms = deepFreeze(structuredClone(source.terms));
    }
    this.fingerprint = source.source_catalog_fingerprint;
  }
  localizedStringFromJSON(input: string): LocalizedString {
    return deserializeBoundary(() => {
      const parsed = parseCanonicalJson(input, "localized value");
      assertObjectKeys(parsed, ["arguments", "entry_id", "format", "identity", "selectors", "source_signature", "version"], [], "localized string");
      assertObjectKeys(parsed.identity, ["identity_version", "meaning", "pattern"], [], "identity");
      assertObjectKeys(parsed.version, ["major", "minor"], [], "localized string version");
      assertDictionary(parsed.arguments, "localized string arguments");
      if (!Array.isArray(parsed.selectors)) throw new TroxDeserializeError("trox.wire-shape", "selectors must be an array");
      if (parsed.format !== "trox-localized-string" || parsed.version.major !== 1 || parsed.version.minor !== 0) {
        throw new TroxDeserializeError("trox.wire-version", "unsupported localized string version");
      }
      const wire = parsed as unknown as LocalizedStringWire;
      validatePatternShape(wire.identity.pattern);
      validateIdentity(wire.identity);
      validateSelectorRecords(wire.identity.pattern, wire.selectors);
      validateWireArguments(wire);
      const digest = blake3(new TextEncoder().encode(canonicalJson(wire.identity)));
      const entryId = `tx1_${base32(digest.slice(0, 16))}`;
      const signature = hex(digest);
      if (entryId !== wire.entry_id || signature !== wire.source_signature) {
        throw new TroxDeserializeError("trox.identity-mismatch", "wire identity hash mismatch");
      }
      if (wire.identity.meaning === LOCALIZATION_TODO_MEANING) {
        const todo = LocalizedString.fromValidatedWire(wire, CONSTRUCTION_TOKEN);
        if (localizationTodoPattern(todo) !== undefined) return todo;
        throw new TroxDeserializeError("trox.unauthorized-entry", "invalid localization TODO value");
      }
      const authorized = ownValue(this.#entries, entryId);
      if (authorized?.source_signature !== signature || canonicalJson(authorized.identity) !== canonicalJson(wire.identity)) {
        throw new TroxDeserializeError("trox.unauthorized-entry", `entry ${entryId} is not authorized`);
      }
      authorizeArgumentSchemas(authorized.arguments, wire.arguments);
      for (const argument of Object.values(wire.arguments)) {
        if (argument.kind === "term") this.authorizeTerm(argument);
        if (argument.kind === "opaque") this.localizedStringFromJSON(canonicalJson(argument.value));
      }
      return LocalizedString.fromValidatedWire(wire, CONSTRUCTION_TOKEN);
    });
  }

  private authorizeTerm(argument: TermArgument): void {
    const termValue = ownValue(this.#terms, argument.term_id);
    if (termValue === undefined) {
      throw new TroxDeserializeError("trox.unknown-term", `unknown term ${argument.term_id}`);
    }
    const formId = argument.form ?? "$default";
    const form = ownValue(termValue.forms, formId);
    if (form === undefined) {
      throw new TroxDeserializeError("trox.missing-term-form", `unknown term form ${argument.term_id}.${formId}`);
    }
    if ((form.kind === "scalar") !== (argument.number === undefined)) {
      throw new TroxDeserializeError("trox.term-number-contract", `term form number contract mismatch for ${argument.term_id}.${formId}`);
    }
  }
}

function authorizeArgumentSchemas(
  schemas: Readonly<Record<string, ArgumentSchema>>,
  argumentsValue: Readonly<Record<string, Argument>>,
): void {
  const schemaNames = Object.keys(schemas).sort();
  const argumentNames = Object.keys(argumentsValue).sort();
  if (canonicalJson(schemaNames) !== canonicalJson(argumentNames)) {
    throw new TroxDeserializeError("trox.argument-schema", "argument schemas differ from the source entry");
  }
  for (const name of schemaNames) {
    const schema = schemas[name]!;
    const argument = argumentsValue[name]!;
    const compatible = schema.kind === "scalar"
      ? argument.kind === "text" || argument.kind === "number" || argument.kind === "boolean"
      : schema.kind === "opaque"
        ? argument.kind === "opaque"
        : argument.kind === "term"
          && schema.form === argument.form
          && schema.number === (argument.number !== undefined);
    if (!compatible) {
      throw new TroxDeserializeError("trox.argument-schema", `argument ${name} does not match its source entry schema`);
    }
  }
}

export class Localizer {
  readonly #target: CompiledBundle;
  readonly #source: CompiledBundle;
  readonly #catalog: SourceCatalog;
  readonly #diagnostic: ((diagnostic: Diagnostic) => void) | undefined;
  constructor(target: Bundle, source: Bundle, options: { readonly strict?: boolean; readonly diagnostic?: (diagnostic: Diagnostic) => void } = {}) {
    deserializeBoundary(() => validateBundle(target));
    deserializeBoundary(() => validateBundle(source));
    if (source.locale !== source.source_locale || target.source_locale !== source.locale) throw new TroxDeserializeError("trox.bundle-pair", "invalid target/source bundle relationship");
    if (options.strict === true && target.source_catalog_fingerprint !== source.source_catalog_fingerprint) throw new TroxDeserializeError("trox.catalog-mismatch", "source catalog fingerprint mismatch");
    const targetSnapshot = deepFreeze(structuredClone(target));
    const sourceSnapshot = deepFreeze(structuredClone(source));
    this.#target = new CompiledBundle(targetSnapshot);
    this.#source = new CompiledBundle(sourceSnapshot);
    this.#catalog = new SourceCatalog(sourceSnapshot, VALIDATED_SOURCE_TOKEN); this.#diagnostic = options.diagnostic;
    deserializeBoundary(() => validateEntryCompatibility(targetSnapshot, sourceSnapshot));
    if (target.source_catalog_fingerprint !== source.source_catalog_fingerprint) {
      this.report({ code: "trox.catalog-mismatch", message: "target and source catalog fingerprints differ; compatible entries remain usable" });
    }
  }
  get sourceCatalog(): SourceCatalog { return this.#catalog; }
  localizedStringFromJSON(input: string): LocalizedString { return this.#catalog.localizedStringFromJSON(input); }
  resolveChecked(value: LocalizedString): string {
    const todoPattern = localizationTodoPattern(value);
    if (todoPattern !== undefined) return this.interpolate(todoPattern, value, false);
    const row = this.targetRow(value);
    return this.interpolate(row.translation, value, true);
  }
  resolve(value: LocalizedString): string {
    const todoPattern = localizationTodoPattern(value);
    if (todoPattern !== undefined) return this.interpolateRecovering(todoPattern, value);
    try { return this.interpolateRecovering(this.targetRow(value).translation, value, true); }
    catch (error) {
      this.emit(error, value.entryId);
      try { return this.interpolateRecovering(selectPattern(this.#source, value).text, value); }
      catch (sourceError) { this.emit(sourceError, value.entryId); return `⟦${value.entryId}⟧`; }
    }
  }
  private targetRow(value: LocalizedString): BundleRow {
    const entry = ownValue(this.#target.wire.entries, value.entryId);
    if (entry?.source_signature !== value.sourceSignature) throw new TroxResolveError("trox.missing-message", `message ${value.entryId} is unavailable`);
    const selected = selectPattern(this.#target, value);
    const row = this.#target.row(value.entryId, selected.pathKey);
    if (row !== undefined) return row;
    const expansion: ExpansionDescriptor = { entry_signature: value.sourceSignature, path: selected.path };
    const digest = blake3(new TextEncoder().encode(canonicalJson(expansion)));
    throw new TroxResolveError("trox.missing-row", `row row1_${base32(digest.slice(0, 16))} is unavailable`);
  }
  private interpolate(pattern: string, value: LocalizedString, preferTarget: boolean): string {
    let output = "";
    for (let index = 0; index < pattern.length;) {
      if (pattern.startsWith("{{", index)) { output += "{"; index += 2; continue; }
      if (pattern.startsWith("}}", index)) { output += "}"; index += 2; continue; }
      if (pattern[index] === "{") {
        const end = pattern.indexOf("}", index + 1); if (end < 0) throw new TroxResolveError("trox.malformed-translation", "unclosed placeholder");
        const name = pattern.slice(index + 1, end); const argument = value.arguments[name];
        if (argument === undefined) throw new TroxResolveError("trox.missing-argument", `argument ${name} is missing`);
        const surface = this.argumentSurface(argument, preferTarget);
        output += this.#target.wire.isolation === "isolate" ? `\u2068${surface}\u2069` : surface; index = end + 1; continue;
      }
      if (pattern[index] === "}") throw new TroxResolveError("trox.malformed-translation", "unmatched closing brace");
      output += pattern[index]; index += 1;
    }
    return output;
  }
  private argumentSurface(argument: Argument, preferTarget: boolean): string {
    switch (argument.kind) {
      case "text": return argument.value;
      case "number": return formatNumber(argument.value, this.#target.wire.number_format);
      case "boolean": return String(argument.value);
      case "opaque": {
        const nested = LocalizedString.fromValidatedWire(argument.value, CONSTRUCTION_TOKEN);
        return preferTarget ? this.resolveChecked(nested) : this.resolve(nested);
      }
      case "term": return this.termSurface(argument, preferTarget);
    }
  }
  private interpolateRecovering(pattern: string, value: LocalizedString, preferTarget = false): string {
    let output = "";
    for (let index = 0; index < pattern.length;) {
      if (pattern.startsWith("{{", index)) { output += "{"; index += 2; continue; }
      if (pattern.startsWith("}}", index)) { output += "}"; index += 2; continue; }
      if (pattern[index] === "{") {
        const end = pattern.indexOf("}", index + 1); if (end < 0) throw new TroxResolveError("trox.malformed-translation", "unclosed placeholder");
        const name = pattern.slice(index + 1, end); const argument = value.arguments[name]; let surface: string;
        if (argument === undefined) { this.emit(new TroxResolveError("trox.missing-argument", `argument ${name} is missing`), value.entryId); surface = `{${name}}`; }
        else if (argument.kind === "term") {
          try { surface = this.termSurface(argument, preferTarget); }
          catch (error) {
            this.emit(error, value.entryId);
            const defaultForm = ownValue(ownValue(this.#source.wire.terms, argument.term_id)?.forms ?? {}, "$default");
            surface = defaultForm?.kind === "scalar" ? defaultForm.text : `⟦term:${argument.term_id}⟧`;
          }
        } else if (argument.kind === "opaque") surface = this.resolve(LocalizedString.fromValidatedWire(argument.value, CONSTRUCTION_TOKEN));
        else { try { surface = this.argumentSurface(argument, preferTarget); } catch { surface = `{${name}}`; } }
        output += this.#target.wire.isolation === "isolate" ? `\u2068${surface}\u2069` : surface; index = end + 1; continue;
      }
      if (pattern[index] === "}") throw new TroxResolveError("trox.malformed-translation", "unmatched closing brace");
      output += pattern[index]; index += 1;
    }
    return output;
  }
  private termSurface(argument: TermArgument, preferTarget: boolean): string {
    const bundle = preferTarget ? this.#target : this.#source; const termValue = ownValue(bundle.wire.terms, argument.term_id);
    if (termValue === undefined) throw new TroxResolveError("trox.unknown-term", `unknown term ${argument.term_id}`);
    const formId = argument.form ?? "$default"; const form = ownValue(termValue.forms, formId);
    if (form === undefined) throw new TroxResolveError("trox.missing-term-form", `missing term form ${argument.term_id}.${formId}`);
    if (form.kind === "scalar" && argument.number === undefined) return form.text;
    if (form.kind === "number" && argument.number !== undefined) {
      const category = pluralCategory(bundle, false, argument.number);
      const surface = form.values[category] ?? form.values.other;
      if (surface !== undefined) return surface.text;
    }
    throw new TroxResolveError("trox.missing-term-form", `term form number contract mismatch for ${argument.term_id}.${formId}`);
  }
  private emit(error: unknown, entryId: string): void {
    const message = error instanceof Error ? error.message : String(error);
    const code = error instanceof TroxResolveError ? error.code : "trox.resolve";
    this.report({ code, entry_id: entryId, message });
  }
  private report(diagnostic: Diagnostic): void {
    try {
      this.#diagnostic?.(diagnostic);
    } catch {
      // Diagnostics are observational and must never change resolution behavior.
    }
  }
}

function validateBundle(bundle: Bundle): void {
  assertObjectKeys(bundle, [
    "cldr_version", "direction", "entries", "fallback_chain", "fallbacks_flattened", "format", "isolation", "locale",
    "message_facets", "number_format", "plural_rules", "source_catalog_fingerprint", "source_locale", "terms", "version",
  ], [], "bundle");
  assertObjectKeys(bundle.version, ["major", "minor"], [], "bundle version");
  if (bundle.format !== "trox-bundle" || bundle.version?.major !== 1 || bundle.version.minor !== 0 || bundle.fallbacks_flattened !== true) throw new TroxDeserializeError("trox.bundle-version", "unsupported or malformed bundle");
  if (bundle.cldr_version !== "48") throw new TroxDeserializeError("trox.cldr-version", `unsupported CLDR version ${String(bundle.cldr_version)}`);
  assertLocale(bundle.locale); assertLocale(bundle.source_locale);
  if (bundle.direction !== "ltr" && bundle.direction !== "rtl") throw new TroxDeserializeError("trox.direction", "invalid bundle direction");
  if (bundle.isolation !== "isolate" && bundle.isolation !== "none") throw new TroxDeserializeError("trox.isolation", "invalid isolation policy");
  if (!/^[0-9a-f]{64}$/.test(bundle.source_catalog_fingerprint)) throw new TroxDeserializeError("trox.fingerprint", "source catalog fingerprint must be lowercase hexadecimal");
  if (!Array.isArray(bundle.fallback_chain)) throw new TroxDeserializeError("trox.fallback-chain", "fallback_chain must be an array");
  const sourceBundle = bundle.locale === bundle.source_locale; const fallbacks = new Set<string>();
  for (const locale of bundle.fallback_chain) {
    assertLocale(locale);
    if (locale === bundle.locale || fallbacks.has(locale)) throw new TroxDeserializeError("trox.fallback-chain", "fallback_chain is cyclic or contains duplicates");
    fallbacks.add(locale);
  }
  if (sourceBundle ? bundle.fallback_chain.length !== 0 : bundle.fallback_chain.at(-1) !== bundle.source_locale) throw new TroxDeserializeError("trox.fallback-chain", "target fallback_chain must end in source_locale; source chain must be empty");
  assertObjectKeys(bundle.number_format, ["decimal", "digits", "exponent", "group", "grouping", "minimum_grouping_digits", "minus", "plus"], [], "number format");
  if (typeof bundle.number_format.digits !== "string") throw new TroxDeserializeError("trox.number-format", "digits must be a string");
  if (new Set([...bundle.number_format.digits]).size !== 10) throw new TroxDeserializeError("trox.number-format", "digits must contain ten distinct Unicode scalars");
  if (!Array.isArray(bundle.number_format.grouping) || bundle.number_format.grouping.length !== 2 || bundle.number_format.grouping.some((width) => !Number.isSafeInteger(width) || width <= 0) || !Number.isSafeInteger(bundle.number_format.minimum_grouping_digits) || bundle.number_format.minimum_grouping_digits <= 0 || [bundle.number_format.decimal, bundle.number_format.group, bundle.number_format.exponent, bundle.number_format.minus, bundle.number_format.plus].some((symbol) => typeof symbol !== "string" || symbol.length === 0)) throw new TroxDeserializeError("trox.number-format", "number format has an invalid symbol or grouping width");
  for (const [name, symbol] of Object.entries(bundle.number_format)) {
    if (typeof symbol === "string") assertNfcForDeserialize(symbol, `number format ${name}`);
  }
  assertObjectKeys(bundle.plural_rules, ["cardinal", "ordinal"], [], "plural rules");
  validatePluralRules("cardinal", bundle.plural_rules.cardinal); validatePluralRules("ordinal", bundle.plural_rules.ordinal);
  if (!Array.isArray(bundle.message_facets)) throw new TroxDeserializeError("trox.message-facets", "message_facets must be an array");
  const messageFacets = new Set<string>();
  for (const facet of bundle.message_facets) { assertStableId(facet, "message facet ID"); if (messageFacets.has(facet)) throw new TroxDeserializeError("trox.message-facets", "duplicate message facet ID"); messageFacets.add(facet); }
  assertDictionary(bundle.entries, "bundle entries");
  assertDictionary(bundle.terms, "bundle terms");
  for (const [entryId, entry] of Object.entries(bundle.entries)) {
    assertObjectKeys(entry, ["rows", "source_signature"], ["arguments", "identity"], `entry ${entryId}`);
    assertDictionary(entry.rows, `entry ${entryId} rows`);
    if (!isCanonicalShortId(entryId, "tx1_") || !/^[0-9a-f]{64}$/.test(entry.source_signature)) throw new TroxDeserializeError("trox.bundle-entry", `malformed entry ID or signature for ${entryId}`);
    if (sourceBundle && (entry.arguments === undefined || entry.identity === undefined || Object.keys(entry.rows).length !== 0)) throw new TroxDeserializeError("trox.bundle-entry", `source entry ${entryId} requires arguments, identity, and empty rows`);
    if (!sourceBundle && (entry.arguments !== undefined || entry.identity !== undefined)) throw new TroxDeserializeError("trox.bundle-entry", `target entry ${entryId} must not contain arguments or identity`);
    if (entry.identity !== undefined) {
      assertObjectKeys(entry.identity, ["identity_version", "meaning", "pattern"], [], `identity ${entryId}`);
      validatePatternShape(entry.identity.pattern);
      validateIdentity(entry.identity);
      const digest = blake3(new TextEncoder().encode(canonicalJson(entry.identity)));
      if (`tx1_${base32(digest.slice(0, 16))}` !== entryId || hex(digest) !== entry.source_signature) throw new TroxDeserializeError("trox.bundle-entry", `identity mismatch for ${entryId}`);
      validateArgumentSchemas(entry.arguments, entry.identity.pattern, entryId);
    }
    for (const [rowId, row] of Object.entries(entry.rows)) {
      assertObjectKeys(row, ["expansion", "origin_locale", "translation"], [], `row ${rowId}`);
      assertObjectKeys(row.expansion, ["entry_signature", "path"], [], `row expansion ${rowId}`);
      if (typeof row.translation !== "string" || row.translation.length === 0) throw new TroxDeserializeError("trox.bundle-row", `row ${rowId} translation must be a nonempty string`);
      assertNfcForDeserialize(row.translation, `row ${rowId} translation`);
      if (!Array.isArray(row.expansion.path)) throw new TroxDeserializeError("trox.bundle-row", `row ${rowId} path must be an array`);
      validateExpansionPathShape(row.expansion.path);
      const digest = blake3(new TextEncoder().encode(canonicalJson(row.expansion)));
      if (`row1_${base32(digest.slice(0, 16))}` !== rowId || row.expansion.entry_signature !== entry.source_signature) throw new TroxDeserializeError("trox.bundle-row", `row hash mismatch for ${rowId}`);
      assertOrigin(row.origin_locale, bundle, fallbacks, `row ${rowId}`);
    }
  }
  for (const [termId, termValue] of Object.entries(bundle.terms)) {
    assertStableId(termId, "term ID");
    assertObjectKeys(termValue, ["facets", "forms"], [], `term ${termId}`);
    assertDictionary(termValue.facets, `term ${termId} facets`);
    assertDictionary(termValue.forms, `term ${termId} forms`);
    if (sourceBundle && ownValue(termValue.forms, "$default") === undefined) throw new TroxDeserializeError("trox.bundle-term", `source term ${termId} lacks $default`);
    for (const [facet, value] of Object.entries(termValue.facets)) { assertStableId(facet, "term facet ID"); assertStableId(value, "term facet value"); }
    for (const [formId, form] of Object.entries(termValue.forms)) {
      if (formId !== "$default") assertStableId(formId, "term form ID");
      if (form.kind === "scalar") { assertObjectKeys(form, ["kind", "origin_locale", "text"], [], `term form ${termId}.${formId}`); if (typeof form.text !== "string" || form.text.length === 0) throw new TroxDeserializeError("trox.bundle-term", `term form ${termId}.${formId} text must be a nonempty string`); assertNfcForDeserialize(form.text, `term form ${termId}.${formId} text`); assertOrigin(form.origin_locale, bundle, fallbacks, `term form ${termId}.${formId}`); }
      else if (form.kind === "number") {
        if (formId === "$default") throw new TroxDeserializeError("trox.bundle-term", `term ${termId} has a numbered $default form`);
        assertObjectKeys(form, ["kind", "values"], [], `term form ${termId}.${formId}`);
        assertDictionary(form.values, `term form ${termId}.${formId} values`);
        if (Object.keys(form.values).length === 0) throw new TroxDeserializeError("trox.bundle-term", `numbered term form ${termId}.${formId} is empty`);
        if (sourceBundle && form.values.other === undefined) throw new TroxDeserializeError("trox.bundle-term", `source numbered term form ${termId}.${formId} lacks other`);
        for (const [category, surface] of Object.entries(form.values)) if (surface !== undefined) { if (!Object.hasOwn(bundle.plural_rules.cardinal, category)) throw new TroxDeserializeError("trox.bundle-term", `term form ${termId}.${formId} uses unsupported category ${category}`); assertObjectKeys(surface, ["origin_locale", "text"], [], `term surface ${termId}.${formId}`); if (typeof surface.text !== "string" || surface.text.length === 0) throw new TroxDeserializeError("trox.bundle-term", `term surface ${termId}.${formId} text must be a nonempty string`); assertNfcForDeserialize(surface.text, `term surface ${termId}.${formId} text`); assertOrigin(surface.origin_locale, bundle, fallbacks, `term surface ${termId}.${formId}`); }
      } else throw new TroxDeserializeError("trox.bundle-term", `unknown term form kind for ${termId}.${formId}`);
    }
  }
}

function validateArgumentSchemas(
  schemas: unknown,
  pattern: Pattern,
  entryId: string,
): asserts schemas is Readonly<Record<string, ArgumentSchema>> {
  assertDictionary(schemas, `argument schemas for ${entryId}`);
  const declared = collectPatternPlaceholders(pattern);
  const actual = Object.keys(schemas).sort();
  if (canonicalJson(declared) !== canonicalJson(actual)) {
    throw new TroxDeserializeError("trox.argument-schema", `source entry ${entryId} argument schemas differ from its placeholders`);
  }
  for (const [name, rawSchema] of Object.entries(schemas)) {
    assertDictionary(rawSchema, `argument schema ${entryId}.${name}`);
    if (rawSchema.kind === "scalar" || rawSchema.kind === "opaque") {
      assertObjectKeys(rawSchema, ["kind"], [], `argument schema ${entryId}.${name}`);
    } else if (rawSchema.kind === "term") {
      assertObjectKeys(rawSchema, ["kind", "number"], ["form"], `argument schema ${entryId}.${name}`);
      if (typeof rawSchema.number !== "boolean") throw new TroxDeserializeError("trox.argument-schema", `term argument schema ${entryId}.${name} number must be boolean`);
      if (rawSchema.form !== undefined) assertStableId(rawSchema.form, "term form");
    } else {
      throw new TroxDeserializeError("trox.argument-schema", `unknown argument schema kind for ${entryId}.${name}`);
    }
  }
}

function isCanonicalShortId(value: string, prefix: string): boolean {
  if (typeof value !== "string" || !value.startsWith(prefix)) return false;
  return /^[a-z2-7]{25}[aeimquy4]$/.test(value.slice(prefix.length));
}

function assertLocale(locale: string): void {
  if (typeof locale !== "string") throw new TroxDeserializeError("trox.locale", `invalid locale ${String(locale)}`);
  try {
    if (Intl.getCanonicalLocales(locale)[0] !== locale) throw new Error("noncanonical");
  } catch {
    throw new TroxDeserializeError("trox.locale", `invalid or noncanonical locale ${locale}`);
  }
}

function assertOrigin(origin: string, bundle: Bundle, fallbacks: ReadonlySet<string>, label: string): void {
  assertLocale(origin);
  if (origin !== bundle.locale && origin !== bundle.source_locale && !fallbacks.has(origin)) throw new TroxDeserializeError("trox.origin-locale", `${label} has origin outside fallback_chain`);
}

function validatePluralRules(label: string, rules: Partial<Record<PluralCategory, string>>): void {
  if (rules === null || typeof rules !== "object" || Array.isArray(rules)) throw new TroxDeserializeError("trox.plural-rules", `${label} rules must be an object`);
  assertObjectKeys(rules as Record<string, unknown>, ["other"], CATEGORIES.slice(0, -1), `${label} plural rules`);
  if (rules.other !== "") throw new TroxDeserializeError("trox.plural-rules", `${label} other rule must be empty`);
  for (const category of CATEGORIES.slice(0, -1)) { const rule = rules[category]; if (rule !== undefined && compilePluralCondition(rule) === undefined) throw new TroxDeserializeError("trox.plural-rules", `invalid ${label} rule for ${category}`); }
}

function validateExpansionPathShape(path: readonly unknown[]): void {
  let facetsStarted = false;
  const seenFacets = new Set<string>();
  for (const raw of path) {
    assertDictionary(raw, "expansion step");
    if (raw.kind === "select") {
      if (facetsStarted) throw new TroxDeserializeError("trox.expansion", "selector step follows a facet step");
      assertObjectKeys(raw, ["branch", "kind"], [], "select expansion");
      if (!Number.isSafeInteger(raw.branch) || (raw.branch as number) < 0) throw new TroxDeserializeError("trox.expansion", "invalid select expansion branch");
      continue;
    }
    if (raw.kind === "plural" || raw.kind === "ordinal") {
      if (facetsStarted) throw new TroxDeserializeError("trox.expansion", "selector step follows a facet step");
      assertObjectKeys(raw, ["branch", "kind", "match"], [], "numeric expansion");
      if (!Number.isSafeInteger(raw.branch) || (raw.branch as number) < 0) throw new TroxDeserializeError("trox.expansion", "invalid numeric expansion branch");
      assertDictionary(raw.match, "numeric expansion match");
      if (Object.hasOwn(raw.match, "exact")) {
        assertObjectKeys(raw.match, ["exact"], [], "exact expansion match");
        assertSelectorInteger(raw.match.exact as number);
      } else {
        assertObjectKeys(raw.match, ["category"], [], "category expansion match");
        if (!CATEGORIES.includes(raw.match.category as PluralCategory)) throw new TroxDeserializeError("trox.expansion", "invalid plural category");
      }
      continue;
    }
    if (raw.kind === "facet") {
      facetsStarted = true;
      assertObjectKeys(raw, ["argument", "facet", "kind", "value"], [], "facet expansion");
      if (typeof raw.argument !== "string" || !PLACEHOLDER.test(raw.argument) || raw.argument.length > 64) throw new TroxDeserializeError("trox.expansion", "invalid facet argument");
      assertStableId(raw.facet, "facet ID");
      assertStableId(raw.value, "facet value");
      const key = `${raw.argument}\0${raw.facet}`;
      if (seenFacets.has(key)) throw new TroxDeserializeError("trox.expansion", "facet steps are duplicate");
      seenFacets.add(key);
      continue;
    }
    throw new TroxDeserializeError("trox.expansion", "unknown expansion step kind");
  }
}

function expansionPathKey(path: readonly unknown[]): string {
  return path.map(expansionStepKey).join("|");
}

function expansionStepKey(raw: unknown): string {
  const step = raw as Record<string, unknown>;
  if (step.kind === "select") return `s:${String(step.branch)}`;
  if (step.kind === "plural" || step.kind === "ordinal") {
    const match = step.match as Record<string, unknown>;
    return Object.hasOwn(match, "exact")
      ? `${step.kind === "plural" ? "p" : "o"}:e:${String(step.branch)}:${String(match.exact)}`
      : `${step.kind === "plural" ? "p" : "o"}:c:${String(step.branch)}:${String(match.category)}`;
  }
  return `f:${String(step.argument)}:${String(step.facet)}:${String(step.value)}`;
}

function validatePatternShape(pattern: Pattern): void {
  assertDictionary(pattern, "pattern");
  if (pattern.kind !== "text" && pattern.kind !== "plural" && pattern.kind !== "ordinal" && pattern.kind !== "select") {
    throw new TroxDeserializeError("trox.pattern", "unknown pattern kind");
  }
  if (pattern.kind === "text") { assertObjectKeys(pattern as unknown as Record<string, unknown>, ["kind", "text"], [], "text pattern"); return; }
  assertObjectKeys(pattern as unknown as Record<string, unknown>, ["branches", "kind"], [], `${pattern.kind} pattern`);
  if (!Array.isArray(pattern.branches)) throw new TroxDeserializeError("trox.pattern", `${pattern.kind} branches must be an array`);
  for (const branch of pattern.branches) {
    assertDictionary(branch, `${pattern.kind} branch`);
    if (pattern.kind === "select") {
      assertObjectKeys(branch as unknown as Record<string, unknown>, ["pattern", "role"], [], "select branch");
    } else {
      assertObjectKeys(branch as unknown as Record<string, unknown>, ["key", "pattern"], [], "numeric branch");
      const key = (branch as unknown as NumericIdentityBranch).key as unknown as Record<string, unknown>; const keyName = pattern.kind === "plural" ? "plural" : "ordinal";
      if ("exact" in key) assertObjectKeys(key, ["exact"], [], "exact branch key");
      else assertObjectKeys(key, [keyName], [], `${pattern.kind} branch key`);
    }
    validatePatternShape((branch as unknown as { readonly pattern: Pattern }).pattern);
  }
}

function validateEntryCompatibility(target: Bundle, source: Bundle): void {
  for (const [entryId, targetEntry] of Object.entries(target.entries)) {
    const sourceEntry = source.entries[entryId];
    if (sourceEntry?.identity === undefined || sourceEntry.source_signature !== targetEntry.source_signature) continue;
    const declared = new Set(collectPatternPlaceholders(sourceEntry.identity.pattern));
    for (const row of Object.values(targetEntry.rows)) {
      validateMessageExpansion(sourceEntry.identity.pattern, row.expansion.path, target);
      const translated = parsePlaceholders(row.translation);
      if (translated.some((name) => !declared.has(name))) throw new TroxDeserializeError("trox.unknown-placeholder", `translation for ${entryId} contains an unknown placeholder`);
    }
  }
}

function validateMessageExpansion(identity: Pattern, rawPath: readonly unknown[], bundle: Bundle): void {
  const declared = new Set(collectPatternPlaceholders(identity));
  let pattern = identity; let index = 0;
  while (pattern.kind !== "text") {
    const raw = rawPath[index];
    if (raw === null || typeof raw !== "object" || Array.isArray(raw)) throw new TroxDeserializeError("trox.expansion", "message expansion omits selector step");
    const step = raw as Record<string, unknown>;
    if (pattern.kind === "select") {
      assertObjectKeys(step, ["branch", "kind"], [], "select expansion");
      if (step.kind !== "select" || !Number.isSafeInteger(step.branch) || (step.branch as number) < 0 || (step.branch as number) >= pattern.branches.length) throw new TroxDeserializeError("trox.expansion", "invalid select expansion branch");
      pattern = pattern.branches[step.branch as number]!.pattern; index += 1; continue;
    }
    assertObjectKeys(step, ["branch", "kind", "match"], [], "numeric expansion");
    if (step.kind !== pattern.kind || !Number.isSafeInteger(step.branch) || (step.branch as number) < 0 || (step.branch as number) >= pattern.branches.length) throw new TroxDeserializeError("trox.expansion", "invalid numeric expansion branch");
    const branchIndex = step.branch as number; const matcher = step.match;
    if (matcher === null || typeof matcher !== "object" || Array.isArray(matcher)) throw new TroxDeserializeError("trox.expansion", "numeric expansion lacks match");
    const match = matcher as Record<string, unknown>;
    if ("exact" in match) {
      assertObjectKeys(match, ["exact"], [], "exact match");
      const branch = pattern.branches[branchIndex]!;
      if (!("exact" in branch.key) || branch.key.exact !== match.exact) throw new TroxDeserializeError("trox.expansion", "exact expansion points at wrong source branch");
    } else {
      assertObjectKeys(match, ["category"], [], "category match");
      const category = match.category;
      if (!CATEGORIES.includes(category as PluralCategory)) throw new TroxDeserializeError("trox.expansion", "invalid plural category");
      const rules = pattern.kind === "ordinal" ? bundle.plural_rules.ordinal : bundle.plural_rules.cardinal;
      if (!(category as string in rules)) throw new TroxDeserializeError("trox.expansion", `bundle row uses unsupported category ${String(category)}`);
      let expected = pattern.branches.findIndex((branch) => ("plural" in branch.key ? branch.key.plural : "ordinal" in branch.key ? branch.key.ordinal : undefined) === category);
      if (expected < 0) expected = pattern.branches.findIndex((branch) => ("plural" in branch.key ? branch.key.plural : "ordinal" in branch.key ? branch.key.ordinal : undefined) === "other");
      if (expected !== branchIndex) throw new TroxDeserializeError("trox.expansion", "category expansion points at wrong source branch");
    }
    pattern = pattern.branches[branchIndex]!.pattern; index += 1;
  }
  const facetOrder = new Map(bundle.message_facets.map((facet, order) => [facet, order])); let prior: readonly [string, number] | undefined; const seen = new Set<string>();
  for (const raw of rawPath.slice(index)) {
    if (raw === null || typeof raw !== "object" || Array.isArray(raw)) throw new TroxDeserializeError("trox.expansion", "facet step is not an object");
    const step = raw as Record<string, unknown>; assertObjectKeys(step, ["argument", "facet", "kind", "value"], [], "facet expansion");
    if (step.kind !== "facet" || typeof step.argument !== "string" || typeof step.facet !== "string" || typeof step.value !== "string") throw new TroxDeserializeError("trox.expansion", "malformed facet step");
    if (!declared.has(step.argument)) throw new TroxDeserializeError("trox.expansion", `facet expansion references undeclared argument ${step.argument}`);
    const order = facetOrder.get(step.facet); if (order === undefined) throw new TroxDeserializeError("trox.expansion", `facet ${step.facet} is not message-scoped`);
    if (!Object.values(bundle.terms).some((termValue) => ownValue(termValue.facets, step.facet as string) === step.value)) throw new TroxDeserializeError("trox.expansion", "facet expansion value is not reachable from a bundled term");
    const current: readonly [string, number] = [step.argument, order]; const key = `${step.argument}\0${step.facet}`;
    if (seen.has(key) || (prior !== undefined && (prior[0] > current[0] || (prior[0] === current[0] && prior[1] >= current[1])))) throw new TroxDeserializeError("trox.expansion", "facet steps are duplicate or noncanonical");
    seen.add(key); prior = current;
  }
}

export function bundleFromCanonicalJSON(input: string): Bundle {
  return deserializeBoundary(() => {
    const parsed = parseCanonicalJson(input, "bundle");
    validateBundle(parsed as Bundle);
    return deepFreeze(parsed as Bundle);
  });
}

function validateWireArguments(wire: LocalizedStringWire): void {
  const expected = collectPatternPlaceholders(wire.identity.pattern); const actual = Object.keys(wire.arguments).sort();
  if (actual.length > 256) throw new TroxDeserializeError("trox.argument-limit", "message exceeds 256 arguments");
  if (canonicalJson(expected) !== canonicalJson(actual)) throw new TroxDeserializeError("trox.argument-mismatch", "wire arguments do not match identity placeholders");
  for (const [name, argument] of Object.entries(wire.arguments)) {
    if (!PLACEHOLDER.test(name) || name.length > 64) throw new TroxDeserializeError("trox.invalid-placeholder", `invalid wire argument ${name}`);
    assertDictionary(argument, `argument ${name}`);
    switch (argument.kind) {
      case "number": assertObjectKeys(argument as unknown as Record<string, unknown>, ["kind", "value"], [], `number argument ${name}`); if (!Number.isFinite(argument.value)) throw new TroxDeserializeError("trox.invalid-number", "wire number is nonfinite"); break;
      case "term":
        assertObjectKeys(argument as unknown as Record<string, unknown>, ["kind", "term_id"], ["form", "number"], `term argument ${name}`);
        assertStableId(argument.term_id, "term ID"); if (argument.form !== undefined) assertStableId(argument.form, "term form");
        if (argument.number !== undefined) assertSelectorInteger(argument.number); break;
      case "opaque": assertObjectKeys(argument as unknown as Record<string, unknown>, ["kind", "value"], [], `opaque argument ${name}`); if (argument.value.identity.pattern.kind !== "text" || Object.keys(argument.value.arguments).length !== 0 || argument.value.selectors.length !== 0) throw new TroxDeserializeError("trox.non-atomic-opaque", "wire opaque value is not atomic"); break;
      case "text": assertObjectKeys(argument as unknown as Record<string, unknown>, ["kind", "value"], [], `text argument ${name}`); assertNfc(argument.value, "wire text"); break;
      case "boolean": assertObjectKeys(argument as unknown as Record<string, unknown>, ["kind", "value"], [], `boolean argument ${name}`); if (typeof argument.value !== "boolean") throw new TroxDeserializeError("trox.invalid-argument", `boolean argument ${name} must contain a boolean`); break;
      default: throw new TroxDeserializeError("trox.argument-kind", "unknown wire argument kind");
    }
  }
  for (const record of wire.selectors) {
    assertDictionary(record, "selector record");
    if (record.kind === "select") assertObjectKeys(record as unknown as Record<string, unknown>, ["branch_keys", "kind", "path", "value"], [], "select record");
    else assertObjectKeys(record as unknown as Record<string, unknown>, ["kind", "path", "value"], [], `${record.kind} record`);
  }
  try {
    validateArgumentMap(wire.identity.pattern, wire.arguments);
  } catch (error) {
    if (error instanceof TroxValueError) throw new TroxDeserializeError(error.code, error.message.slice(error.code.length + 2));
    throw error;
  }
}

const SELECTOR_INDEX = new WeakMap<LocalizedString, ReadonlyMap<string, SelectorRecord>>();

function selectorIndex(value: LocalizedString): ReadonlyMap<string, SelectorRecord> {
  let records = SELECTOR_INDEX.get(value);
  if (records === undefined) {
    records = new Map(value.selectors.map((record) => [record.path.join(","), record]));
    SELECTOR_INDEX.set(value, records);
  }
  return records;
}

function selectPattern(bundle: CompiledBundle, value: LocalizedString): { text: string; path: unknown[]; pathKey: string } {
  const records = selectorIndex(value); const path: unknown[] = []; const pathKey: string[] = [];
  const walk = (pattern: Pattern, structural: number[]): string => {
    if (pattern.kind === "text") return pattern.text;
    const record = records.get(structural.join(","));
    if (pattern.kind === "select") {
      if (record?.kind !== "select") throw new TroxResolveError("trox.selector-record", "select record kind mismatch");
      const index = record.branch_keys.findIndex((key) => key === record.value); const selected = index < 0 ? pattern.branches.length - 1 : index;
      const step = { branch: selected, kind: "select" }; path.push(step); pathKey.push(expansionStepKey(step)); return walk(pattern.branches[selected]!.pattern, [...structural, selected]);
    }
    if (record?.kind !== pattern.kind) throw new TroxResolveError("trox.selector-record", "numeric selector record kind mismatch");
    const exact = pattern.branches.findIndex((branch) => "exact" in branch.key && branch.key.exact === record.value);
    const category = pluralCategory(bundle, pattern.kind === "ordinal", record.value);
    let selected = exact >= 0 ? exact : pattern.branches.findIndex((branch) => ("plural" in branch.key ? branch.key.plural : "ordinal" in branch.key ? branch.key.ordinal : undefined) === category);
    if (selected < 0) selected = pattern.branches.findIndex((branch) => ("plural" in branch.key ? branch.key.plural : "ordinal" in branch.key ? branch.key.ordinal : undefined) === "other");
    if (selected < 0) throw new TroxResolveError("trox.selector-fallback", "numeric selector lacks fallback");
    const step = { branch: selected, kind: pattern.kind, match: exact >= 0 ? { exact: record.value } : { category } }; path.push(step); pathKey.push(expansionStepKey(step));
    return walk(pattern.branches[selected]!.pattern, [...structural, selected]);
  };
  const text = walk(value.identity.pattern, []);
  for (const argumentName of Object.keys(value.arguments).sort()) {
    const argument = value.arguments[argumentName]!;
    if (argument.kind !== "term") continue;
    const termValue = ownValue(bundle.wire.terms, argument.term_id); if (termValue === undefined) continue;
    for (const facet of bundle.wire.message_facets) {
      const facetValue = ownValue(termValue.facets, facet);
      if (facetValue !== undefined) { const step = { argument: argumentName, facet, kind: "facet", value: facetValue }; path.push(step); pathKey.push(expansionStepKey(step)); }
    }
  }
  return { text, path, pathKey: pathKey.join("|") };
}

function pluralCategory(bundle: CompiledBundle, ordinal: boolean, value: number): PluralCategory {
  return (ordinal ? bundle.ordinal : bundle.cardinal).category(value);
}
