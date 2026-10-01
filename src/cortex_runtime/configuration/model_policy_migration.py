"""Explicit owner-candidate model-policy migration; no template overlay or discovery."""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import stat
import secrets
from ..portable.errors import PortableError
from pathlib import Path

from ..connections import registry
from ..routing import defaults
from . import management as m


def _preserve(old, new, field="routing"):
    """Existing owner decisions must survive; only missing policy may be completed."""
    for key, value in old.items():
        if field == "routing" and key == "model_policy_version":
            continue
        if field == "routing" and key == "tiers" and value:
            for model, tier in value.items():
                matches = [config.get("tiers", {}).get(model) for config in new.get("models", {}).values()
                           if model in config.get("tiers", {})]
                from ..routing.model_policy import tier_list
                if len(matches) != 1 or tier_list(matches[0]) != tier_list(tier):
                    raise m._error("migration_legacy_tiers_ambiguous", "Move each legacy tier to one exact engine without changing its placement.", field=f"routing.tiers.{model}")
            continue
        if key not in new:
            raise m._error("migration_preservation_conflict", "The candidate removes an existing owner setting.", field=f"{field}.{key}")
        if isinstance(value, dict) and isinstance(new[key], dict):
            _preserve(value, new[key], f"{field}.{key}")
        elif value != new[key]:
            raise m._error("migration_preservation_conflict", "The candidate changes an existing owner decision; repair it separately with explicit owner consent.", field=f"{field}.{key}")


def _plan(candidate_path, environ, profile):
    # Candidate is owner-supplied, never the shipped installation template.
    candidate_bytes = m._read_plain(Path(candidate_path))
    if candidate_bytes is None:
        raise m._error("migration_candidate_missing", "Supply an existing candidate JSON file.")
    candidate = m._json_object(candidate_bytes, "candidate", "invalid_migration_candidate")
    public = {k: v for k, v in candidate.items() if k in m.ROUTING_KEYS}
    m.validate_routing_rules(public)
    root = registry.config_root(environ)
    path = root / ("routing-profiles.json" if profile is not None else "routing.json")
    for directory in (root, *root.parents):
        try:
            info = directory.lstat()
        except FileNotFoundError:
            continue
        if stat.S_ISLNK(info.st_mode) or not stat.S_ISDIR(info.st_mode):
            raise m._error("unsafe_storage", "Migration requires real configuration directories, without symlink ancestors.")
        if directory == root and (info.st_uid != os.getuid() or info.st_mode & 0o077):
            raise m._error("unsafe_storage", "Migration requires an owner-private configuration root.")
    data = registry.read_owner_file(path)
    if data is None:
        raise m._error("migration_source_missing", "There is no saved owner file to migrate; use installation for absent files.")
    document = m._json_object(data, path.name, "invalid_migration_source")
    if profile is None:
        old = document
        proposed = candidate
    else:
        from . import routing_profiles
        routing_profiles._name(profile)
        routing_profiles._parse(data)  # Validate all retained snapshots before any proposed write.
        if document.get("pending") is not None:
            raise m._error("profile_interrupted", "Resolve the interrupted activation before migrating a snapshot.")
        if type(document.get("format")) is not int or document.get("format") not in (1, 2):
            raise m._error("unsupported_profiles_format", "Migrate only a format-1 or format-2 profile snapshot.")
        entry = (document.get("profiles") or {}).get(profile)
        if not isinstance(entry, dict):
            raise m._error("profile_not_found", "No saved profile has that name.", field="name")
        old = entry.get("rules") if document["format"] == 1 else (entry.get("settings") or {}).get("routing")
        if not isinstance(old, dict):
            raise m._error("invalid_migration_source", "The snapshot has no valid routing object.")
        proposed = json.loads(json.dumps(document))
        target = proposed["profiles"][profile]
        if document["format"] == 1:
            target["rules"] = candidate
        else:
            target["settings"]["routing"] = candidate
    _preserve(old, candidate)
    # Unknown owner fields are retained, not silently stripped by validation.
    body = m._pretty(proposed)
    return path, data, body, dict(expected_revision=m._revision(data),
                                 candidate_sha256=hashlib.sha256(candidate_bytes).hexdigest(),
                                 proposed_revision=m._revision(body), profile=profile,
                                 changed=body != data)


def preview(candidate_path, *, environ=None, profile=None):
    """Nonmutating digest-bound preview; never initializes roots or lock files."""
    return dict(_plan(candidate_path, environ, profile)[3], applied=False)


def apply(candidate_path, expected_revision, candidate_sha256, *, consent=False, environ=None, profile=None):
    """Owner consent plus exact source/candidate digests; verified write and rollback."""
    if consent is not True:
        raise m._error("migration_consent_required", "Explicit migration consent is required.")
    m._expected(expected_revision)
    with m._owner_config_lock(environ):
        path, data, body, plan = _plan(candidate_path, environ, profile)
        m._check(path.name, expected_revision, plan["expected_revision"])
        if candidate_sha256 != plan["candidate_sha256"]:
            raise m._error("migration_candidate_changed", "The candidate differs from the preview; preview it again.")
        if not plan["changed"]:
            return dict(plan, applied=False)
        root = registry.config_root(environ)
        backup_dir = root / ".migration-backups"
        registry.ensure_private_dir(backup_dir, root=root, environ=environ)
        backup = backup_dir / f"{path.stem}.model-policy.{secrets.token_hex(12)}.json"
        if not defaults._create_private(backup, data.decode("utf-8")):
            raise m._error("migration_backup_conflict", "Backup creation conflicted; nothing was replaced.")
        if registry.read_owner_file(backup) != data:
            raise m._error("migration_backup_invalid", "Private backup verification failed; owner settings were not replaced.")
        m._check(path.name, expected_revision, m._revision(registry.read_owner_file(path)))
        try:
            m._replace_file(path, body)
            if registry.read_owner_file(path) != body:
                raise m._error("migration_verification_failed", "Written bytes could not be verified.")
        except Exception:
            try:
                m._replace_file(path, data)
                if registry.read_owner_file(path) != data:
                    raise m._error("migration_rollback_failed", "Rollback could not be verified; retain the private backup and repair storage.")
            except Exception:
                raise m._error("migration_rollback_failed", "Storage interrupted rollback; retain the private backup and repair storage.") from None
            raise m._error("migration_verification_failed", "Migration failed; original bytes restored and private backup retained.") from None
    return dict(plan, applied=True, backup=str(backup))


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=("preview", "apply"))
    parser.add_argument("--candidate", required=True)
    parser.add_argument("--profile", help="Migrate only this saved snapshot; never working defaults or other profiles.")
    parser.add_argument("--expected-revision")
    parser.add_argument("--candidate-sha256")
    parser.add_argument("--i-consent", action="store_true")
    args = parser.parse_args(argv)
    try:
        if args.action == "preview":
            result = preview(args.candidate, profile=args.profile)
        else:
            result = apply(args.candidate, args.expected_revision, args.candidate_sha256,
                           consent=args.i_consent, profile=args.profile)
    except OSError:
        print(json.dumps(dict(ok=False, error=dict(code="unsafe_storage", message="Storage refused migration; inspect the private backup before retrying."))))
        return 1
    except PortableError as error:
        print(json.dumps(dict(ok=False, error=m.error_payload(error))))
        return 1
    print(json.dumps(dict(ok=True, result=result), sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
