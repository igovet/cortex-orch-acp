"""Source-only build for the rendered editor journey. Never stamps or packages the checkout."""
from pathlib import Path

from prepare_stop_reports import main as prepare

ROOT = Path(__file__).resolve().parents[2]


if __name__ == "__main__":
    raise SystemExit(prepare(ROOT / ".workspace-build/editor-native", ROOT / ".workspace-build/stop-reports", ("packages/workspace-ui",)))
