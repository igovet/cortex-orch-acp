# Python-era project fixture provenance

These are disposable projects authored by Cortex's retired Python runtime,
not a Rust database with a patched version header. Both SQLite inputs were
created, populated, and closed by the producer's own Store code using Python
isolated mode and stdlib SQLite 3.46.1. No model, network, real project, native
account, or existing config was used. Archive data is invented fixture text.
Only the historical store's automatically issued **hashes** of throwaway channel
secrets are persisted; their plaintext values were never saved or printed.

| Archive | Producer revision | Absolute project identity |
| --- | --- | --- |
| `format-11/store` | `e5c0963f677cbbb042462220058311a78e2a26e7` | `/tmp/cortex-python-era-fixture-v1/format-11` |
| `format-12/store` | `2d9c1d878557d7d0dd706ddae9c58125cc624614` | `/tmp/cortex-python-era-fixture-v1/format-12` |
| `profiles/format-1.json` | `e243941e867ecb9fbfc4f2799a9ea734b1448d42` | Separate disposable user config, not project-bound |
| `profiles/format-2.json` | `2d9c1d878557d7d0dd706ddae9c58125cc624614` | Separate disposable user config, not project-bound |

Both projects contain two tasks, four published documents (including one
`pipeline.md`), one unpublished draft, a project default, and a bound ACP host
session with a two-message transcript. The format-11 producer predates the
Python `runtime_tasks` state machine and project routing management writer, so
neither is fabricated for that archive. The format-12 producer writes one
cancelled and one unfinished runtime task plus a project routing file. The
profile documents are independent Python-written user-level samples.

The archive directory is named `store` so Git can carry it without including
an active store root. Tests restore private modes (0700 directories, 0600
regular files), exclusively create the identity root, and then copy the files.
No project identity or SQLite payload is rewritten for the positive tests.
A preexisting path is a hard collision, not something the test reuses or
deletes; normal test teardown removes only its own root. Run the focused cases
sequentially in the same filesystem namespace. When testing the Dev ACP
binary, the test deliberately transplants the same payload into its
channel-specific directory at the same absolute project identity. It checks
draft index and body there, not cross-channel draft access; the direct Release
Store case tests the draft through the Rust reader.

The legacy-root control places the Python format-12 files, unchanged, under
`.codex/cortex`, the input layout recognized by the historical implementation's
one-way move. The format-10 and unknown-version controls change only the
four-byte version header on disposable copies; neither is an archive of that
version. `expected.json` records pre-open SQLite rows for comparison: the
Rust tests need neither Python nor a network connection.

## Generator (documentation only)

Run from the repository root, with the exact fixture root unused. The producer
generates new IDs, timestamps, and draft inode metadata on each run; these steps
reproduce the provenance and contents, not the original random bytes. Do not
overwrite the archived sample to run the tests.

```bash
set -euo pipefail
project_script=$(cat <<'PYTHON'
import os, sys, sqlite3
from pathlib import Path
src, version, root = sys.argv[1:]
sys.path.insert(0, src)
p = Path(root)
from cortex_runtime.store import Store
from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.acp import sessions, replay_store
from cortex_runtime.configuration.policy import PolicyCeiling
from cortex_runtime.configuration.store import write_project_defaults

p.mkdir(mode=0o700)
store = Store(p/'.cortex', project_root=p)
def task(text, name):
    return bootstrap_task(store, SourceEvent(origin='acp', event_id=name), text, 'fixture-owner')
a = task('Historical request to preserve', 'primary')
b = task('Second historical request to preserve', 'secondary')

def draft(item, template, body, name, publish=True):
    with store.connection() as db:
        principal = db.execute('SELECT thread_id FROM thread_bindings WHERE task_id=?', (item.task_id,)).fetchone()[0]
    d = store.call('create_draft', {'template':template,'request_key':'draft-'+name}, principal, trusted=True)
    path = Path(d['draft_path'])
    marker = path.read_text().split('\n\n', 1)[0]+'\n\n'
    path.write_text(marker + body + '\n', encoding='utf-8')
    if publish:
        store.call('write_report', dict(draft_id=d['draft_id'], title=name, summary='Historical fixture content.',
                    author='fixture-author', request_key='publish-'+name), principal, trusted=True)
draft(a, 'general', 'The historical report body is intact.', 'Fixture report')
draft(a, 'pipeline', '# Fixture pipeline\n\nPreserve this historical plan.', 'Fixture pipeline')
draft(b, 'general', 'This old unpublished draft is intact.', 'Fixture draft', False)
sid = '9b40b9a3-1de9-4a1f-a7ba-444af198d95c'
sessions.create_session(store, sid, str(p), {'name':'python-era-fixture','version':'1'}, {}, [])
sessions.bind_task(store, sid, a.task_id)
replay_store.append_host_transcript(store, sid, 'user_message', 'Historical question')
replay_store.append_host_transcript(store, sid, 'agent_message', 'Historical Python answer')
write_project_defaults(store, str(p), {'language':'en'}, PolicyCeiling())
if version == '12':
    from cortex_runtime.orchestration.state import TaskMachine
    from cortex_runtime.configuration import management, routing_profiles
    machine = TaskMachine(store)
    machine.create(a.task_id, 'Historical terminal goal')
    machine.cancel(a.task_id)
    machine.create(b.task_id, 'Historical unfinished goal')
    routing = {'model_policy_version':2,'deny':['fixture-disabled-model']}
    management.update_routing('project', 'absent', routing, project_root=str(p))
    view = routing_profiles.read_profiles()
    routing_profiles.mutate('create', view['revision'], name='historical-v2',
                            description='Historical Python format 2', expected_revisions=view['current_revisions'])
with store.connection() as db:
    assert db.execute('PRAGMA user_version').fetchone()[0] == int(version)
    assert db.execute('PRAGMA quick_check').fetchone()[0] == 'ok'
    print(version, 'tasks', db.execute('SELECT count(*) FROM tasks').fetchone()[0],
          'reports', db.execute('SELECT count(*) FROM reports').fetchone()[0])
PYTHON
)
profile1_script=$(cat <<'PYTHON'
import sys
sys.path.insert(0,sys.argv[1])
from cortex_runtime.configuration import routing_profiles
view=routing_profiles.read_profiles()
routing_profiles.mutate('create', view['revision'], name='historical-v1', description='Historical Python format 1', rules={'deny':['fixture-disabled-model']})
PYTHON
)
bash -c 'set -euo pipefail
umask 022
root=/tmp/cortex-python-era-fixture-v1
mkdir -m 700 "$root"
for spec in 2d9c1d878557d7d0dd706ddae9c58125cc624614:12 e5c0963f677cbbb042462220058311a78e2a26e7:11 e243941e867ecb9fbfc4f2799a9ea734b1448d42:profiles1; do
  rev=${spec%:*}; label=${spec#*:}
  mkdir -m 700 "$root/source-$label" "$root/home-$label"
  git archive "$rev" src report-templates | tar -xf - -C "$root/source-$label"
done
for n in 12 11; do
  env -i PATH=/usr/bin:/bin HOME="$root/home-$n" XDG_CONFIG_HOME="$root/home-$n/config" XDG_STATE_HOME="$root/home-$n/state" python3 -I -B -c "$1" "$root/source-$n/src" "$n" "$root/format-$n"
done
env -i PATH=/usr/bin:/bin HOME="$root/home-profiles1" XDG_CONFIG_HOME="$root/home-profiles1/config" python3 -I -B -c "$2" "$root/source-profiles1/src"' fixture-generation "$project_script" "$profile1_script"

root=/tmp/cortex-python-era-fixture-v1
out=tests/fixtures/python-era
mkdir -p tests/fixtures
mkdir "$out"
for n in 11 12; do
  mkdir "$out/format-$n"
  cp -R "$root/format-$n/.cortex" "$out/format-$n/store"
done
mkdir "$out/profiles"
cp "$root/home-profiles1/config/cortex/routing-profiles.json" "$out/profiles/format-1.json"
cp "$root/home-12/config/cortex/routing-profiles.json" "$out/profiles/format-2.json"
find "$out" -type d -exec chmod 0755 '{}' +
find "$out" -type f -exec chmod 0644 '{}' +

snapshot_script=$(cat <<'PYTHON'
import json, sqlite3, sys
from pathlib import Path
root = Path(sys.argv[1]).resolve()
queries = json.loads(sys.argv[2])
for version in (11, 12):
    fixture = root / f'format-{version}'
    with sqlite3.connect((fixture/'store/cortex.sqlite3').as_uri()+'?immutable=1', uri=True) as db:
        expected = {k: [json.loads(row[0]) for row in db.execute(sql)]
                    for k, sql in queries.items() if k != 'runtime_tasks' or version == 12}
    (fixture/'expected.json').write_text(json.dumps(expected, indent=2)+'\n', encoding='utf-8')
PYTHON
)
queries=$(cat <<'JSON'
{
  "tasks": "SELECT json_array(id,project_root,created_at) FROM tasks ORDER BY id",
  "reports": "SELECT json_array(id,task_id,kind,filename) FROM reports ORDER BY id",
  "editions": "SELECT json_array(sequence,report_id,title,summary,author,digest,size_bytes) FROM editions ORDER BY sequence",
  "drafts": "SELECT json_array(id,task_id,owner_thread_id,kind,template,path,created_at,published_report_id) FROM drafts ORDER BY id",
  "defaults": "SELECT json_array(project_root,json(defaults),json(ceiling),updated_at) FROM project_configuration_defaults ORDER BY project_root",
  "sessions": "SELECT json_array(session_id,project_root,task_id,json(client_info),json(client_capabilities),json(mcp_servers),created_at,closed_at) FROM acp_host_sessions WHERE session_id='9b40b9a3-1de9-4a1f-a7ba-444af198d95c' ORDER BY session_id",
  "transcript": "SELECT json_array(session_id,seq,kind,text,json(attachments_json),created_at) FROM host_transcript ORDER BY session_id,seq",
  "runtime_tasks": "SELECT json_array(task_id,state,goal) FROM runtime_tasks ORDER BY task_id"
}
JSON
)
python3 -I -B -c "$snapshot_script" "$out" "$queries"
```

## SHA-256 of all fixture payload files

Paths are relative to this directory; the provenance document does not hash
itself. Run the following from `tests/fixtures/python-era` to verify:

```bash
sha256sum --check <<'SHA256'
e2bbf5d703c45351947748176a3494ef92ad0938dc869f5383bbf2d6e93da74a  format-11/expected.json
e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855  format-11/store/.access.lock
400df6e2ee4fd97d26c97a4c84fb259bd75b2ca6c9a0a8114ece4df1e2d52724  format-11/store/cortex.sqlite3
951252462ee8b5f12b8e1231093379141ba8f3b48933fe2193ff5be352e78f22  format-11/store/draft-reports/d_be79c6dbf091.md
03c505fb07133b37c2f1de5679dbc85731620c297df683ddaadb5e771673e409  format-11/store/t_08e015c2ed9e/r_c25db2dc4f5b.md
dad60191ae8e7f6c4a85ca0257fd463c15070013570a54a426c83fe5eb42fe88  format-11/store/t_4455b9ce763f/pipeline.md
ff10f16e295fb25896cb50e44f12adef929dccae0f1e2ed99dfcfe19c2397d0e  format-11/store/t_4455b9ce763f/r_70202a33e1d1.md
c4d9506e800a196d10d019df28a9a4f4a1f31423f87603f47c3ec137d46b8199  format-11/store/t_4455b9ce763f/r_fd271b79a306.md
d5776d52d44eb6ec25c8ba844b3a9950ee34cc8ffb6dc0c3f5d14cb2a566a62a  format-12/expected.json
e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855  format-12/store/.access.lock
73c48fd39a752cbeb5d101897f499cbae6c236a4af9efbc413b58777c5603462  format-12/store/cortex.sqlite3
49c49b295437f2c76a5e467096cb9d0e138dfc6b4841e02749554549d26e306d  format-12/store/draft-reports/d_2cb7b6321878.md
38357cbea59e4c4768c850730e0776c9c737230c48b25f0a87420871317adefe  format-12/store/routing.json
03c505fb07133b37c2f1de5679dbc85731620c297df683ddaadb5e771673e409  format-12/store/t_682c7de28079/r_bb29a7a448b4.md
da4b7a336c18e0add12c19426bb95230b5a0d13c2b234913557e3781e3023ab8  format-12/store/t_e14c775524fd/pipeline.md
ff10f16e295fb25896cb50e44f12adef929dccae0f1e2ed99dfcfe19c2397d0e  format-12/store/t_e14c775524fd/r_5e7336c836f8.md
e4b2f08600898af61ce49e6e64276ea1a63cbe1744e40f8a4801137f467ff5f9  format-12/store/t_e14c775524fd/r_95b95c45694a.md
7581ddac6e7385c6d344fbf543fffa6d11d6110e1785aaadad22b91b3d1a0ef2  profiles/format-1.json
0a6038504503a212986bd19ed016db78b75809ad74a8a73a04cb228eb511c92c  profiles/format-2.json
SHA256
```
