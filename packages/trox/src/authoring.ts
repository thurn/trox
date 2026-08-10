import { blake3 } from "@noble/hashes/blake3.js";
import { base32, canonicalJson, comparePaths, deepFreeze, hex, sortRecord } from "./canonical-json.js";
import { TroxValueError } from "./errors.js";
import { assertWellFormedUnicode } from "./unicode.js";

export const MAX_SAFE_SELECTOR_INTEGER = 9_007_199_254_740_991;
const STABLE_ID = /^[a-z][a-z0-9]*(?:[.-][a-z0-9]+)*$/;
export const PLACEHOLDER = /^[a-z][a-z0-9]*(?:_[a-z0-9]+)*$/;
export const CATEGORIES = ["zero", "one", "two", "few", "many", "other"] as const;
export type PluralCategory = (typeof CATEGORIES)[number];
export type SelectorKey = string | boolean;

export function assertStableId(value: unknown, label: string): asserts value is string {
  if (typeof value !== "string" || new TextEncoder().encode(value).length > 96 || !STABLE_ID.test(value)) {
    throw new TroxValueError("trox.invalid-stable-id", `invalid ${label} \`${String(value)}\``);
  }
}

export function assertSelectorInteger(value: number): void {
  if (!Number.isSafeInteger(value) || value < 0 || value > MAX_SAFE_SELECTOR_INTEGER) {
    throw new TroxValueError(
      "trox.invalid-selector-number",
      "selector values must be nonnegative safe integers",
    );
  }
}

export function assertNfc(value: string, label: string): void {
  if (typeof value !== "string") {
    throw new TroxValueError("trox.invalid-string", `${label} must be a string`);
  }
  assertWellFormedUnicode(value, label);
  if (value.normalize("NFC") !== value) {
    throw new TroxValueError("trox.non-nfc", `${label} must be NFC`);
  }
}

export interface TextPattern { readonly kind: "text"; readonly text: string }
export interface ExactKey { readonly exact: number }
export interface PluralKey { readonly plural: PluralCategory }
export interface OrdinalKey { readonly ordinal: PluralCategory }
export type NumericKey = ExactKey | PluralKey | OrdinalKey;
export interface NumericIdentityBranch { readonly key: NumericKey; readonly pattern: Pattern }
export interface PluralPattern { readonly kind: "plural"; readonly branches: readonly NumericIdentityBranch[] }
export interface OrdinalPattern { readonly kind: "ordinal"; readonly branches: readonly NumericIdentityBranch[] }
export interface WhenIdentityBranch { readonly role: "when"; readonly pattern: Pattern }
export interface OtherwiseIdentityBranch { readonly role: "otherwise"; readonly pattern: Pattern }
export interface SelectPattern { readonly kind: "select"; readonly branches: readonly (WhenIdentityBranch | OtherwiseIdentityBranch)[] }
export type Pattern = TextPattern | PluralPattern | OrdinalPattern | SelectPattern;

export interface IdentityDescriptor {
  readonly identity_version: 1;
  readonly meaning: string | null;
  readonly pattern: Pattern;
}

export interface PluralSelectorRecord { readonly kind: "plural"; readonly path: readonly number[]; readonly value: number }
export interface OrdinalSelectorRecord { readonly kind: "ordinal"; readonly path: readonly number[]; readonly value: number }
export interface SelectSelectorRecord {
  readonly branch_keys: readonly SelectorKey[];
  readonly kind: "select";
  readonly path: readonly number[];
  readonly value: SelectorKey;
}
export type SelectorRecord = PluralSelectorRecord | OrdinalSelectorRecord | SelectSelectorRecord;

interface PatternValue {
  readonly pattern: Pattern;
  readonly selectors: readonly SelectorRecord[];
  readonly meaning: string | null;
}

type PatternInput = string | PatternValue;

function patternValue(input: PatternInput): PatternValue {
  if (typeof input === "string") {
    assertNfc(input, "source text");
    parsePlaceholders(input);
    return { pattern: { kind: "text", text: input }, selectors: [], meaning: null };
  }
  return input;
}

type NumericArmKey = { readonly exact: number } | { readonly category: PluralCategory };
interface NumericArm { readonly key: NumericArmKey; readonly value: PatternValue }

export function exact(value: number, pattern: PatternInput): NumericArm {
  assertSelectorInteger(value);
  const armValue = patternValue(pattern);
  assertNoNestedMeaning(armValue);
  return { key: { exact: value }, value: armValue };
}

function categoryArm(category: PluralCategory, pattern: PatternInput): NumericArm {
  const armValue = patternValue(pattern);
  assertNoNestedMeaning(armValue);
  return { key: { category }, value: armValue };
}
export const zero = (pattern: PatternInput): NumericArm => categoryArm("zero", pattern);
export const one = (pattern: PatternInput): NumericArm => categoryArm("one", pattern);
export const two = (pattern: PatternInput): NumericArm => categoryArm("two", pattern);
export const few = (pattern: PatternInput): NumericArm => categoryArm("few", pattern);
export const many = (pattern: PatternInput): NumericArm => categoryArm("many", pattern);
export const other = (pattern: PatternInput): NumericArm => categoryArm("other", pattern);

export function plural(value: number, branches: readonly NumericArm[]): PatternValue {
  return numericPattern("plural", value, branches);
}

export function ordinal(value: number, branches: readonly NumericArm[]): PatternValue {
  return numericPattern("ordinal", value, branches);
}

function numericPattern(kind: "plural" | "ordinal", value: number, branches: readonly NumericArm[]): PatternValue {
  assertSelectorInteger(value);
  if (branches.length === 0 || branches.length > 256) {
    throw new TroxValueError("trox.branch-limit", "numeric selector must have 1..=256 branches");
  }
  let prior: readonly [number, number] | undefined;
  let hasOther = false;
  const selectors: SelectorRecord[] = [];
  const identityBranches: NumericIdentityBranch[] = [];
  branches.forEach((arm, index) => {
    const order: readonly [number, number] = "exact" in arm.key
      ? [0, arm.key.exact]
      : [1, CATEGORIES.indexOf(arm.key.category)];
    if (prior !== undefined && (order[0] < prior[0] || (order[0] === prior[0] && order[1] <= prior[1]))) {
      throw new TroxValueError("trox.branch-order", "numeric selector branches are duplicate or noncanonical");
    }
    prior = order;
    if ("category" in arm.key && arm.key.category === "other") hasOther = true;
    selectors.push(...prefixSelectors(arm.value.selectors, index));
    const key: NumericKey = "exact" in arm.key
      ? { exact: arm.key.exact }
      : kind === "plural" ? { plural: arm.key.category } : { ordinal: arm.key.category };
    identityBranches.push({ key, pattern: arm.value.pattern });
  });
  if (!hasOther) throw new TroxValueError("trox.missing-other", "numeric selector requires other");
  selectors.push({ kind, path: [], value });
  return { pattern: { kind, branches: identityBranches }, selectors, meaning: null };
}

interface WhenArm<K extends SelectorKey> { readonly role: "when"; readonly key: K; readonly value: PatternValue }
interface OtherwiseArm { readonly role: "otherwise"; readonly value: PatternValue }
type SelectArm<K extends SelectorKey = SelectorKey> = WhenArm<K> | OtherwiseArm;

export function when<K extends SelectorKey>(key: K, pattern: PatternInput): WhenArm<K> {
  if (typeof key === "string") assertStableId(key, "selector key");
  const armValue = patternValue(pattern);
  assertNoNestedMeaning(armValue);
  return { role: "when", key, value: armValue };
}

export function otherwise(pattern: PatternInput): OtherwiseArm {
  const armValue = patternValue(pattern);
  assertNoNestedMeaning(armValue);
  return { role: "otherwise", value: armValue };
}

function assertNoNestedMeaning(value: PatternValue): void {
  if (value.meaning !== null) {
    throw new TroxValueError("trox.nested-meaning", "meaning may only wrap a complete top-level pattern");
  }
}

type FiniteSelector<K extends SelectorKey> = K extends string ? string extends K ? never : K : K;

export function select<K extends SelectorKey>(
  value: FiniteSelector<K>,
  branches: readonly SelectArm<K>[],
): PatternValue {
  if (typeof value === "string") assertStableId(value, "selector key");
  if (branches.length === 0 || branches.length > 256 || branches.at(-1)?.role !== "otherwise") {
    throw new TroxValueError("trox.missing-otherwise", "select requires final otherwise");
  }
  if (branches.slice(0, -1).some((arm) => arm.role !== "when")) {
    throw new TroxValueError("trox.otherwise-order", "otherwise must appear exactly once at the end");
  }
  const keys: SelectorKey[] = [];
  const seen = new Set<SelectorKey>();
  const selectors: SelectorRecord[] = [];
  const identityBranches: (WhenIdentityBranch | OtherwiseIdentityBranch)[] = [];
  branches.forEach((arm, index) => {
    selectors.push(...prefixSelectors(arm.value.selectors, index));
    if (arm.role === "when") {
      if (typeof arm.key !== typeof value) throw new TroxValueError("trox.selector-key-type", "select keys must have one JSON type");
      if (seen.has(arm.key)) throw new TroxValueError("trox.duplicate-selector-key", "select branch keys must be unique");
      seen.add(arm.key); keys.push(arm.key);
      identityBranches.push({ role: "when", pattern: arm.value.pattern });
    } else identityBranches.push({ role: "otherwise", pattern: arm.value.pattern });
  });
  selectors.push({ branch_keys: keys, kind: "select", path: [], value });
  return { pattern: { kind: "select", branches: identityBranches }, selectors, meaning: null };
}

function prefixSelectors(records: readonly SelectorRecord[], branch: number): SelectorRecord[] {
  return records.map((record) => ({ ...record, path: [branch, ...record.path] }));
}

export function meaning(meaningId: string, pattern: PatternInput): PatternValue {
  assertStableId(meaningId, "meaning");
  const value = patternValue(pattern);
  if (value.meaning !== null) throw new TroxValueError("trox.duplicate-meaning", "duplicate meaning wrapper");
  return { ...value, meaning: meaningId };
}

export class TermId {
  readonly value: string;
  private readonly _termIdBrand = true;
  constructor(value: string) { assertStableId(value, "term ID"); this.value = value; Object.freeze(this); }
}
export function termId(value: string): TermId { return new TermId(value); }

export interface TextArgument { readonly kind: "text"; readonly value: string }
export interface NumberArgument { readonly kind: "number"; readonly value: number }
export interface BooleanArgument { readonly kind: "boolean"; readonly value: boolean }
export interface TermArgument {
  readonly kind: "term";
  readonly term_id: string;
  readonly form?: string;
  readonly number?: number;
}
export interface OpaqueArgument { readonly kind: "opaque"; readonly value: LocalizedStringWire }
export type Argument = TextArgument | NumberArgument | BooleanArgument | TermArgument | OpaqueArgument;
export type ArgumentInput = string | number | boolean | TermArgument | OpaqueArgument;

class TermBuilder {
  readonly id: TermId;
  readonly formId: string | undefined;
  readonly numberValue: number | undefined;
  constructor(id: TermId, formId?: string, numberValue?: number) { this.id = id; this.formId = formId; this.numberValue = numberValue; }
  form(formId: string): TermBuilder {
    assertStableId(formId, "term form");
    if (this.formId !== undefined) throw new TroxValueError("trox.duplicate-term-form", "duplicate term form");
    return new TermBuilder(this.id, formId, this.numberValue);
  }
  number(number: number): TermArgument { assertSelectorInteger(number); return toTermArgument(new TermBuilder(this.id, this.formId, number)); }
  finish(): TermArgument { return toTermArgument(this); }
}
function toTermArgument(value: TermBuilder): TermArgument {
  return {
    kind: "term", term_id: value.id.value,
    ...(value.formId === undefined ? {} : { form: value.formId }),
    ...(value.numberValue === undefined ? {} : { number: value.numberValue }),
  };
}
export function term(id: TermId): TermBuilder { return new TermBuilder(id); }
export function indefinite(id: TermId): TermArgument { return term(id).form("indefinite").finish(); }
export function counted(id: TermId, number: number): TermArgument { return term(id).form("counted").number(number); }

export function opaque(value: LocalizedString): OpaqueArgument {
  if (!value.isAtomic()) throw new TroxValueError("trox.non-atomic-opaque", "opaque value must be atomic");
  return { kind: "opaque", value: value.wireValue() };
}

export interface LocalizedStringWire {
  readonly arguments: Readonly<Record<string, Argument>>;
  readonly entry_id: string;
  readonly format: "trox-localized-string";
  readonly identity: IdentityDescriptor;
  readonly selectors: readonly SelectorRecord[];
  readonly source_signature: string;
  readonly version: { readonly major: 1; readonly minor: 0 };
}

export class LocalizedString {
  readonly #wire: LocalizedStringWire;
  private constructor(wire: LocalizedStringWire, token: symbol) {
    if (token !== CONSTRUCTION_TOKEN) throw new TroxValueError("trox.constructor", "use tx or txa to construct LocalizedString");
    this.#wire = deepFreeze(wire);
    Object.freeze(this);
  }
  /** @internal */
  static fromValidatedWire(wire: LocalizedStringWire, token: symbol): LocalizedString {
    return new LocalizedString(wire, token);
  }
  get entryId(): string { return this.#wire.entry_id; }
  get sourceSignature(): string { return this.#wire.source_signature; }
  get identity(): IdentityDescriptor { return this.#wire.identity; }
  get arguments(): Readonly<Record<string, Argument>> { return this.#wire.arguments; }
  get selectors(): readonly SelectorRecord[] { return this.#wire.selectors; }
  isAtomic(): boolean { return this.#wire.identity.pattern.kind === "text" && Object.keys(this.#wire.arguments).length === 0 && this.#wire.selectors.length === 0; }
  toCanonicalJSON(): string { return canonicalJson(this.#wire); }
  wireValue(): LocalizedStringWire { return structuredClone(this.#wire); }
  toString(): never { throw new TroxValueError("trox.explicit-resolution", "LocalizedString must be resolved by a Localizer"); }
  [Symbol.toPrimitive](): never { throw new TroxValueError("trox.explicit-resolution", "LocalizedString must be resolved by a Localizer"); }
}

/** @internal */
export const CONSTRUCTION_TOKEN = Symbol("trox-construction");

export function tx(pattern: PatternInput, description: string): LocalizedString {
  return construct(patternValue(pattern), {}, description);
}

export function txa(pattern: PatternInput, inputs: Readonly<Record<string, ArgumentInput>>, description: string): LocalizedString {
  if (inputs === null || typeof inputs !== "object" || Array.isArray(inputs)) {
    throw new TroxValueError("trox.invalid-arguments", "argument bindings must be an object");
  }
  const args: Record<string, Argument> = {};
  for (const [name, input] of Object.entries(inputs)) {
    if (!PLACEHOLDER.test(name) || name.length > 64) throw new TroxValueError("trox.invalid-placeholder", `invalid argument name \`${name}\``);
    args[name] = argumentFrom(input);
  }
  return construct(patternValue(pattern), args, description);
}

function argumentFrom(value: ArgumentInput): Argument {
  if (typeof value === "string") { assertNfc(value, "argument text"); return { kind: "text", value }; }
  if (typeof value === "number") {
    if (!Number.isFinite(value)) throw new TroxValueError("trox.invalid-number", "Trox numbers must be finite");
    return { kind: "number", value: Object.is(value, -0) ? 0 : value };
  }
  if (typeof value === "boolean") return { kind: "boolean", value };
  if (value !== null && typeof value === "object" && (value.kind === "term" || value.kind === "opaque")) {
    validateArgument(value);
    return structuredClone(value);
  }
  throw new TroxValueError("trox.invalid-argument", "unsupported argument value");
}

function construct(value: PatternValue, args: Record<string, Argument>, description: string): LocalizedString {
  assertNfc(description, "description");
  if (description.trim() === "") throw new TroxValueError("trox.description", "description must not be empty");
  validatePattern(value.pattern);
  validateArgumentMap(value.pattern, args);
  const identity: IdentityDescriptor = { identity_version: 1, meaning: value.meaning, pattern: value.pattern };
  let digest: Uint8Array | undefined;
  const identityDigest = (): Uint8Array => {
    digest ??= blake3(new TextEncoder().encode(canonicalJson(identity)));
    return digest;
  };
  const wire = {
    arguments: sortRecord(args),
    get entry_id(): string { return `tx1_${base32(identityDigest().slice(0, 16))}`; },
    format: "trox-localized-string",
    identity,
    selectors: [...value.selectors].sort((a, b) => comparePaths(a.path, b.path)),
    get source_signature(): string { return hex(identityDigest()); },
    version: { major: 1, minor: 0 },
  } satisfies LocalizedStringWire;
  validateSelectorRecords(wire.identity.pattern, wire.selectors);
  return LocalizedString.fromValidatedWire(wire, CONSTRUCTION_TOKEN);
}

export function validateArgumentMap(pattern: Pattern, argumentsValue: Readonly<Record<string, Argument>>): void {
  const actual = Object.keys(argumentsValue).sort();
  if (actual.length > 256) {
    throw new TroxValueError("trox.argument-limit", "message exceeds 256 arguments");
  }
  const expected = collectPatternPlaceholders(pattern);
  if (expected.join("\0") !== actual.join("\0")) {
    throw new TroxValueError("trox.argument-mismatch", `expected ${expected.join(", ")}; got ${actual.join(", ")}`);
  }
  for (const argument of Object.values(argumentsValue)) validateArgument(argument);
}

function validateArgument(argument: Argument): void {
  if (argument === null || typeof argument !== "object" || Array.isArray(argument)) {
    throw new TroxValueError("trox.invalid-argument", "argument must be a tagged object");
  }
  switch (argument.kind) {
    case "text":
      assertNfc(argument.value, "argument text");
      return;
    case "number":
      if (typeof argument.value !== "number" || !Number.isFinite(argument.value)) {
        throw new TroxValueError("trox.invalid-number", "Trox numbers must be finite");
      }
      return;
    case "boolean":
      if (typeof argument.value !== "boolean") {
        throw new TroxValueError("trox.invalid-argument", "boolean argument value must be boolean");
      }
      return;
    case "term":
      assertStableId(argument.term_id, "term ID");
      if (argument.form !== undefined) assertStableId(argument.form, "term form");
      if (argument.number !== undefined) assertSelectorInteger(argument.number);
      return;
    case "opaque": {
      const nested = argument.value;
      if (nested === null || typeof nested !== "object" || Array.isArray(nested)
        || nested.format !== "trox-localized-string"
        || nested.version?.major !== 1 || nested.version.minor !== 0
        || nested.identity?.pattern?.kind !== "text"
        || nested.arguments === null || typeof nested.arguments !== "object"
        || Object.keys(nested.arguments).length !== 0
        || !Array.isArray(nested.selectors) || nested.selectors.length !== 0) {
        throw new TroxValueError("trox.non-atomic-opaque", "opaque value must be atomic");
      }
      validateIdentity(nested.identity);
      const nestedDigest = blake3(new TextEncoder().encode(canonicalJson(nested.identity)));
      if (nested.entry_id !== `tx1_${base32(nestedDigest.slice(0, 16))}` || nested.source_signature !== hex(nestedDigest)) {
        throw new TroxValueError("trox.identity-mismatch", "opaque value identity hash mismatch");
      }
      return;
    }
  }
}

function validatePattern(pattern: Pattern, depth = 0, nodes = { count: 0 }): void {
  nodes.count += 1;
  if (nodes.count > 4096) throw new TroxValueError("trox.pattern-limit", "pattern exceeds 4,096 nodes");
  if (depth > 16) throw new TroxValueError("trox.selector-depth", "pattern exceeds 16 nested selectors");
  if (pattern.kind === "text") { if (typeof pattern.text !== "string") throw new TroxValueError("trox.pattern", "text pattern requires string text"); assertNfc(pattern.text, "source text"); parsePlaceholders(pattern.text); return; }
  if (!Array.isArray(pattern.branches) || pattern.branches.length === 0 || pattern.branches.length > 256) throw new TroxValueError("trox.branch-limit", "selector branch limit exceeded");
  if (pattern.kind === "select") {
    if (pattern.branches.at(-1)?.role !== "otherwise" || pattern.branches.slice(0, -1).some((branch) => branch.role !== "when")) throw new TroxValueError("trox.otherwise-order", "select requires exactly one final otherwise");
  } else {
    let prior: readonly [number, number] | undefined; let last: readonly [number, number] | undefined; let hasOther = false;
    for (const branch of pattern.branches) {
      let order: readonly [number, number];
      if ("exact" in branch.key) { assertSelectorInteger(branch.key.exact); order = [0, branch.key.exact]; }
      else {
        const category = pattern.kind === "plural" && "plural" in branch.key ? branch.key.plural : pattern.kind === "ordinal" && "ordinal" in branch.key ? branch.key.ordinal : undefined;
        if (category === undefined || !CATEGORIES.includes(category)) throw new TroxValueError("trox.branch-kind", "numeric branch key differs from selector kind");
        order = [1, CATEGORIES.indexOf(category)]; hasOther ||= category === "other";
      }
      prior = last; if (prior !== undefined && (order[0] < prior[0] || (order[0] === prior[0] && order[1] <= prior[1]))) throw new TroxValueError("trox.branch-order", "numeric branches are duplicate or noncanonical"); last = order;
    }
    if (!hasOther) throw new TroxValueError("trox.missing-other", "numeric selector requires other");
  }
  for (const branch of pattern.branches) validatePattern(branch.pattern, depth + 1, nodes);
}

export function validateIdentity(identity: IdentityDescriptor): void {
  if (identity.identity_version !== 1) throw new TroxValueError("trox.identity-version", "identity version must be 1");
  if (identity.meaning !== null) { if (typeof identity.meaning !== "string") throw new TroxValueError("trox.meaning", "meaning must be a stable ID or null"); assertStableId(identity.meaning, "meaning"); }
  validatePattern(identity.pattern);
}

export function parsePlaceholders(text: string): string[] {
  const names = new Set<string>();
  for (let index = 0; index < text.length;) {
    if (text.startsWith("{{", index) || text.startsWith("}}", index)) { index += 2; continue; }
    if (text[index] === "{") {
      const end = text.indexOf("}", index + 1);
      if (end < 0) throw new TroxValueError("trox.invalid-braces", "unclosed `{`");
      const name = text.slice(index + 1, end);
      if (name.length > 64 || !PLACEHOLDER.test(name)) throw new TroxValueError("trox.invalid-placeholder", `invalid placeholder \`{${name}}\``);
      names.add(name); index = end + 1; continue;
    }
    if (text[index] === "}") throw new TroxValueError("trox.invalid-braces", "unmatched `}`");
    index += 1;
  }
  return [...names].sort();
}

export function collectPatternPlaceholders(pattern: Pattern): string[] {
  const names = new Set<string>();
  const visit = (node: Pattern): void => {
    if (node.kind === "text") for (const name of parsePlaceholders(node.text)) names.add(name);
    else for (const branch of node.branches) visit(branch.pattern);
  };
  visit(pattern); return [...names].sort();
}

export function validateSelectorRecords(pattern: Pattern, records: readonly SelectorRecord[]): void {
  if (!Array.isArray(records)) throw new TroxValueError("trox.selector-records", "selector records must be an array");
  const expected: { readonly path: number[]; readonly kind: Pattern["kind"]; readonly selectBranches: number }[] = [];
  const visit = (node: Pattern, path: number[]): void => {
    if (node.kind === "text") return;
    expected.push({ path, kind: node.kind, selectBranches: node.kind === "select" ? node.branches.length - 1 : 0 });
    node.branches.forEach((branch, index) => visit(branch.pattern, [...path, index]));
  };
  visit(pattern, []); expected.sort((left, right) => comparePaths(left.path, right.path));
  if (records.length !== expected.length) throw new TroxValueError("trox.selector-records", "selector record count differs from pattern nodes");
  records.forEach((record, index) => {
    const wanted = expected[index]!;
    if (record.kind !== wanted.kind || canonicalJson(record.path) !== canonicalJson(wanted.path)) throw new TroxValueError("trox.selector-records", "selector records do not match canonical pattern order");
    if (record.kind === "select") {
      if (!Array.isArray(record.branch_keys) || (typeof record.value !== "string" && typeof record.value !== "boolean")) {
        throw new TroxValueError("trox.selector-records", "select keys and value must be strings or booleans");
      }
      if (record.branch_keys.length !== wanted.selectBranches || record.branch_keys.some((key: unknown) => typeof key !== typeof record.value)) throw new TroxValueError("trox.selector-records", "select branch keys do not match selector value");
      const seen = new Set<SelectorKey>(); for (const key of record.branch_keys) { if (typeof key !== "string" && typeof key !== "boolean") throw new TroxValueError("trox.selector-records", "select keys must be strings or booleans"); if (typeof key === "string") assertStableId(key, "selector key"); if (seen.has(key)) throw new TroxValueError("trox.duplicate-selector-key", "select branch keys must be unique"); seen.add(key); } if (typeof record.value === "string") assertStableId(record.value, "selector value");
    } else assertSelectorInteger(record.value);
  });
}
