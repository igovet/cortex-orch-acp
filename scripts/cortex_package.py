"""Content stamping and isolated candidate construction (development only)."""
from pathlib import Path
import argparse
import hashlib
import json
import os
import shutil
import sys
import tempfile
import tomllib
import re
import stat
from generate_agent_profiles import check as check_agent_profiles

ROOT = Path(__file__).resolve().parents[1]
PLUGIN = ROOT
BASE = '1.0.0'

# What actually ships. Named explicitly because the repository root is no
# longer a container for the payload -- it *is* the project (the plugin
# layout was removed with the plugin, T033/T035), so walking it would fold
# tests, docs and development scripts into the package identity and make
# it change for reasons that have nothing to do with what runs.
PAYLOAD_ENTRIES = (
    'src', 'skills', 'agents', 'agent-sources', 'assets', 'policy', 'report-templates',
    'package-identity.json', 'pinned-dependencies.json', 'profiles.json',
    'requirements.lock', 'requirements.txt', 'runtime-payload.json', '.mcp.json',
)


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
        for path in sorted(entry.rglob('*')):
            if path.is_file() or path.is_symlink():
                entries.append((path.relative_to(root).as_posix(), path))
    return sorted(entries)


def _tree_digest(root):
    digest = hashlib.sha256()
    for path in sorted(root.rglob('*')):
        if path.is_symlink():
            raise ValueError('dependency target contains a symlink')
        if not path.is_file():
            continue
        if path.suffix in {'.pyc', '.pyo'} or '__pycache__' in path.parts:
            raise ValueError('dependency target contains bytecode')
        relative = path.relative_to(root).as_posix()
        data = path.read_bytes()
        digest.update(relative.encode() + b'\0' + str(len(data)).encode() + b'\0')
        digest.update(data)
    return digest.hexdigest()


def dependency_identity(plugin_root, target=None, *, required=False):
    """Return lock-manifest, installed-byte, and combined dependency identity.

    Relocated verbatim from the deleted (T033) ``cortex_runtime.runtime.state``
    gateway-supervisor module; only the dev candidate preparation flow
    (``prepare`` below) still needs this, so it now lives here.
    """
    manifest = plugin_root/'requirements.lock'
    manifest_digest = hashlib.sha256(manifest.read_bytes()).hexdigest()
    raw_target = target or (Path(os.environ['CORTEX_DEPENDENCY_DIR']) if os.environ.get('CORTEX_DEPENDENCY_DIR') else None)
    if raw_target is None:
        if required:
            raise ValueError('CORTEX_DEPENDENCY_DIR is required for isolated candidate preparation')
        bytes_digest = 'unbound'
    else:
        target_path = Path(raw_target).expanduser()
        if not target_path.is_absolute() or target_path.is_symlink():
            raise ValueError('dependency target must be an absolute non-symlink directory')
        current = Path(target_path.anchor)
        for part in target_path.parts[1:]:
            current /= part
            if current.is_symlink():
                raise ValueError('dependency target contains an unsafe symlink path')
        info = target_path.lstat()
        if not stat.S_ISDIR(info.st_mode) or info.st_uid != os.getuid() or info.st_mode & 0o077:
            raise ValueError('dependency target must be owner-only and owned by the current user')
        bytes_digest = _tree_digest(target_path)
    combined = hashlib.sha256(f'{manifest_digest}\0{bytes_digest}'.encode('ascii')).hexdigest()
    return manifest_digest, bytes_digest, combined


def _assert_no_symlink_tree(path, *, label='candidate'):
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
        for name in [*directories, *files]:
            entry = Path(parent) / name
            if entry.is_symlink():
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
        if '__pycache__' in path.parts or path.suffix in {'.pyc','.pyo'}:
            raise ValueError('bytecode in payload')
        body=path.read_bytes()
        if relative=='package-identity.json':
            value=json.loads(body); value['version']=BASE
            body=json.dumps(value,sort_keys=True,separators=(',',':')).encode()
        digest.update(relative.encode()+b'\0'+str(len(body)).encode()+b'\0'+body)
    return digest.hexdigest()


def stamp():
    path=PLUGIN/'package-identity.json'
    value=json.loads(path.read_text())
    value['version']=BASE+'+sha256.'+payload_digest(PLUGIN)[:16]
    path.write_text(json.dumps(value,indent=2)+'\n')
    return value['version']


def validate(plugin=PLUGIN):
    check_agent_profiles(plugin)
    value=json.loads((plugin/'package-identity.json').read_text())
    if value['name']!='cortex' or value['version']!=BASE+'+sha256.'+payload_digest(plugin)[:16]:
        raise ValueError('invalid package identity or stale cache stamp')
    profiles=json.loads((plugin/'profiles.json').read_text())['profiles']
    agents=list((plugin/'agents').glob('*.toml'))
    expected_profile_count=23
    if len(profiles)!=expected_profile_count or len(agents)!=expected_profile_count or len({p['name'] for p in profiles})!=expected_profile_count:
        raise ValueError('profile count')
    for profile in profiles:
        parsed=tomllib.loads((plugin/'agents'/profile['filename']).read_text())
        if parsed['name']!=profile['name'] or not parsed.get('developer_instructions'):
            raise ValueError('profile definition')
        instructions=parsed['developer_instructions']
        if '## Attached worker guidance' in instructions or '../skills/' in instructions or '.codex/plugins/' in instructions:
            raise ValueError('profiles must not contain copied skill bundles or installation paths')
        if 'cortex:context-compaction' not in instructions:
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
    capability_manifest=json.loads((plugin/'policy/capability-manifest-v1.json').read_text())
    if (set(capability_manifest) != {'schema','version','profiles','routes','targets','phases','host_enforcement'}
            or capability_manifest.get('schema') != 'capability-manifest-v1'
            or capability_manifest.get('version') != 1
            or capability_manifest.get('host_enforcement') != 'required_host_attestation'
            or not isinstance(capability_manifest.get('profiles'), dict)):
        raise ValueError('invalid semantic capability manifest')
    templates={path.stem for path in (plugin/'report-templates').glob('*.md')}
    if templates!={'general','planning','investigation','implementation','verification','documentation','synthesis','pipeline'}:
        raise ValueError('draft template set')
    mcp=json.loads((plugin/'.mcp.json').read_text())['mcpServers']['cortex']
    if mcp['command']!='python3' or mcp['args']!=['-B','./scripts/cortex.py']:
        raise ValueError('MCP entry point')
    if set(mcp.get('env_vars', [])) != {'CORTEX_OBSERVATION_DIR'}:
        raise ValueError('MCP environment contract')
    return value['version']


def private_dir(path):
    current=Path(path.anchor)
    for part in path.parts[1:]:
        current/=part
        if current.is_symlink():
            raise ValueError('unsafe candidate directory')
    path.mkdir(parents=True,exist_ok=True,mode=0o700)
    info = path.lstat()
    if (stat.S_ISLNK(info.st_mode) or not stat.S_ISDIR(info.st_mode)
            or info.st_uid != os.getuid()):
        raise ValueError('unsafe candidate directory')
    # mkdir(mode=...) does not tighten an existing directory.  These are
    # isolated candidate roots, so repair a permissive mode before any
    # receipt, dependency, or Marketplace state is written beneath them.
    os.chmod(path, 0o700)
    info = path.lstat()
    if info.st_mode & 0o077:
        raise ValueError('candidate directory must be owner-only')


def prepare():
    # Caller is the sole live-dev entry point, and points HOME at the isolated target.
    owner=Path(os.environ['CORTEX_DEV_OWNER_HOME'])
    dev=owner/'.cortex-dev'
    if not owner.is_absolute() or owner.resolve()!=owner or Path.home()!=dev or Path(os.environ['CODEX_HOME'])!=dev/'.codex':
        raise ValueError('candidate must use exact isolated HOME and CODEX_HOME')
    private_dir(dev); private_dir(dev/'.codex')
    version=stamp(); validate()
    dependency_target=Path(os.environ.get('CORTEX_DEPENDENCY_DIR',''))
    manifest_digest, dependency_bytes_digest, dependency_digest = dependency_identity(PLUGIN, dependency_target, required=True)
    candidates=dev/'candidates'; private_dir(candidates)
    target=candidates/version
    if target.exists():
        _assert_no_symlink_tree(target, label='candidate')
        validate(target/'.')
        if payload_digest(target/'.')!=payload_digest(PLUGIN):
            raise ValueError('candidate content mismatch')
    else:
        temp=Path(tempfile.mkdtemp(prefix='.staging-',dir=candidates))
        try:
            shutil.copytree(PLUGIN,temp/'.')
            (temp/'.agents/plugins').mkdir(parents=True)
            shutil.copyfile(ROOT/'.agents/plugins/marketplace.json',temp/'.agents/plugins/marketplace.json')
            validate(temp/'.')
            os.replace(temp,target)
        finally:
            if temp.exists(): shutil.rmtree(temp)
    receipt=dict(version=version,digest=payload_digest(PLUGIN),candidate=str(target),plugin=str(target/'.'),dependency_manifest_digest=manifest_digest,dependency_bytes_digest=dependency_bytes_digest,dependency_digest=dependency_digest)
    file=dev/'.codex/cortex-candidate.json'
    fd,temp=tempfile.mkstemp(dir=file.parent)
    with os.fdopen(fd,'w') as stream:
        json.dump(receipt,stream);stream.flush();os.fsync(stream.fileno())
    os.replace(temp,file)
    return receipt


if __name__=='__main__':
    parser=argparse.ArgumentParser();parser.add_argument('action',choices=['stamp','validate','prepare']);args=parser.parse_args()
    try:
        print(json.dumps(prepare()) if args.action=='prepare' else stamp() if args.action=='stamp' else validate())
    except (OSError,ValueError,KeyError):
        raise SystemExit('Cortex package operation failed; check package or isolated directory configuration.') from None
