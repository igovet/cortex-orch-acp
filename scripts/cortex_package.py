"""Content stamping and validation for the Cortex ACP distribution."""
from pathlib import Path
import argparse
import hashlib
import json
import os
from generate_agent_profiles import check as check_agent_profiles
from paseo_integrity import payload_digest as paseo_payload_digest

ROOT = Path(__file__).resolve().parents[1]
PLUGIN = ROOT
BASE = '1.0.1'

# What actually ships. Named explicitly because the repository root is no
# longer a container for the payload -- it *is* the project (the plugin
# layout was removed with the plugin, T033/T035), so walking it would fold
# tests, docs and development scripts into the package identity and make
# it change for reasons that have nothing to do with what runs.
PAYLOAD_ENTRIES = (
    'src', 'skills', 'agent-sources', 'assets', 'policy', 'report-templates', 'integrations',
    'package-identity.json', 'pinned-dependencies.json', 'profiles.json',
    'runtime-payload.json',
)
PASEO_PLUGIN = ROOT / 'integrations' / 'paseo'

# Development dependencies and generated build output can live below a
# payload entry (the Paseo integration keeps its local node_modules here).
# They are never release inputs.  Keep this by directory name so the rule
# applies to nested package workspaces without excluding integration source.
PAYLOAD_EXCLUDED_DIRS = frozenset({
    '.cache', '.git', '.next', '.turbo', '__pycache__',
    'build', 'coverage', 'dist', 'node_modules', 'out', 'target',
})


def _tree_files(root):
    """Yield real files below *root*, pruning non-payload directories."""
    root = Path(root)
    for parent, directories, files in os.walk(root, topdown=True, followlinks=False):
        directories[:] = sorted(
            name for name in directories if name not in PAYLOAD_EXCLUDED_DIRS
        )
        for name in sorted(files):
            yield Path(parent) / name


def paseo_plugin_payload_digest(root=PASEO_PLUGIN):
    """Hash the provider plugin source, excluding its generated integrity file."""
    return paseo_payload_digest(root)


def stamp_paseo_plugin(root=PASEO_PLUGIN):
    root = Path(root)
    if not root.is_dir():
        return None
    digest = paseo_plugin_payload_digest(root)
    (root / 'plugin-integrity.json').write_text(
        json.dumps({'algorithm': 'sha256', 'payload_sha256': digest}, indent=2) + '\n'
    )
    return digest


def payload_paths(root):
    """Every file of the payload, in a stable order, as (relative, path)."""
    entries = []
    for name in PAYLOAD_ENTRIES:
        entry = root/name
        if not entry.exists():
            continue
        if entry.is_file():
            entries.append((name, entry))
            continue
        for path in _tree_files(entry):
            # Compiled Python files are local execution residue, never package
            # inputs. Exclude them at payload enumeration so stamping does not
            # depend on destructive cleanup of ignored files.
            if '__pycache__' in path.parts or path.suffix in {'.pyc', '.pyo'}:
                continue
            if path.is_file() or path.is_symlink():
                entries.append((path.relative_to(root).as_posix(), path))
    return sorted(entries)


def _assert_no_symlink_tree(path, *, label='payload'):
    """Reject symlinks in every existing path component and descendant."""
    path = Path(path)
    if not path.is_absolute():
        raise ValueError(f'{label} path must be absolute')
    current = Path(path.anchor)
    for part in path.parts[1:]:
        current /= part
        if current.is_symlink():
            raise ValueError(f'unsafe symlink in {label} path')
        if current.exists() and not current.is_dir() and current != path:
            raise ValueError(f'unsafe non-directory in {label} path')
    if not path.exists():
        return
    if path.is_symlink() or not path.is_dir():
        raise ValueError(f'{label} root must be a real directory')
    for parent, directories, files in os.walk(path, topdown=True, followlinks=False):
        directories[:] = [
            name for name in directories if name not in PAYLOAD_EXCLUDED_DIRS
        ]
        for name in [*directories, *files]:
            if (Path(parent) / name).is_symlink():
                raise ValueError(f'unsafe symlink in {label} tree')


def payload_digest(plugin):
    digest=hashlib.sha256()
    for name in PAYLOAD_ENTRIES:
        entry=plugin/name
        if entry.is_dir():
            _assert_no_symlink_tree(entry, label='payload')
    for relative,path in payload_paths(plugin):
        if path.is_symlink():
            raise ValueError('symlink in payload')
        body=path.read_bytes()
        if relative=='package-identity.json':
            value=json.loads(body); value['version']=BASE
            body=json.dumps(value,sort_keys=True,separators=(',',':')).encode()
        digest.update(relative.encode()+b'\0'+str(len(body)).encode()+b'\0'+body)
    return digest.hexdigest()


def stamp():
    path=PLUGIN/'package-identity.json'
    value=json.loads(path.read_text())
    # Stamp generated integration identities before calculating the release
    # payload, so a Paseo-only edit changes the single Cortex package hash
    # without creating a circular digest.
    stamp_paseo_plugin()
    value['version']=BASE+'+sha256.'+payload_digest(PLUGIN)[:16]
    path.write_text(json.dumps(value,indent=2)+'\n')
    return value['version']


def validate(plugin=PLUGIN):
    check_agent_profiles(plugin)
    value=json.loads((plugin/'package-identity.json').read_text())
    if value['name']!='cortex' or value['version']!=BASE+'+sha256.'+payload_digest(plugin)[:16]:
        raise ValueError('invalid package identity or stale cache stamp')
    paseo_root = plugin / 'integrations' / 'paseo'
    if paseo_root.is_dir():
        integrity_path = paseo_root / 'plugin-integrity.json'
        if not integrity_path.is_file():
            raise ValueError('missing Paseo plugin integrity stamp')
        integrity = json.loads(integrity_path.read_text())
        if integrity.get('payload_sha256') != paseo_plugin_payload_digest(paseo_root):
            raise ValueError('stale Paseo plugin integrity stamp')
    profiles=json.loads((plugin/'profiles.json').read_text())['profiles']
    skills=list((plugin/'skills').glob('worker-*/SKILL.md'))
    expected_profile_count=23
    if len(profiles)!=expected_profile_count or len(skills)!=expected_profile_count or len({p['name'] for p in profiles})!=expected_profile_count:
        raise ValueError('profile count')
    for profile in profiles:
        instructions=(plugin/'skills'/('worker-'+Path(profile['source']).stem)/'SKILL.md').read_text()
        if '## Attached worker guidance' in instructions or '../skills/' in instructions or '.codex/plugins/' in instructions:
            raise ValueError('profiles must not contain copied skill bundles or installation paths')
        if '`context-compaction`' not in instructions:
            raise ValueError('missing recovery skill requirement')
        # Generated byte equality is checked above. Validate the shared role
        # structure, not incidental wording or a repeated tool bootstrap.
        if not all(section in instructions for section in (
                '## Role and responsibility','## Specialist workflow',
                '## Report class selection')):
            raise ValueError('incomplete specialist structure')
    declared=json.loads((plugin/'runtime-payload.json').read_text())['files']
    actual=sorted(p.relative_to(plugin).as_posix() for p in (plugin/'src').rglob('*.py'))
    if declared!=actual:
        raise ValueError('runtime payload differs from manifest')
    templates={path.stem for path in (plugin/'report-templates').glob('*.md')}
    if templates!={'general','planning','investigation','implementation','verification','documentation','synthesis','pipeline'}:
        raise ValueError('draft template set')
    return value['version']


if __name__=='__main__':
    parser=argparse.ArgumentParser();parser.add_argument('action',choices=['stamp','validate']);args=parser.parse_args()
    try:
        print(stamp() if args.action=='stamp' else validate())
    except (OSError,ValueError,KeyError):
        raise SystemExit('Cortex package operation failed; check package or isolated directory configuration.') from None
