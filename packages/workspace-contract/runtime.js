// Runtime validation is separate from TypeScript's compile-time types.
import schema from "./schema.json" with { type: "json" };
import { PROTOCOL, SCHEMA_FINGERPRINT, MAPPING_VERSION, LIMITS, OPERATIONS } from "./index.js";

function jsonValue(value, depth = 0) {
  if (depth > 64) fail("nesting_limit");
  if (value === null || typeof value === "boolean") return;
  if (typeof value === "number" && Number.isFinite(value)) return;
  if (typeof value === "string") {
    if ([...value].some(char => char.codePointAt(0) >= 0xd800 && char.codePointAt(0) <= 0xdfff)) fail("invalid_json");
    return;
  }
  if (Array.isArray(value)) { value.forEach(item => jsonValue(item, depth + 1)); return; }
  if (typeof value === "object" && Object.getPrototypeOf(value) === Object.prototype) {
    Object.entries(value).forEach(([key, item]) => { jsonValue(key, depth + 1); jsonValue(item, depth + 1); }); return;
  }
  fail("invalid_json");
}
const bytes = value => { jsonValue(value); return new TextEncoder().encode(JSON.stringify(value)); };
const fail = code => { throw new Error(code); };
function check(spec, value, path = "$", depth = 0) {
  if (depth > 64) fail("nesting_limit");
  if (spec.$ref) return check(schema.$defs[spec.$ref.split("/").at(-1)], value, path, depth + 1);
  for (const union of ["oneOf", "anyOf"]) {
    if (spec[union]) {
      let matches = 0;
      for (const branch of spec[union]) {
        try { check(branch, value, path, depth + 1); matches++; } catch {}
      }
      if (!matches || (union === "oneOf" && matches !== 1)) fail(path + ":union");
      return;
    }
  }
  if ("const" in spec && value !== spec.const) fail(path + ":const");
  if (spec.enum && !spec.enum.includes(value)) fail(path + ":enum");
  const kind = spec.type;
  const valid = {
    object: value !== null && typeof value === "object" && !Array.isArray(value),
    array: Array.isArray(value), string: typeof value === "string",
    integer: Number.isSafeInteger(value), boolean: typeof value === "boolean", null: value === null
  };
  if (kind && !valid[kind]) fail(path + ":type");
  if (kind === "object") {
    const properties = spec.properties ?? {};
    if ((spec.required ?? []).some(key => !Object.hasOwn(value, key))) fail(path + ":required");
    for (const [key, item] of Object.entries(value)) {
      if (Object.hasOwn(properties, key)) check(properties[key], item, path + "." + key, depth + 1);
      else if (spec.additionalProperties === false) fail(path + ":unknown_field");
    }
  } else if (kind === "array") {
    if (value.length < (spec.minItems ?? 0) || value.length > (spec.maxItems ?? Infinity)) fail(path + ":items");
    if (spec.uniqueItems && new Set(value.map(v => JSON.stringify(v))).size !== value.length) fail(path + ":unique");
    value.forEach(item => check(spec.items, item, path + "[]", depth + 1));
  } else if (kind === "string") {
    const length = [...value].length;
    if (length < (spec.minLength ?? 0) || length > (spec.maxLength ?? Infinity)) fail(path + ":length");
    if (spec.pattern && value.match(new RegExp(spec.pattern))?.[0] !== value) fail(path + ":pattern");
  } else if (kind === "integer") {
    if (value < (spec.minimum ?? -Infinity) || value > (spec.maximum ?? Infinity)) fail(path + ":range");
  }
}
export function validate(type, value) {
  if (!Object.hasOwn(schema.$defs, type)) fail("unknown_type");
  const size = bytes(value).length;
  if (size > LIMITS.frameBytes) fail("frame_too_large");
  if (schema.$defs[type]["x-page"] && size > LIMITS.pageContentBytes) fail("page_too_large");
  check(schema.$defs[type], value);
  return value;
}
function normalize(spec, value, warnings, path = "$") {
  if (spec.$ref) return normalize(schema.$defs[spec.$ref.split("/").at(-1)], value, warnings, path);
  for (const union of ["oneOf", "anyOf"]) {
    if (spec[union]) {
      for (const branch of spec[union]) {
        try { check(branch, value); } catch { continue; }
        return normalize(branch, value, warnings, path);
      }
    }
  }
  if (spec["x-known-values"] && !spec["x-known-values"].includes(value)) {
    warnings.push(path + ":unknown_enum");
    return "unknown";
  }
  if (spec.type === "object") return Object.fromEntries(Object.entries(value).map(([key, item]) =>
    [key, Object.hasOwn(spec.properties ?? {}, key) ? normalize(spec.properties[key], item, warnings, path + "." + key) : item]));
  if (spec.type === "array") return value.map(item => normalize(spec.items, item, warnings, path + "[]"));
  return value;
}
export function decodeResponse(type, value) {
  validate(type, value);
  const warnings = [];
  return { value: normalize(schema.$defs[type], structuredClone(value), warnings), warnings };
}
export function capabilities(buildIdentity, generation, operations, acpCapabilities = {}) {
  if (operations.some(op => !OPERATIONS.includes(op))) fail("unsupported_operation");
  return validate("Capabilities", { protocol: { ...PROTOCOL }, buildIdentity, schemaFingerprint: SCHEMA_FINGERPRINT,
    mappingVersion: MAPPING_VERSION, operations: [...new Set(operations)].sort(), limits: { ...LIMITS }, generation, acpCapabilities });
}
export function negotiate(local, remote) {
  validate("Capabilities", local); validate("Capabilities", remote);
  if (local.protocol.major !== remote.protocol.major) fail("version_mismatch");
  return { protocol: { major: local.protocol.major, minor: Math.min(local.protocol.minor, remote.protocol.minor) },
    operations: [...new Set(local.operations.filter(op => remote.operations.includes(op)))].sort(),
    limits: Object.fromEntries(Object.keys(LIMITS).map(key => [key, Math.min(local.limits[key], remote.limits[key])])),
    mappingVersion: local.mappingVersion === remote.mappingVersion ? local.mappingVersion : null };
}
export function encodeFrame(value) {
  const data = bytes(value);
  if (!data.length || data.length > LIMITS.frameBytes) fail("frame_too_large");
  const frame = new Uint8Array(data.length + 4);
  new DataView(frame.buffer).setUint32(0, data.length);
  frame.set(data, 4);
  return frame;
}
export function decodeFrame(frame) {
  if (frame.length < 4) fail("incomplete_frame");
  const size = new DataView(frame.buffer, frame.byteOffset, 4).getUint32(0);
  if (!size || size > LIMITS.frameBytes) fail("frame_too_large");
  if (size !== frame.length - 4) fail("incomplete_frame");
  const value = JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(frame.subarray(4)));
  jsonValue(value);
  return value;
}
