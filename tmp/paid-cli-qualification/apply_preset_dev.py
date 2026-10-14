"""Dev-only: back up the ~/.paseo-dev Cortex routing/defaults and adopt the
checkout preset's routing object. Refuses any non-dev config root."""
import hashlib
import json
import os
import shutil
import sys
import time
from pathlib import Path

CHECKOUT = Path(sys.argv[1]).resolve()
MODE = sys.argv[2]  # "apply" or "restore:<backup dir>"
sys.path.insert(0, str(CHECKOUT / "src"))

DEV = Path.home() / ".paseo-dev" / "cortex-xdg"
ENV = {"XDG_CONFIG_HOME": str(DEV / "config"), "XDG_STATE_HOME": str(DEV / "state"), "HOME": str(Path.home())}

from cortex_runtime.configuration import management  # noqa: E402
from cortex_runtime.connections import registry  # noqa: E402

root = Path(registry.config_root(ENV)).resolve()
if root != (DEV / "config" / "cortex").resolve():
    raise SystemExit(f"refusing: config root {root} is not the dev root")

if MODE == "apply":
    backup = root / f".paid-cli-20261014-backups" / time.strftime("%Y%m%dT%H%M%S")
    backup.mkdir(parents=True, mode=0o700)
    for name in ("routing.json", "defaults.json"):
        shutil.copy2(root / name, backup / name)
    data = (root / "routing.json").read_bytes()
    rev = "sha256:" + hashlib.sha256(data).hexdigest()
    preset = json.loads((CHECKOUT / "policy" / "model-settings-default.json").read_text())
    result = management.update_routing("user", rev, preset["routing"], environ=ENV)
    after = json.loads((root / "routing.json").read_text())
    print(json.dumps({"backup": str(backup), "before_revision": rev,
                      "equal_to_preset_after": {k: after.get(k) == preset["routing"].get(k) for k in preset["routing"]},
                      "result_keys": sorted(result)}, indent=1))
elif MODE.startswith("restore:"):
    backup = Path(MODE.split(":", 1)[1]).resolve()
    if root not in backup.parents:
        raise SystemExit("refusing: backup outside dev root")
    for name in ("routing.json", "defaults.json"):
        shutil.copy2(backup / name, root / name)
    print(json.dumps({"restored_from": str(backup)}))
else:
    raise SystemExit("bad mode")
