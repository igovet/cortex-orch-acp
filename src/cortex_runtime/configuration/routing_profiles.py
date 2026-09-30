"""Owner routing snapshots materialized into the user routing.json.

Profiles are a management surface, not another runtime routing source. The
loader continues to read routing.json with its existing precedence.
"""
from __future__ import annotations

import hashlib
import json
import os
import re
import stat
from typing import Mapping

from ..connections import registry
from ..portable.errors import PortableError
from . import management as m

_NAME = re.compile(r"[a-z0-9][a-z0-9-]{0,39}\Z")
_MAX_PROFILES = 32


def _paths(environ):
    root = registry.config_root(environ)
    return root / "routing-profiles.json", root / "routing.json"


def _read_file(path):
    try:
        info = path.lstat()
    except FileNotFoundError:
        return None
    if (not stat.S_ISREG(info.st_mode) or info.st_nlink != 1 or info.st_uid != os.getuid()
            or info.st_mode & 0o077):
        raise m._error("unsafe_storage", "Routing profiles must be an owner-only, single-link regular file.")
    return m._read_plain(path)


def _document(data):
    if data is None:
        return {"format": 1, "active": None, "applied": None, "profiles": {}}
    value = m._json_object(data, "routing-profiles.json", "invalid_profiles")
    if type(value.get("format")) is not int or value["format"] != 1:
        raise m._error("unsupported_profiles_format", "routing-profiles.json format must be 1.", field="format")
    profiles = value.get("profiles")
    if (not isinstance(profiles, dict) or len(profiles) > _MAX_PROFILES
            or not all(isinstance(k, str) and _NAME.fullmatch(k) and isinstance(v, dict)
                       for k, v in profiles.items())
            or value.get("active") is not None and value.get("active") not in profiles
            or value.get("applied") is not None and not isinstance(value.get("applied"), dict)):
        raise m._error("invalid_profiles", "routing-profiles.json has invalid profile metadata.")
    for entry in profiles.values():
        if (not isinstance(entry.get("rules"), dict)
                or not isinstance(entry.get("description"), str)
                or len(entry["description"]) > 256 or m._CONTROL.search(entry["description"])):
            raise m._error("invalid_profiles", "A routing profile has invalid rules or description.")
    return value


def _entry_revision(entry):
    return "sha256:" + hashlib.sha256(json.dumps(entry, sort_keys=True, separators=(",", ":"),
                                             ensure_ascii=False).encode("utf-8")).hexdigest()


def _state(doc, routing_revision):
    name = doc.get("active")
    if name is None:
        return "none"
    applied = doc.get("applied") or {}
    if (applied.get("profile") == name and applied.get("routing_revision") == routing_revision
            and applied.get("profile_revision") == _entry_revision(doc["profiles"][name])):
        return "in_sync"
    return "diverged"


def _load(environ):
    path, routing_path = _paths(environ)
    data = _read_file(path)
    doc = _document(data)
    routing_data = m._read_plain(routing_path)
    return path, routing_path, data, doc, routing_data


def _routing_rules(data):
    if data is None:
        return {}
    value = m._json_object(data, "routing.json", "invalid_routing_overrides")
    try:
        from ..routing import auto_route
        auto_route._validate(value)
    except ValueError:
        raise m._error("invalid_routing_overrides", "The user routing.json is invalid; repair it first.") from None
    return {k: value[k] for k in m.ROUTING_KEYS if k in value}


def _view(doc, path, data, routing_data, project_root, environ):
    state = _state(doc, m._revision(routing_data))
    try:
        current = _routing_rules(routing_data)
    except PortableError:
        current = None
    shadowed = next((label for label, source in m._routing_sources(project_root, environ)
                     if label != "user" and source.is_file()), None)
    profiles = []
    for name, entry in sorted(doc["profiles"].items()):
        rules = entry["rules"]
        models = rules.get("models") if isinstance(rules.get("models"), dict) else {}
        deny = rules.get("deny") if isinstance(rules.get("deny"), list) else []
        profiles.append(dict(name=name, description=entry.get("description", ""),
                             revision=_entry_revision(entry), rules=rules,
                             created_at=entry.get("created_at"), updated_at=entry.get("updated_at"),
                             summary=dict(providers=rules.get("providers"),
                                          denied_engines=[engine for engine in m.SUPPORTED_ENGINES
                                                          if f"{engine}/*" in deny],
                                          selected_models={engine: len(config.get("auto", []))
                                                           for engine, config in models.items()
                                                           if isinstance(config, dict) and isinstance(config.get("auto"), list)})))
    matches = [item["name"] for item in profiles if item["rules"] == current]
    return dict(surface="routing_profiles", path=str(path), revision=m._revision(data),
                active=doc.get("active"), state=state, applied=doc.get("applied"), profiles=profiles,
                user_routing_revision=m._revision(routing_data), shadowed_by=shadowed,
                matches_profile=matches)


def read_profiles(project_root=None, environ=None):
    env = m._env(environ)
    path, _, data, doc, routing_data = _load(env)
    return _view(doc, path, data, routing_data, project_root, env)


def _name(name):
    if not isinstance(name, str) or not _NAME.fullmatch(name):
        raise m._error("invalid_profile_name", "Profile name must be 1-40 lowercase letters, digits or hyphens.",
                       field="name")


def _description(value):
    if not isinstance(value, str) or len(value) > 256 or m._CONTROL.search(value):
        raise m._error("invalid_profile_description", "Description must be at most 256 printable characters.",
                       field="description")


def _save(path, doc):
    m._replace_file(path, m._pretty(doc))


def _apply(routing_path, routing_data, rules):
    previous = m._json_object(routing_data, "routing.json", "invalid_routing_overrides") if routing_data else {}
    preserved = {key: value for key, value in previous.items() if key not in m.ROUTING_KEYS}
    body = m._pretty({**rules, **preserved})
    m._replace_file(routing_path, body)
    return m._revision(body)


def mutate(action, expected_revision, *, name=None, description=None, rules=None, from_current=False,
           rename_to=None, expected_routing_revision=None, project_root=None, environ=None):
    """Revision-checked profile mutation under the connections registry lock."""
    expected = m._expected(expected_revision)
    env = m._env(environ)
    with m._owner_config_lock(env):
        path, routing_path, data, doc, routing_data = _load(env)
        m._check("routing_profiles", expected, m._revision(data))
        entries = doc["profiles"]
        now = m._now_iso()
        if action == "create":
            _name(name)
            if name in entries:
                raise m._error("profile_exists", "A profile with that name already exists.", field="name")
            if len(entries) >= _MAX_PROFILES:
                raise m._error("too_many_profiles", "At most 32 profiles are supported.")
            if (rules is None) == (from_current is not True):
                raise m._error("invalid_profile_source", "Supply exactly one of rules or from_current: true.")
            _description(description if description is not None else "")
            chosen = _routing_rules(routing_data) if from_current else m.validate_routing_rules(rules)
            entries[name] = dict(description=description or "", rules=chosen, created_at=now, updated_at=now)
        elif action == "update":
            _name(name)
            if name not in entries:
                raise m._error("profile_not_found", "No profile has that name.", field="name")
            if rename_to is not None:
                _name(rename_to)
                if rename_to != name and rename_to in entries:
                    raise m._error("profile_exists", "A profile with that name already exists.", field="rename_to")
            if description is not None:
                _description(description)
            was_active = doc.get("active") == name
            if was_active and rules is not None and _state(doc, m._revision(routing_data)) != "in_sync":
                raise m._error("profile_diverged", "Active profile diverged; re-apply it or detach first.")
            entry = dict(entries[name])
            if description is not None:
                entry["description"] = description
            if rules is not None:
                entry["rules"] = m.validate_routing_rules(rules)
            entry["updated_at"] = now
            target = rename_to or name
            if target != name:
                del entries[name]
            entries[target] = entry
            if was_active:
                if rules is not None:
                    m._check("routing (user)", m._expected(expected_routing_revision), m._revision(routing_data))
                    routing_revision = _apply(routing_path, routing_data, entry["rules"])
                else:
                    routing_revision = m._revision(routing_data)
                doc["active"] = target
                doc["applied"] = dict(profile=target, routing_revision=routing_revision,
                                      profile_revision=_entry_revision(entry), at=now)
        elif action == "delete":
            _name(name)
            if name not in entries:
                raise m._error("profile_not_found", "No profile has that name.", field="name")
            if doc.get("active") == name:
                raise m._error("profile_active", "Detach the active profile before deleting it.", field="name")
            del entries[name]
        elif action == "activate":
            _name(name)
            if name not in entries:
                raise m._error("profile_not_found", "No profile has that name.", field="name")
            m._check("routing (user)", m._expected(expected_routing_revision), m._revision(routing_data))
            chosen = m.validate_routing_rules(entries[name]["rules"])
            routing_revision = _apply(routing_path, routing_data, chosen)
            doc["active"] = name
            doc["applied"] = dict(profile=name, routing_revision=routing_revision,
                                  profile_revision=_entry_revision(entries[name]), at=now)
        elif action == "deactivate":
            doc["active"] = None
            doc["applied"] = None
        else:
            raise m._error("unsupported_action", "Unknown routing profile operation.", field="action")
        _save(path, doc)
    result = read_profiles(project_root, env)
    result["changed"] = True
    if action in ("activate", "update") and result["active"] is not None:
        result["routing"] = m.read_routing(project_root, env)
    return result


def write_through(doc, routing_data, new_rules, body, path):
    """Called by management.update_routing while holding connections.lock."""
    name = doc.get("active")
    if name is None:
        return None
    if _state(doc, m._revision(routing_data)) != "in_sync":
        return dict(name=name, state="diverged")
    entry = dict(doc["profiles"][name], rules=new_rules, updated_at=m._now_iso())
    doc["profiles"][name] = entry
    doc["applied"] = dict(profile=name, routing_revision=m._revision(body),
                          profile_revision=_entry_revision(entry), at=m._now_iso())
    _save(path, doc)
    return dict(name=name, written_through=True, state="in_sync")


def effective_label(environ, routing_revision):
    """Best-effort audit label; never affects routing."""
    try:
        view = read_profiles(environ=environ)
    except PortableError:
        return None
    if view["state"] == "in_sync" and view["user_routing_revision"] == routing_revision:
        return dict(name=view["active"], state="in_sync")
    return None
