#!/usr/bin/env python3
"""Preview/apply an owner-supplied policy candidate, without engine/network calls."""
from __future__ import annotations
import argparse
import json
from pathlib import Path
import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "src"))
from cortex_runtime.configuration.model_policy_migration import main

if __name__ == "__main__":
    raise SystemExit(main())
