"""The same complete file set as the installed ACP runtime, plus its Workspace host."""
import importlib.util
import json
from pathlib import Path
import sys


def payload_files(root):
    root = Path(root)
    scripts = root / "scripts"
    sys.path.insert(0, str(scripts))
    try:
        spec = importlib.util.spec_from_file_location("workspace_cortex_package", scripts / "cortex_package.py")
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        for entry in module.PAYLOAD_ENTRIES:
            if not (root / entry).exists():
                raise ValueError("missing runtime payload entry: " + entry)
        files = dict(module.payload_paths(root))
        identity = json.loads((root / "package-identity.json").read_text())
        if identity["version"] != module.BASE + "+sha256." + module.payload_digest(root)[:16]:
            raise ValueError("runtime content stamp mismatch")
    finally:
        sys.path.remove(str(scripts))
    declared = json.loads((root / "runtime-payload.json").read_text())["files"]
    for name in declared:
        if name not in files:
            raise ValueError("missing runtime module: " + name)
    for host in ("agent_launcher.py", "machine_tools.py", "task_git.py", "same_branch.py", "scheduler.py", "mcp_discovery.py", "task_titles.py", "directory_search.py", "maintenance.py", "engine_connections.py", "owner_workflows.py", "sprint_recovery.py", "planning_jobs.py"):
        files["crates/workspace-service/" + host] = root / "crates/workspace-service" / host
    for name, path in files.items():
        if (path.is_symlink() or not path.is_file() or root.resolve() not in path.resolve().parents
                or any(part in {".git", "tests", "prototypes", "node_modules"} for part in Path(name).parts)):
            raise ValueError("unsafe or missing runtime file: " + name)
    return sorted(files.items())
