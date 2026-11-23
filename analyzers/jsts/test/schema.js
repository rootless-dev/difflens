// A small JSON Schema checker for the protocol's documents in tests, without a dependency: the keywords the
// schemas of analyzers/protocol/ use (type, const, enum, required, properties, additionalProperties, items,
// prefixItems, minItems, maxItems, minimum, minLength, $ref to #/$defs/...).

import { readFileSync } from "node:fs";
import { join } from "node:path";

import { ROOT } from "./helpers.js";

/** The parsed schema analyzers/protocol/<name>. */
export function protocolSchema(name) {
  return JSON.parse(readFileSync(join(ROOT, "..", "protocol", name), "utf8"));
}

const TYPES = {
  object: (v) => v !== null && typeof v === "object" && !Array.isArray(v),
  array: Array.isArray,
  string: (v) => typeof v === "string",
  integer: Number.isInteger,
  number: (v) => typeof v === "number",
  boolean: (v) => typeof v === "boolean",
  null: (v) => v === null,
};

/** The problems of `value` against `schema` (a sub-schema of `root`), as "path: why" lines; [] when it matches. */
export function validate(value, schema, root = schema, path = "$") {
  if (schema.$ref !== undefined) {
    const name = schema.$ref.replace(/^#\/\$defs\//, "");
    if (root.$defs?.[name] === undefined) return [`${path}: unknown $ref ${schema.$ref}`];
    return validate(value, root.$defs[name], root, path);
  }
  const problems = [];
  if (schema.type !== undefined) {
    const types = [schema.type].flat();
    if (!types.some((type) => TYPES[type]?.(value))) return [`${path}: not ${types.join(" or ")}`];
  }
  if (Object.hasOwn(schema, "const") && value !== schema.const) problems.push(`${path}: not ${JSON.stringify(schema.const)}`);
  if (schema.enum !== undefined && !schema.enum.includes(value)) problems.push(`${path}: not one of ${schema.enum.join(", ")}`);
  if (schema.minimum !== undefined && typeof value === "number" && value < schema.minimum) problems.push(`${path}: below ${schema.minimum}`);
  if (schema.minLength !== undefined && typeof value === "string" && [...value].length < schema.minLength) problems.push(`${path}: shorter than ${schema.minLength}`);
  if (TYPES.object(value)) {
    for (const key of schema.required ?? []) if (!Object.hasOwn(value, key)) problems.push(`${path}: no ${key}`);
    for (const [key, item] of Object.entries(value)) {
      const property = schema.properties?.[key];
      if (property !== undefined) problems.push(...validate(item, property, root, `${path}.${key}`));
      else if (schema.additionalProperties === false) problems.push(`${path}: unknown field ${key}`);
    }
  }
  if (Array.isArray(value)) {
    if (schema.minItems !== undefined && value.length < schema.minItems) problems.push(`${path}: fewer than ${schema.minItems} items`);
    if (schema.maxItems !== undefined && value.length > schema.maxItems) problems.push(`${path}: more than ${schema.maxItems} items`);
    value.forEach((item, i) => {
      const itemSchema = schema.prefixItems?.[i] ?? schema.items;
      if (itemSchema !== undefined) problems.push(...validate(item, itemSchema, root, `${path}[${i}]`));
    });
  }
  return problems;
}
