import { TroxValueError } from "./errors.js";

export function assertWellFormedUnicode(value: string, label: string): void {
  for (let index = 0; index < value.length; index += 1) {
    const unit = value.charCodeAt(index);
    if (unit >= 0xd800 && unit <= 0xdbff) {
      const next = value.charCodeAt(index + 1);
      if (!(next >= 0xdc00 && next <= 0xdfff)) {
        throw new TroxValueError("trox.invalid-unicode", `${label} contains an unpaired surrogate`);
      }
      index += 1;
    } else if (unit >= 0xdc00 && unit <= 0xdfff) {
      throw new TroxValueError("trox.invalid-unicode", `${label} contains an unpaired surrogate`);
    }
  }
}
