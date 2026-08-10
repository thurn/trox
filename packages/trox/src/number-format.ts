import { TroxValueError } from "./errors.js";

export interface NumberFormat {
  readonly decimal: string;
  readonly digits: string;
  readonly exponent: string;
  readonly group: string;
  readonly grouping: readonly [number, number];
  readonly minimum_grouping_digits: number;
  readonly minus: string;
  readonly plus: string;
}

export function formatNumber(value: number, format: NumberFormat): string {
  if (!Number.isFinite(value)) {
    throw new TroxValueError("trox.invalid-number", "number must be finite");
  }
  const ascii = JSON.stringify(Object.is(value, -0) ? 0 : value);
  const exponentIndex = ascii.search(/[eE]/);
  const mantissa = exponentIndex < 0 ? ascii : ascii.slice(0, exponentIndex);
  const exponentPart = exponentIndex < 0 ? undefined : ascii.slice(exponentIndex + 1);
  const negative = mantissa.startsWith("-");
  const [integer = "0", fraction] = mantissa.replace(/^-/, "").split(".");
  const grouped = exponentPart === undefined ? groupDigits(integer, format) : integer;
  let output = `${negative ? format.minus : ""}${mapDigits(grouped, format)}`;
  if (fraction !== undefined) output += `${format.decimal}${mapDigits(fraction, format)}`;
  if (exponentPart !== undefined) {
    output += format.exponent;
    let exponent = exponentPart;
    if (exponent.startsWith("+")) {
      output += format.plus;
      exponent = exponent.slice(1);
    } else if (exponent.startsWith("-")) {
      output += format.minus;
      exponent = exponent.slice(1);
    }
    output += mapDigits(exponent, format);
  }
  return output;
}

function groupDigits(integer: string, format: NumberFormat): string {
  const [primary, secondary] = format.grouping;
  if (primary === 0 || integer.length < primary + format.minimum_grouping_digits) return integer;
  const groups: string[] = [];
  let end = integer.length;
  let width = primary;
  while (end > width) {
    groups.push(integer.slice(end - width, end));
    end -= width;
    width = Math.max(1, secondary);
  }
  groups.push(integer.slice(0, end));
  return groups.reverse().join(format.group);
}

function mapDigits(value: string, format: NumberFormat): string {
  const digits = [...format.digits];
  return value.replace(/[0-9]/g, (digit) => digits[Number(digit)]!);
}
