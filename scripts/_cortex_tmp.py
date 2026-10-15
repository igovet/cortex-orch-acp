"""Point a script's temporary files at /var/tmp (cortex_runtime.tempdirs).

Loaded by file path, so a script that chooses its own ``cortex_runtime``
source (cortex_paseo_dev) never gets this checkout's package imported early.
"""
from __future__ import annotations

import importlib.util
from pathlib import Path


def prefer_disk_tmp() -> str:
    path = Path(__file__).resolve().parents[1] / "src" / "cortex_runtime" / "tempdirs.py"
    spec = importlib.util.spec_from_file_location("_cortex_tempdirs", path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module.prefer_disk_tmp()
