export class TroxValueError extends Error {
  readonly code: string;

  constructor(code: string, message: string) {
    super(`${code}: ${message}`);
    this.name = "TroxValueError";
    this.code = code;
  }
}

export class TroxDeserializeError extends Error {
  readonly code: string;

  constructor(code: string, message: string) {
    super(`${code}: ${message}`);
    this.name = "TroxDeserializeError";
    this.code = code;
  }
}

export class TroxResolveError extends Error {
  readonly code: string;

  constructor(code: string, message: string) {
    super(`${code}: ${message}`);
    this.name = "TroxResolveError";
    this.code = code;
  }
}
