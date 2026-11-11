"""Small, standard-library-only Workspace wire validator and codecs.

Validation is structural, not authorization, CAS, persistence or dispatch.
Only explicitly implemented operations may be advertised by an adapter.
"""
import copy
import hashlib
import json
import math
from pathlib import Path
import re

_DIRECTORY = Path(__file__).resolve().parent
SCHEMA = json.loads((_DIRECTORY / "schema.json").read_text(encoding="utf-8"))
PROTOCOL = SCHEMA["x-wire-version"]
LIMITS = SCHEMA["x-limits"]
OPERATIONS = tuple(SCHEMA["x-operations"])
MAPPING_VERSION = SCHEMA["x-mapping-version"]
SCHEMA_FINGERPRINT = hashlib.sha256(
    json.dumps(SCHEMA, sort_keys=True, ensure_ascii=False,
               separators=(",", ":"), allow_nan=False).encode("utf-8")
).hexdigest()


class ContractError(ValueError):
    """A bounded, value-free wire error."""


def _json_value(value, depth=0):
    if depth > 64:
        raise ContractError("nesting_limit")
    if isinstance(value, dict):
        for key, item in value.items():
            if not isinstance(key, str):
                raise ContractError("invalid_json")
            _json_value(item, depth + 1)
    elif isinstance(value, list):
        for item in value:
            _json_value(item, depth + 1)
    elif value is not None and type(value) not in (str, int, float, bool):
        raise ContractError("invalid_json")


def _json_bytes(value):
    _json_value(value)
    try:
        return json.dumps(value, ensure_ascii=False, separators=(",", ":"),
                          allow_nan=False).encode("utf-8")
    except (ValueError, TypeError, UnicodeError, RecursionError) as error:
        raise ContractError("invalid_json") from error


def _validate(spec, value, path="$", depth=0):
    if depth > 64:
        raise ContractError("nesting_limit")
    if "$ref" in spec:
        return _validate(SCHEMA["$defs"][spec["$ref"].rsplit("/", 1)[1]],
                         value, path, depth + 1)
    for union in ("oneOf", "anyOf"):
        if union in spec:
            matches = 0
            for branch in spec[union]:
                try:
                    _validate(branch, value, path, depth + 1)
                    matches += 1
                except ContractError:
                    pass
            if matches < 1 or (union == "oneOf" and matches != 1):
                raise ContractError(path + ":union")
            return
    if "const" in spec:
        constant = spec["const"]
        numeric = type(value) in (int, float) and type(constant) in (int, float)
        if value != constant or (not numeric and type(value) is not type(constant)):
            raise ContractError(path + ":const")
    if "enum" in spec and value not in spec["enum"]:
        raise ContractError(path + ":enum")
    kind = spec.get("type")
    valid = {"object": lambda: isinstance(value, dict),
             "array": lambda: isinstance(value, list),
             "string": lambda: isinstance(value, str),
             "integer": lambda: type(value) is int or (type(value) is float and math.isfinite(value) and value.is_integer()),
             "boolean": lambda: type(value) is bool,
             "null": lambda: value is None}
    if kind and not valid[kind]():
        raise ContractError(path + ":type")
    if kind == "object":
        if not all(isinstance(key, str) for key in value):
            raise ContractError(path + ":key")
        if any(key not in value for key in spec.get("required", [])):
            raise ContractError(path + ":required")
        properties = spec.get("properties", {})
        if spec.get("additionalProperties") is False and value.keys() - properties.keys():
            raise ContractError(path + ":unknown_field")
        for key, item in value.items():
            if key in properties:
                _validate(properties[key], item, path + "." + key, depth + 1)
    elif kind == "array":
        if not spec.get("minItems", 0) <= len(value) <= spec.get("maxItems", float("inf")):
            raise ContractError(path + ":items")
        if spec.get("uniqueItems") and len({_json_bytes(item) for item in value}) != len(value):
            raise ContractError(path + ":unique")
        for item in value:
            _validate(spec["items"], item, path + "[]", depth + 1)
    elif kind == "string":
        if not spec.get("minLength", 0) <= len(value) <= spec.get("maxLength", float("inf")):
            raise ContractError(path + ":length")
        if "pattern" in spec and re.fullmatch(spec["pattern"], value, flags=re.ASCII) is None:
            raise ContractError(path + ":pattern")
    elif kind == "integer":
        if not spec.get("minimum", -float("inf")) <= value <= spec.get("maximum", float("inf")):
            raise ContractError(path + ":range")


def validate(type_name, value):
    if type_name not in SCHEMA["$defs"]:
        raise ContractError("unknown_type")
    data = _json_bytes(value)
    if len(data) > LIMITS["frameBytes"]:
        raise ContractError("frame_too_large")
    spec = SCHEMA["$defs"][type_name]
    if spec.get("x-page") and len(data) > LIMITS["pageContentBytes"]:
        raise ContractError("page_too_large")
    _validate(spec, value)
    return value


def _normalize(spec, value, warnings, path="$"):
    if "$ref" in spec:
        return _normalize(SCHEMA["$defs"][spec["$ref"].rsplit("/", 1)[1]],
                          value, warnings, path)
    for union in ("oneOf", "anyOf"):
        if union in spec:
            for branch in spec[union]:
                try:
                    _validate(branch, value)
                except ContractError:
                    continue
                return _normalize(branch, value, warnings, path)
    if "x-known-values" in spec and value not in spec["x-known-values"]:
        warnings.append(path + ":unknown_enum")
        return "unknown"
    if spec.get("type") == "object":
        return {key: _normalize(spec["properties"][key], item, warnings, path + "." + key)
                if key in spec.get("properties", {}) else item for key, item in value.items()}
    if spec.get("type") == "array":
        return [_normalize(spec["items"], item, warnings, path + "[]") for item in value]
    return value


def decode_response(type_name, value):
    validate(type_name, value)
    warnings = []
    decoded = _normalize(SCHEMA["$defs"][type_name], copy.deepcopy(value), warnings)
    return decoded, warnings


def capabilities(*, build_identity, generation, operations, acp_capabilities=None):
    if set(operations) - set(OPERATIONS):
        raise ContractError("unsupported_operation")
    result = {"protocol": dict(PROTOCOL), "buildIdentity": build_identity,
              "schemaFingerprint": SCHEMA_FINGERPRINT, "mappingVersion": MAPPING_VERSION,
              "operations": sorted(set(operations)), "limits": dict(LIMITS),
              "generation": generation, "acpCapabilities": acp_capabilities or {}}
    return validate("Capabilities", result)


def negotiate(local, remote):
    validate("Capabilities", local)
    validate("Capabilities", remote)
    if local["protocol"]["major"] != remote["protocol"]["major"]:
        raise ContractError("version_mismatch")
    return {"protocol": {"major": local["protocol"]["major"],
                         "minor": min(local["protocol"]["minor"], remote["protocol"]["minor"])},
            "operations": sorted(set(local["operations"]) & set(remote["operations"])),
            "limits": {key: min(local["limits"][key], remote["limits"][key]) for key in LIMITS
                       if key in local["limits"] and key in remote["limits"]},
            "mappingVersion": local["mappingVersion"] if local["mappingVersion"] == remote["mappingVersion"] else None}


def encode_frame(value):
    data = _json_bytes(value)
    if not data or len(data) > LIMITS["frameBytes"]:
        raise ContractError("frame_too_large")
    return len(data).to_bytes(4, "big") + data


def decode_frame(frame):
    if len(frame) < 4:
        raise ContractError("incomplete_frame")
    size = int.from_bytes(frame[:4], "big")
    if size == 0 or size > LIMITS["frameBytes"]:
        raise ContractError("frame_too_large")
    if size != len(frame) - 4:
        raise ContractError("incomplete_frame")
    try:
        value = json.loads(frame[4:].decode("utf-8"),
                           parse_constant=lambda _: (_ for _ in ()).throw(ValueError()))
        _json_bytes(value)
        return value
    except (ValueError, UnicodeError, RecursionError) as error:
        raise ContractError("invalid_json") from error
