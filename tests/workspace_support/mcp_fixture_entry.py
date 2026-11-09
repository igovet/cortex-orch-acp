"""Real MCP entry with only the verification OS launch replaced for fixtures.

This is not sandbox qualification. Commands still execute and produce actual
outcomes; argument validation, authority, receipts and the MCP wire are real.
"""
from pathlib import Path
import runpy
import sys

ROOT = Path(__file__).resolve().parents[2]
sys.path[:0] = [str(ROOT / "src"), str(ROOT / "tests")]

from cortex_runtime.orchestration import controller
from sandbox_fakes import verification_command

controller._verification_command = verification_command
runpy.run_path(str(ROOT / "src/cortex.py"), run_name="__main__")
