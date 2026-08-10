import { canonicalJson } from "./canonical-json.js";
import { TroxDeserializeError, TroxValueError } from "./errors.js";

export function parseCanonicalJson(input: string, label: string): unknown {
  let parsed: unknown;
  try {
    parsed = JSON.parse(input);
  } catch (error) {
    const detail = error instanceof Error ? error.message : String(error);
    throw new TroxDeserializeError("trox.invalid-json", `${label} is not valid JSON: ${detail}`);
  }
  if (canonicalJson(parsed) !== input) {
    throw new TroxDeserializeError("trox.noncanonical-json", `${label} must use canonical JSON`);
  }
  return parsed;
}

export function deserializeBoundary<T>(operation: () => T): T {
  try {
    return operation();
  } catch (error) {
    if (error instanceof TroxDeserializeError) throw error;
    if (error instanceof TroxValueError) {
      const prefix = `${error.code}: `;
      const detail = error.message.startsWith(prefix) ? error.message.slice(prefix.length) : error.message;
      throw new TroxDeserializeError(error.code, detail);
    }
    const detail = error instanceof Error ? error.message : String(error);
    throw new TroxDeserializeError("trox.deserialize", detail);
  }
}

export function assertDictionary(value: unknown, label: string): asserts value is Record<string, unknown> {
  if (value === null || typeof value !== "object" || Array.isArray(value)) {
    throw new TroxDeserializeError("trox.invalid-shape", `${label} must be an object`);
  }
}

export function assertObjectKeys(
  record: unknown,
  required: readonly string[],
  optional: readonly string[],
  label: string,
): asserts record is Record<string, unknown> {
  assertDictionary(record, label);
  const allowed = new Set([...required, ...optional]);
  for (const key of Object.keys(record)) {
    if (!allowed.has(key)) {
      throw new TroxDeserializeError("trox.unknown-field", `${label} has unknown field ${key}`);
    }
  }
  for (const key of required) {
    if (!Object.hasOwn(record, key)) {
      throw new TroxDeserializeError("trox.missing-field", `${label} lacks required field ${key}`);
    }
  }
}

export function ownValue<T>(record: Readonly<Record<string, T>>, key: string): T | undefined {
  return Object.hasOwn(record, key) ? record[key] : undefined;
}
