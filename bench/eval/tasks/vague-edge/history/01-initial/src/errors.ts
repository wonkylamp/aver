export class InvalidCursorError extends Error {
  constructor(message = 'cursor is malformed or was not issued by this API') {
    super(message);
    this.name = 'InvalidCursorError';
  }
}

export class InvalidParameterError extends Error {
  readonly parameter: string;

  constructor(parameter: string, message: string) {
    super(message);
    this.name = 'InvalidParameterError';
    this.parameter = parameter;
  }
}
