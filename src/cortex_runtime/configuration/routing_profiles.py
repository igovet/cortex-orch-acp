"""Owner profiles: snapshots of every user-level setting, materialized into
the files the runtime already reads.

A profile governs up to four sections (:data:`PROFILE_SECTIONS`):

- ``routing`` -- the eight supported keys of the user ``routing.json``;
- ``user_defaults`` -- ``defaults.json``: the route default, its parameters,
  the language and the owner ceiling;
- ``mcp_servers`` -- the usage policy of each ``mcp-servers.json`` server
  (``enabled`` and its tool/path/network/role/engine limits). ``command``,
  ``args`` and ``env`` are an installation definition that may carry
  credentials: they are never captured, stored or written from a profile;
- ``connections`` -- ``max_concurrent_sessions`` of each registered
  connection.

Profiles are a management surface, not another runtime source: the loaders
keep reading the backing files with their existing precedence. Saving is
explicit (``save`` into the selected profile, ``create`` as a new one); an
edit anywhere only makes the selected profile ``modified``. Activation
validates every target first, journals ``pending`` in the profiles file,
replaces the changed backing files in a fixed order and rolls back in
process on failure; an unfinished activation reads as ``interrupted``.
Project-level settings (project routing, project ceiling) are never part of
a profile.
"""
from __future__ import annotations

from dataclasses import replace
from datetime import datetime, timezone
import hashlib
import json
import os
import re
import stat
from typing import Mapping

from ..connections import registry
from ..connections.model import MAX_CONCURRENT_SESSIONS_LIMIT, SUPPORTED_ENGINES
from ..portable.errors import PortableError
from . import management as m
from . import store as configuration_store
from .policy import PolicyCeiling

FORMAT = 2
PROFILE_SECTIONS = ("routing", "user_defaults", "mcp_servers", "connections")
#: Server keys a profile never stores: installation definition and secrets.
MCP_DEFINITION_KEYS = ("command", "args", "env")
PROFILE_MCP_KEYS = tuple(key for key in m.MCP_SERVER_KEYS if key not in MCP_DEFINITION_KEYS)
STATES = ("none", "in_sync", "modified", "interrupted", "unavailable")

_NAME = re.compile(r"[a-z0-9][a-z0-9-]{0,39}\Z")
_MAX_PROFILES = 32
_MAX_DIFFERENCES = 200
_SECTION_LABELS = dict(routing="routing (user)", user_defaults="user_defaults", mcp_servers="mcp_servers",
                       connections="connections")
_DEFAULT_GROUPS = (("route", ("coordinator", *m.ROUTE_DEFAULT_KEYS)), ("parameters", ("parameters",)), ("language", ("language",)))
_UNRESTRICTED = m.ceiling_to_dict(PolicyCeiling())


def _paths(environ):
    root = registry.config_root(environ)
    return root / "routing-profiles.json", root / "routing.json"


def _section_path(section, environ):
    root = registry.config_root(environ)
    return dict(routing=root / "routing.json", user_defaults=root / "defaults.json",
                mcp_servers=root / "mcp-servers.json", connections=registry.registry_path(environ))[section]


def _read_file(path):
    try:
        info = path.lstat()
    except FileNotFoundError:
        return None
    if (not stat.S_ISREG(info.st_mode) or info.st_nlink != 1 or info.st_uid != os.getuid()
            or info.st_mode & 0o077):
        raise m._error("unsafe_storage", "Routing profiles must be an owner-only, single-link regular file.")
    return m._read_plain(path)


# --- section validation, capture and comparison ------------------------------------

def _prefixed(error, field):
    """``error`` (a validation refusal) with every field under ``field``."""
    if isinstance(error, m.ConfigValidationError):
        items = []
        for item in error.errors:
            where = item["field"]
            where = field + where[len("rules"):] if where == "rules" or where.startswith("rules.") else f"{field}.{where}"
            items.append(dict(item, field=where))
        return m.ConfigValidationError(items)
    return m.ConfigValidationError([dict(field=field, code=error.code, message=error.remediation)])


def _normalize_section(section, value):
    """The canonical form of one section of a profile (stored or captured);
    raises :class:`management.ConfigValidationError` naming every problem."""
    field = f"settings.{section}"
    errors = m._Errors()
    if section == "routing":
        try:
            known = {k: v for k, v in value.items() if k in m.ROUTING_KEYS} if isinstance(value, Mapping) else value
            m.validate_routing_rules(known, require_policy=False)
            return dict(value)
        except PortableError as error:
            raise _prefixed(error, field) from None
    if section == "user_defaults":
        if not isinstance(value, Mapping) or set(value) != {"defaults", "ceiling"} \
                or not isinstance(value.get("defaults"), Mapping):
            errors.add(field, "invalid_type", "must be an object with a 'defaults' object and a 'ceiling'")
            errors.raise_if_any()
        defaults = dict(value["defaults"])
        patch = m._Errors()
        m._validate_default_patch(defaults, patch)
        for item in patch.items:
            errors.add(f"{field}.{item['field']}", item["code"], item["message"])
        for key, item in defaults.items():
            if item is None:
                errors.add(f"{field}.defaults.{key}", "invalid_value", "an unset default is omitted, never null")
        if defaults.get("coordinator") is not None and any(k in defaults for k in m.ROUTE_DEFAULT_KEYS):
            errors.add(field, "route_conflict", "portable coordinator and bound route are mutually exclusive")
        present = [key for key in m.ROUTE_DEFAULT_KEYS if key in defaults]
        if present and len(present) != len(m.ROUTE_DEFAULT_KEYS):
            errors.add(f"{field}.defaults", "route_incomplete",
                       "route_id, route_connection_id and route_model are stored together")
        ceiling = None
        try:
            ceiling = m.parse_ceiling(value["ceiling"], field=f"{field}.ceiling")
        except m.ConfigValidationError as error:
            errors.absorb(error)
        errors.raise_if_any()
        return dict(defaults=defaults, ceiling=m.ceiling_to_dict(ceiling))
    if section == "mcp_servers":
        if not isinstance(value, Mapping):
            errors.add(field, "invalid_type", "must map a server name to its usage policy")
            errors.raise_if_any()
        for name, policy in value.items():
            where = f"{field}.{name}"
            if not isinstance(name, str) or not m._SERVER_NAME.fullmatch(name) or name == "cortex":
                errors.add(where, "invalid_server_name", "must be a configured server name other than 'cortex'")
            elif not isinstance(policy, Mapping):
                errors.add(where, "invalid_type", "must be an object")
            else:
                for key in sorted(policy, key=str):
                    if key in MCP_DEFINITION_KEYS:
                        errors.add(f"{where}.{key}", "definition_not_allowed",
                                   "command, args and env stay in mcp-servers.json; a profile never stores them")
                    elif key not in PROFILE_MCP_KEYS:
                        errors.add(f"{where}.{key}", "unsupported_setting", f"use one of {', '.join(PROFILE_MCP_KEYS)}")
                if type(policy.get("enabled")) is not bool:
                    errors.add(f"{where}.enabled", "invalid_type", "must be true or false")
        errors.raise_if_any()
        return {name: dict(policy) for name, policy in value.items()}
    if section == "connections":
        if not isinstance(value, Mapping):
            errors.add(field, "invalid_type", "must map a connection id to its session limit")
            errors.raise_if_any()
        for connection_id, item in value.items():
            where = f"{field}.{connection_id}"
            if not isinstance(connection_id, str) or not m._CONNECTION_ID.fullmatch(connection_id):
                errors.add(where, "invalid_connection_id", "must be cn_<12 hex>")
            elif (not isinstance(item, Mapping) or set(item) != {"max_concurrent_sessions"}
                  or not m._non_negative_int(item["max_concurrent_sessions"])
                  or not 1 <= item["max_concurrent_sessions"] <= MAX_CONCURRENT_SESSIONS_LIMIT):
                errors.add(where, "invalid_value", "must be {\"max_concurrent_sessions\": 1-"
                                                   f"{MAX_CONCURRENT_SESSIONS_LIMIT}}}")
        errors.raise_if_any()
        return {connection_id: dict(item) for connection_id, item in value.items()}
    raise m._error("invalid_profiles", "Unknown profile section.", field="sections")


def _routing_rules(data):
    if data is None:
        return {}
    value = m._json_object(data, "routing.json", "invalid_routing_overrides")
    try:
        from ..routing import auto_route
        auto_route._validate(value, require_policy=False)
    except ValueError:
        raise m._error("invalid_routing_overrides", "The user routing.json is invalid; repair it first.") from None
    return {k: value[k] for k in m.ROUTING_KEYS if k in value}


def _capture(section, data):
    """The working state of ``section`` in profile form, from its file bytes."""
    if section == "routing":
        return _routing_rules(data)
    if section == "user_defaults":
        defaults, ceiling, _ = m._parse_user_defaults(data)
        captured = {key: defaults[key] for key in ("coordinator", "route_id", "route_connection_id", "route_model", "language")
                    if defaults.get(key) is not None}
        # The ACP picker writes {} for "no parameters"; both mean unset.
        if defaults.get("parameters"):
            captured["parameters"] = defaults["parameters"]
        return dict(defaults=captured, ceiling=m.ceiling_to_dict(ceiling))
    if section == "mcp_servers":
        document, key = m._mcp_document(data)
        captured = {}
        for name, spec in document[key].items():
            if not isinstance(spec, Mapping):
                raise m._error("invalid_mcp_registry", "mcp-servers.json has a server entry that is not an object.")
            policy = {item: spec[item] for item in PROFILE_MCP_KEYS if item in spec and item != "enabled"}
            policy["enabled"] = spec.get("enabled", True) is not False
            captured[name] = policy
        return captured
    state = registry._parse_registry_bytes(data or b"")
    return {connection_id: dict(max_concurrent_sessions=record.max_concurrent_sessions)
            for connection_id, record in sorted(state.connections.items())}


def _read_section(section, path):
    return m._read_plain(path) if section == "routing" else registry.read_owner_file(path)


def _snapshot(environ):
    """Every section's path, bytes, revision and capture (or why not)."""
    snapshot = {}
    for section in PROFILE_SECTIONS:
        item = dict(path=_section_path(section, environ), data=None, revision=None,
                    read_error=None, capture=None, capture_error=None)
        try:
            item["data"] = _read_section(section, item["path"])
            item["revision"] = m._revision(item["data"])
        except PortableError as error:
            item["read_error"] = item["capture_error"] = error
            snapshot[section] = item
            continue
        try:
            item["capture"] = _normalize_section(section, _capture(section, item["data"]))
        except PortableError as error:
            item["capture_error"] = error
        snapshot[section] = item
    return snapshot


def _compare(section, saved, current):
    """``(differing keys, notes)`` of a saved section against the working
    state: key names only, never values."""
    notes = []
    if section == "routing":
        keys = sorted(key for key in set(saved) | set(current) if saved.get(key) != current.get(key))
    elif section == "user_defaults":
        keys = [group for group, members in _DEFAULT_GROUPS
                if any(saved["defaults"].get(key) != current["defaults"].get(key) for key in members)]
        if saved["ceiling"] != current["ceiling"]:
            keys.append("ceiling")
    elif section == "mcp_servers":
        keys = sorted([name for name, policy in saved.items() if current.get(name) != policy]
                      + [name for name, policy in current.items() if name not in saved and policy.get("enabled")])
    else:
        keys = sorted(connection_id for connection_id, item in saved.items()
                      if connection_id in current and current[connection_id] != item)
        notes = [f"connection_missing:{connection_id}" for connection_id in sorted(saved) if connection_id not in current]
    return keys, notes


def _problem(section, error):
    return dict(section=section, code=error.code, message=error.remediation)


def _status(doc, snapshot):
    """The selected profile's state against the working settings."""
    name = doc.get("active")
    pending = doc.get("pending")
    problems = [_problem(section, snapshot[section]["capture_error"]) for section in PROFILE_SECTIONS
                if snapshot[section]["capture_error"] is not None]
    modified, differences, notes, unavailable = [], [], [], False
    if name is not None:
        entry = doc["profiles"][name]
        for section in entry["sections"]:
            item = snapshot[section]
            if item["capture_error"] is not None:
                unavailable = True
                continue
            keys, section_notes = _compare(section, entry["settings"][section], item["capture"])
            notes.extend(section_notes)
            if keys:
                modified.append(section)
                differences.extend(dict(section=section, key=key) for key in keys)
    if pending is not None:
        state = "interrupted"
    elif name is None:
        state = "none"
    elif unavailable:
        state = "unavailable"
    else:
        state = "modified" if modified else "in_sync"
    return dict(state=state, modified_sections=modified, differences=differences[:_MAX_DIFFERENCES],
                problems=problems, notes=notes)


def _matches(entry, snapshot):
    for section in entry["sections"]:
        item = snapshot[section]
        if item["capture_error"] is not None or _compare(section, entry["settings"][section], item["capture"])[0]:
            return False
    return True


# --- the profiles document ------------------------------------------------------

def _invalid(message, field=None):
    return m._error("invalid_profiles", message, field=field)


def _check_description(value):
    return isinstance(value, str) and len(value) <= 256 and not m._CONTROL.search(value)


def _check_profiles_map(value):
    profiles = value.get("profiles")
    if (not isinstance(profiles, dict) or len(profiles) > _MAX_PROFILES
            or not all(isinstance(k, str) and _NAME.fullmatch(k) and isinstance(v, dict) for k, v in profiles.items())
            or value.get("active") is not None and value.get("active") not in profiles
            or value.get("applied") is not None and not isinstance(value.get("applied"), dict)):
        raise _invalid("routing-profiles.json has invalid profile metadata.")
    return profiles


def _document_v1(value):
    profiles = _check_profiles_map(value)
    for entry in profiles.values():
        if not isinstance(entry.get("rules"), dict) or not _check_description(entry.get("description")):
            raise _invalid("A routing profile has invalid rules or description.")
    return value


def _document_v2(value):
    """A validated format-2 document with every section in canonical form."""
    profiles = _check_profiles_map(value)
    result = {key: item for key, item in value.items() if key != "profiles"}
    result["profiles"] = {}
    for name, entry in profiles.items():
        where = f"profiles.{name}"
        sections = entry.get("sections")
        settings = entry.get("settings")
        if (not _check_description(entry.get("description"))
                or any(entry.get(key) is not None and not isinstance(entry.get(key), str)
                       for key in ("created_at", "updated_at"))
                or not isinstance(sections, list) or not sections
                or not all(isinstance(item, str) and item in PROFILE_SECTIONS for item in sections)
                or len(set(sections)) != len(sections)
                or not isinstance(settings, dict) or set(settings) != set(sections)
                or set(entry) - {"description", "created_at", "updated_at", "sections", "settings"}):
            raise _invalid("A profile has an invalid description, sections or settings.", where)
        normalized = {}
        for section in PROFILE_SECTIONS:
            if section not in sections:
                continue
            try:
                normalized[section] = _normalize_section(section, settings[section])
            except PortableError as error:
                raise _invalid("A profile holds an invalid setting; repair or delete that profile.",
                               f"{where}.{error.field or 'settings.' + section}") from None
        result["profiles"][name] = dict(entry, sections=[s for s in PROFILE_SECTIONS if s in sections],
                                        settings=normalized)
    pending = value.get("pending")
    if pending is not None and (not isinstance(pending, dict) or pending.get("operation") != "activate"
                                or pending.get("profile") not in profiles
                                or not isinstance(pending.get("at"), str)):
        raise _invalid("routing-profiles.json has an invalid pending activation.", "pending")
    result.setdefault("pending", None)
    result.setdefault("applied", None)
    result.setdefault("active", None)
    return result


def _migrate_v1(doc):
    """The format-2 form of a validated format-1 document (pure; routing only)."""
    profiles = {}
    for name, entry in doc["profiles"].items():
        migrated = dict(description=entry["description"])
        for key in ("created_at", "updated_at"):
            if isinstance(entry.get(key), str):
                migrated[key] = entry[key]
        migrated.update(sections=["routing"], settings=dict(routing=entry["rules"]))
        profiles[name] = migrated
    applied = doc.get("applied")
    if isinstance(applied, dict) and applied.get("profile") in profiles:
        applied = dict(profile=applied["profile"], at=applied.get("at"),
                       revisions=dict(routing=applied.get("routing_revision")))
    else:
        applied = None
    return dict(format=FORMAT, active=doc.get("active"), applied=applied, pending=None, profiles=profiles)


def _parse(data):
    """``(format-2 document, migrated_from)``; nothing is written."""
    if data is None:
        return dict(format=FORMAT, active=None, applied=None, pending=None, profiles={}), None
    value = m._json_object(data, "routing-profiles.json", "invalid_profiles")
    version = value.get("format")
    if type(version) is int and version == 1:
        original = _document_v1(value)
        return _document_v2(_migrate_v1(original)), 1
    if type(version) is not int or version != FORMAT:
        raise m._error("unsupported_profiles_format", "routing-profiles.json format must be 2 (or 1, which is "
                                                      "migrated on the first change).", field="format")
    return _document_v2(value), None


def _load(environ):
    path, _ = _paths(environ)
    data = _read_file(path)
    doc, migrated_from = _parse(data)
    return path, data, doc, migrated_from


def _entry_revision(entry):
    return "sha256:" + hashlib.sha256(json.dumps(entry, sort_keys=True, separators=(",", ":"),
                                             ensure_ascii=False).encode("utf-8")).hexdigest()


def _summary(entry):
    settings = entry["settings"]
    rules = settings.get("routing") or {}
    models = rules.get("models") if isinstance(rules.get("models"), dict) else {}
    deny = rules.get("deny") if isinstance(rules.get("deny"), list) else []
    user = settings.get("user_defaults")
    defaults = user["defaults"] if user else {}
    servers = settings.get("mcp_servers") or {}
    return dict(
        providers=rules.get("providers"),
        denied_engines=[engine for engine in SUPPORTED_ENGINES if f"{engine}/*" in deny],
        selected_models={engine: len(config.get("auto", [])) for engine, config in models.items()
                         if isinstance(config, dict) and isinstance(config.get("auto"), list)},
        default_model=defaults.get("route_model"), language=defaults.get("language"),
        enabled_mcp_servers=sum(1 for policy in servers.values() if policy.get("enabled") is True),
        ceiling_set=bool(user) and user["ceiling"] != _UNRESTRICTED,
    )


def _view(doc, path, data, migrated_from, snapshot, project_root, environ):
    status = _status(doc, snapshot)
    shadowed = next((label for label, source in m._routing_sources(project_root, environ)
                     if label != "user" and source.is_file()), None)
    profiles = []
    for name, entry in sorted(doc["profiles"].items()):
        profiles.append(dict(name=name, description=entry.get("description", ""), revision=_entry_revision(entry),
                             sections=list(entry["sections"]), settings=entry["settings"],
                             created_at=entry.get("created_at"), updated_at=entry.get("updated_at"),
                             summary=_summary(entry)))
    current = {section: snapshot[section]["revision"] for section in PROFILE_SECTIONS}
    return dict(surface="routing_profiles", path=str(path), revision=m._revision(data), format=FORMAT,
                migrated_from=migrated_from, active=doc.get("active"), state=status["state"],
                modified_sections=status["modified_sections"], differences=status["differences"],
                pending=doc.get("pending"), problems=status["problems"], applied=doc.get("applied"),
                current_revisions=current, profiles=profiles, user_routing_revision=current["routing"],
                shadowed_by=shadowed,
                matches_profile=[name for name, entry in sorted(doc["profiles"].items()) if _matches(entry, snapshot)],
                notes=status["notes"])


def read_profiles(project_root=None, environ=None):
    env = m._env(environ)
    path, data, doc, migrated_from = _load(env)
    return _view(doc, path, data, migrated_from, _snapshot(env), project_root, env)


# --- mutations ---------------------------------------------------------------------

def _name(name, field="name"):
    if not isinstance(name, str) or not _NAME.fullmatch(name):
        raise m._error("invalid_profile_name", "Profile name must be 1-40 lowercase letters, digits or hyphens.",
                       field=field)


def _description(value):
    if not _check_description(value):
        raise m._error("invalid_profile_description", "Description must be at most 256 printable characters.",
                       field="description")


def _expected_revisions(value):
    if (not isinstance(value, Mapping) or set(value) != set(PROFILE_SECTIONS)
            or not all(isinstance(item, str) and (item == m.ABSENT or m._REVISION.fullmatch(item))
                       for item in value.values())):
        raise m._error("invalid_expected_revisions",
                       "expected_revisions must hold the revision a read returned for each of "
                       f"{', '.join(PROFILE_SECTIONS)} ('absent' or 'sha256:<64 hex>').", field="expected_revisions")
    return dict(value)


def _check_sections(snapshot, expected):
    for section in PROFILE_SECTIONS:
        item = snapshot[section]
        if item["read_error"] is not None:
            raise item["read_error"]
        m._check(_SECTION_LABELS[section], expected[section], item["revision"])


def _capture_all(snapshot):
    captured = {}
    for section in PROFILE_SECTIONS:
        error = snapshot[section]["capture_error"]
        if error is not None:
            raise m._error("profile_section_unavailable",
                           f"The current {_SECTION_LABELS[section]} settings cannot be stored in a profile "
                           f"({error.code}); repair them first. Nothing was written.", field=f"settings.{section}")
        captured[section] = snapshot[section]["capture"]
    return captured


def _applied(name, now, revisions):
    return dict(profile=name, at=now, revisions=dict(revisions))


def _backup(path, data, environ):
    root = registry.config_root(environ)
    directory = root / ".migration-backups"
    registry.ensure_private_dir(directory, root=root, environ=environ)
    stamp = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%S%fZ")
    target = directory / f"{path.stem}.format-1.{stamp}.json"
    registry.atomic_write_private(target, data, directory=directory)
    return target


def _preserves(original, parsed, touched, active_touched):
    """The migrated document keeps every untouched format-1 profile."""
    for name, entry in original["profiles"].items():
        if name in touched:
            continue
        got = parsed["profiles"].get(name)
        if (got is None or got["settings"].get("routing") != entry["rules"]
                or got.get("description") != entry["description"]
                or got.get("created_at") != entry.get("created_at")):
            return False
    return active_touched or parsed.get("active") == original.get("active")


class _Profiles:
    """Writes the profiles document; the first write of a format-1 file is
    the verified migration (private backup, re-read check, restore)."""

    def __init__(self, path, data, migrated_from, environ, touched, active_touched):
        self.path, self.data, self.environ = path, data, environ
        self.migrating = migrated_from == 1
        self.touched, self.active_touched = touched, active_touched
        self.notes = []

    def write(self, doc):
        body = m._pretty(doc)
        if not self.migrating:
            m._replace_file(self.path, body)
            return
        original = _document_v1(m._json_object(self.data, "routing-profiles.json", "invalid_profiles"))
        _backup(self.path, self.data, self.environ)
        m._replace_file(self.path, body)
        try:
            parsed, _ = _parse(_read_file(self.path))
            verified = parsed == json.loads(body) and _preserves(original, parsed, self.touched, self.active_touched)
        except (PortableError, ValueError):
            verified = False
        if not verified:
            m._replace_file(self.path, self.data)
            raise m._error("profile_migration_failed",
                           "Migrating routing-profiles.json to format 2 could not be verified; the original file "
                           "was restored and a private backup kept in .migration-backups. Nothing was changed.")
        self.migrating = False
        self.notes.append("migrated_from_format_1")


def _build(section, settings, item, environ, now, notes):
    """The new bytes of one backing file, validated with the runtime's own
    rules; ``None`` when it already holds the profile's settings."""
    if section == "routing":
        m.validate_routing_rules({k: v for k, v in settings.items() if k in m.ROUTING_KEYS})
    data = item["data"]
    if item["capture_error"] is None and not _compare(section, settings, item["capture"])[0]:
        if section == "connections":
            notes.extend(_compare(section, settings, item["capture"])[1])
        return None
    field = f"settings.{section}"
    if section == "routing":
        try:
            rules = m.validate_routing_rules({k: v for k, v in settings.items() if k in m.ROUTING_KEYS})
        except PortableError as error:
            raise _prefixed(error, field) from None
        previous = m._json_object(data, "routing.json", "invalid_routing_overrides") if data else {}
        preserved = {key: value for key, value in previous.items() if key not in m.ROUTING_KEYS}
        saved_unknown = {k: v for k, v in settings.items() if k not in m.ROUTING_KEYS}
        return m._pretty({**rules, **preserved, **saved_unknown}), None
    if section == "user_defaults":
        current, current_ceiling, document = m._parse_user_defaults(data)
        merged = {key: value for key, value in current.items() if key not in m.USER_DEFAULT_KEYS}
        merged.update(settings["defaults"])
        if merged.get("route_connection_id") is not None \
                and merged["route_connection_id"] not in registry.inspect_registry(environ).connections:
            raise m._error("profile_reference_missing", "The profile's default route names a connection that is not "
                                                        "registered here; nothing was written.",
                           field=f"{field}.defaults.route_connection_id")
        ceiling = m.parse_ceiling(settings["ceiling"], field=f"{field}.ceiling")
        if m.ceiling_to_dict(ceiling) != m.ceiling_to_dict(current_ceiling):
            notes.append("owner_ceiling_changed")
        payload = {key: value for key, value in document.items() if key not in ("defaults", "ceiling")}
        payload.update(defaults=merged, ceiling=m.ceiling_to_dict(ceiling))
        return configuration_store._encode(payload).encode("utf-8"), None
    if section == "mcp_servers":
        document, key = m._mcp_document(data)
        if document.get("version", 1) != 1:
            raise m._error("unsupported_mcp_registry_version", "mcp-servers.json 'version' must be 1.",
                           field="version")
        missing = [name for name in settings if name not in document[key]]
        if missing:
            raise m._error("profile_mcp_server_missing", "The profile names an MCP server that is not configured "
                                                         "here; add it again or save the profile. Nothing was written.",
                           field=f"{field}.{missing[0]}")
        servers = {}
        for name, spec in document[key].items():
            if not isinstance(spec, Mapping):
                servers[name] = spec
            elif name in settings:
                kept = {item: value for item, value in spec.items() if item not in PROFILE_MCP_KEYS}
                servers[name] = {**kept, **settings[name]}
            else:
                # Never deleted: its definition and env values stay.
                servers[name] = dict(spec, enabled=False)
        document = dict(document)
        document.setdefault("version", 1)
        document[key] = servers
        try:
            from . import mcp_servers as mcp_registry
            mcp_registry._normalize_entries(document, item["path"])
        except PortableError as error:
            raise m.ConfigValidationError([dict(field=field, code=error.code, message=error.remediation)]) from None
        return m._pretty(document), None
    state = registry._parse_registry_bytes(data or b"")
    for connection_id, value in settings.items():
        record = state.connections.get(connection_id)
        if record is None:
            notes.append(f"connection_missing:{connection_id}")
        elif record.max_concurrent_sessions != value["max_concurrent_sessions"]:
            state.connections[connection_id] = replace(
                record, max_concurrent_sessions=value["max_concurrent_sessions"], updated_at=now)
    return state.to_json_bytes(), state


def _write_section(section, item, body, state, environ):
    path = item["path"]
    if section == "routing":
        m._replace_file(path, body)
    elif section == "connections":
        registry.write_registry(state, environ)
    else:
        registry.ensure_private_dir(path.parent, root=registry.config_root(environ), environ=environ)
        registry.atomic_write_private(path, body, directory=path.parent)


def _restore(section, item):
    path, data = item["path"], item["data"]
    if data is None:
        try:
            os.unlink(path)
        except FileNotFoundError:
            pass
    elif section == "routing":
        m._replace_file(path, data)
    else:
        registry.atomic_write_private(path, data, directory=path.parent)


def _activate(doc, name, snapshot, profiles, environ, now, notes):
    entry = doc["profiles"][name]
    targets = []
    for section in PROFILE_SECTIONS:
        if section not in entry["sections"]:
            continue
        item = snapshot[section]
        built = _build(section, entry["settings"][section], item, environ, now, notes)
        if built is not None:
            targets.append((section, item, built[0], built[1]))
    doc["pending"] = dict(operation="activate", profile=name, at=now)
    profiles.write(doc)
    written = []
    try:
        for section, item, body, state in targets:
            _write_section(section, item, body, state, environ)
            written.append((section, item))
    except BaseException:
        restored = True
        for section, item in reversed(written):
            try:
                _restore(section, item)
            except Exception:
                restored = False
        if restored:
            doc["pending"] = None
            try:
                profiles.write(doc)
            except Exception:
                pass
        raise
    revisions = {section: snapshot[section]["revision"] for section in PROFILE_SECTIONS}
    revisions.update({section: m._revision(body) for section, _, body, _ in targets})
    doc.update(active=name, applied=_applied(name, now, revisions), pending=None)
    profiles.write(doc)


_TOUCHED = dict(create=False, save=True, update=True, delete=True, activate=False, deactivate=False)


def mutate(action, expected_revision, *, name=None, description=None, rename_to=None, expected_revisions=None,
           discard_modifications=False, project_root=None, environ=None):
    """Revision-checked profile mutation under the owner configuration lock.

    ``create`` captures every section as a new profile and selects it;
    ``save`` overwrites the selected (or interrupted) profile with the
    current settings; ``activate`` materializes a profile into its backing
    files (a modified selection needs ``discard_modifications: true``);
    ``deactivate`` only clears the selection; ``update`` edits the name and
    description; ``delete`` removes only the named snapshot and clears its selection linkage. ``create``,
    ``save`` and ``activate`` also need the ``expected_revisions`` of all
    four backing files. Nothing is written on any refusal.
    """
    if action not in _TOUCHED:
        raise m._error("unsupported_action", "Unknown routing profile operation.", field="action")
    expected = m._expected(expected_revision)
    revisions = _expected_revisions(expected_revisions) if action in ("create", "save", "activate") else None
    if type(discard_modifications) is not bool:
        raise m._error("invalid_value", "discard_modifications must be true or false.", field="discard_modifications")
    env = m._env(environ)
    notes = []
    with m._owner_config_lock(env):
        path, data, doc, migrated_from = _load(env)
        m._check("routing_profiles", expected, m._revision(data))
        entries = doc["profiles"]
        now = m._now_iso()
        if action != "deactivate" and action != "create":
            _name(name)
            if name not in entries:
                raise m._error("profile_not_found", "No profile has that name.", field="name")
        touched = {name} if _TOUCHED[action] else set()
        profiles = _Profiles(path, data, migrated_from, env, touched, action != "delete" or doc.get("active") == name)
        pending = doc.get("pending")
        pending_name = pending["profile"] if pending else None
        snapshot = None
        if revisions is not None:
            snapshot = _snapshot(env)
            _check_sections(snapshot, revisions)
        if action == "create":
            _name(name)
            if name in entries:
                raise m._error("profile_exists", "A profile with that name already exists.", field="name")
            if len(entries) >= _MAX_PROFILES:
                raise m._error("too_many_profiles", "At most 32 profiles are supported.")
            _description(description if description is not None else "")
            entries[name] = dict(description=description or "", created_at=now, updated_at=now,
                                 sections=list(PROFILE_SECTIONS), settings=_capture_all(snapshot))
            doc.update(active=name, applied=_applied(name, now, {s: snapshot[s]["revision"] for s in PROFILE_SECTIONS}),
                       pending=None)
            profiles.write(doc)
        elif action == "save":
            if doc.get("active") is None and pending_name is None:
                raise m._error("no_active_profile", "No profile is selected; create a new profile instead.",
                               field="name")
            if name not in (doc.get("active"), pending_name):
                raise m._error("profile_not_active", "Only the selected profile can be saved; select it first or "
                                                     "save as a new profile.", field="name")
            entry = dict(entries[name], sections=list(PROFILE_SECTIONS), settings=_capture_all(snapshot),
                         updated_at=now)
            entries[name] = entry
            doc.update(active=name, applied=_applied(name, now, {s: snapshot[s]["revision"] for s in PROFILE_SECTIONS}),
                       pending=None)
            profiles.write(doc)
        elif action == "update":
            if rename_to is not None:
                _name(rename_to, "rename_to")
                if rename_to != name and rename_to in entries:
                    raise m._error("profile_exists", "A profile with that name already exists.", field="rename_to")
            if description is not None:
                _description(description)
            entry = dict(entries[name])
            if description is not None:
                entry["description"] = description
            entry["updated_at"] = now
            target = rename_to or name
            renamed = {key: value for key, value in entries.items() if key != name}
            renamed[target] = entry
            doc["profiles"] = dict(sorted(renamed.items()))
            if doc.get("active") == name:
                doc["active"] = target
            if isinstance(doc.get("applied"), dict) and doc["applied"].get("profile") == name:
                doc["applied"] = dict(doc["applied"], profile=target)
            if pending_name == name:
                doc["pending"] = dict(pending, profile=target)
            profiles.write(doc)
        elif action == "delete":
            if pending is not None:
                raise m._error("profile_interrupted", "Resolve the interrupted activation before deleting a profile.", field="name")
            if doc.get("active") == name:
                doc.update(active=None, applied=None)
            del entries[name]
            try:
                profiles.write(doc)
                if _read_file(path) != m._pretty(doc):
                    raise m._error("profile_delete_failed", "Profile deletion could not be verified.")
            except (PortableError, OSError):
                if _read_file(path) != data:
                    m._replace_file(path, data)
                raise m._error("profile_delete_failed", "Profile deletion failed; original snapshot bytes retained or restored.") from None
        elif action == "activate":
            if "routing" in entries[name]["settings"]:
                m.validate_routing_rules({k: v for k, v in entries[name]["settings"]["routing"].items() if k in m.ROUTING_KEYS})
            active = doc.get("active")
            recovering = pending_name == name
            if active is not None and discard_modifications is not True and not recovering:
                status = _status(dict(doc, pending=None), snapshot)
                if status["state"] in ("modified", "unavailable"):
                    raise m._error("profile_modified",
                                   f"The selected profile '{active}' has unsaved changes; save them, or activate "
                                   "with discard_modifications: true. Nothing was written.",
                                   field="discard_modifications")
            _activate(doc, name, snapshot, profiles, env, now, notes)
        else:
            doc.update(active=None, applied=None, pending=None)
            profiles.write(doc)
    result = read_profiles(project_root, env)
    result["changed"] = True
    result["notes"] = profiles.notes + sorted(set(notes) | set(result.get("notes", [])))
    return result


# --- read-only helpers for other surfaces -------------------------------------------

def selection_status(environ=None):
    """``{name, state}`` of the selected profile after another surface's
    save (``None`` without a selection); best-effort, never raises."""
    try:
        env = m._env(environ)
        _, _, doc, _ = _load(env)
        name = doc.get("active") or (doc.get("pending") or {}).get("profile")
        if name is None:
            return None
        return dict(name=name, state=_status(doc, _snapshot(env))["state"])
    except PortableError as error:
        return dict(state="unavailable", code=error.code)


def effective_label(environ, routing_revision):
    """Best-effort audit label of the profile whose routing the loader read;
    never affects routing."""
    try:
        env = m._env(environ)
        _, _, doc, _ = _load(env)
        name = doc.get("active")
        if name is None or doc.get("pending") is not None:
            return None
        entry = doc["profiles"][name]
        if "routing" not in entry["sections"]:
            return None
        data = m._read_plain(_paths(env)[1])
        if m._revision(data) != routing_revision or _routing_rules(data) != entry["settings"]["routing"]:
            return None
    except PortableError:
        return None
    return dict(name=name, state="in_sync")
