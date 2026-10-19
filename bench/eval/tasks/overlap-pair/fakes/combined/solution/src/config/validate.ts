import { schema, hasType } from "./schema.ts";
import type { FieldSpec, FieldType } from "./schema.ts";

export interface ConfigProblem {
  /** Dotted path of the bad value: "server.port", a section name, or "(root)". */
  path: string;
  message: string;
}

const sections: Record<string, Record<string, FieldSpec>> = schema;

// Every problem in a parsed config file, in file order. An empty list means
// the file is valid.
export function validate(raw: unknown): ConfigProblem[] {
  if (!isRecord(raw)) return [{ path: "(root)", message: `expected an object, got ${describe(raw)}` }];
  const problems: ConfigProblem[] = [];
  for (const [section, given] of Object.entries(raw)) {
    const fields = Object.hasOwn(sections, section) ? sections[section] : undefined;
    if (fields === undefined) {
      problems.push({ path: section, message: "unknown section" });
      continue;
    }
    if (!isRecord(given)) {
      problems.push({ path: section, message: `expected an object, got ${describe(given)}` });
      continue;
    }
    for (const [key, value] of Object.entries(given)) {
      const path = `${section}.${key}`;
      const spec = Object.hasOwn(fields, key) ? fields[key] : undefined;
      if (spec === undefined) {
        problems.push({ path, message: "unknown key" });
      } else if (!hasType(spec.type, value)) {
        problems.push({ path, message: `expected ${typeName(spec.type)}, got ${describe(value)}` });
      }
    }
  }
  return problems;
}

export function formatProblems(problems: ConfigProblem[]): string {
  return `invalid config: ${problems.map((p) => `${p.path}: ${p.message}`).join("; ")}`;
}

export function typeName(type: FieldType): string {
  switch (type) {
    case "string":
      return "a string";
    case "number":
      return "a number";
    case "boolean":
      return "true or false";
    case "list":
      return "a list of strings";
  }
}

function describe(value: unknown): string {
  if (value === null) return "null";
  if (Array.isArray(value)) return "an array";
  if (typeof value === "string") return `string ${JSON.stringify(value)}`;
  if (typeof value === "object") return "an object";
  return `${typeof value} ${String(value)}`;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}
