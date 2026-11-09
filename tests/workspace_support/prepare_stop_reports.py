"""Source-only native qualification build. Never stamps or packages the checkout."""
import importlib.util
from pathlib import Path
import shutil
import subprocess

ROOT = Path(__file__).resolve().parents[2]
OUT = ROOT / ".workspace-build/stop-reports"
CACHE = ROOT / ".workspace-build/picker/root/.workspace-build/build-picker00"


def main(OUT=OUT, CACHE=CACHE, fresh_modules=()):
    OUT.mkdir(parents=True, exist_ok=True)
    spec = importlib.util.spec_from_file_location("workspace_builder", ROOT / "scripts/build_workspace_desktop.py")
    builder = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(builder)
    # Deliberately not the packaging snapshot: it verifies a release stamp.
    # This is an unstamped source-only build used exclusively with fake engines.
    work = OUT / "work"
    for name in ("Cargo.toml", "Cargo.lock", "rust-toolchain.toml", "package-identity.json"):
        work.mkdir(parents=True, exist_ok=True)
        shutil.copy2(ROOT / name, work / name)
    for name in (*builder.TREES, "src", "report-templates", "policy", "skills", "agent-sources"):
        source = ROOT / name
        if source.exists():
            shutil.copytree(source, work / name, dirs_exist_ok=True,
                ignore=shutil.ignore_patterns("node_modules", "target", "dist", "__pycache__", ".cortex", ".cortex-dev", ".cortex-dev-owner.json", ".cortex-workspace-dev.json", ".git"))

    shutil.copytree(ROOT / "tests", OUT / "work/tests", dirs_exist_ok=True, ignore=shutil.ignore_patterns("__pycache__"))
    shutil.copytree(ROOT / "packaging/container", OUT / "work/packaging/container", dirs_exist_ok=True)
    for name in ("target", "cargo"):
        (OUT / name).mkdir(exist_ok=True)
        subprocess.run(["rsync", "-a", str(CACHE / name) + "/", str(OUT / name) + "/"], check=True)
    for name in ("packages/workspace-ui", "packages/workspace-domain", "apps/cortex-workspace-desktop"):
        destination = OUT / "work" / name / "node_modules"
        destination.mkdir(exist_ok=True)
        subprocess.run(["rsync", "-a", str(CACHE / "work" / name / "node_modules") + "/", str(destination) + "/"], check=True)
    # A package added since the cache: the checkout's own locked install, because this build has no network.
    for name in fresh_modules:
        subprocess.run(["rsync", "-a", str(ROOT / name / "node_modules") + "/", str(OUT / "work" / name / "node_modules") + "/"], check=True)
    (OUT / "home").mkdir(exist_ok=True)
    (OUT / "pkg/usr/bin").mkdir(parents=True, exist_ok=True)
    # Native dependencies come from the already installed builder image, not apt on the host.
    return subprocess.run(["docker", "run", "--rm", "--network=none", "--user",
        str(__import__("os").getuid()) + ":" + str(__import__("os").getgid()),
        "--cap-drop=ALL", "--security-opt=no-new-privileges",
        "--mount", "type=bind,source=" + str(OUT) + ",target=/build",
        "--env", "HOME=/build/home", "--env", "CARGO_HOME=/build/cargo",
        "--env", "CARGO_TARGET_DIR=/build/target", "--env", "CARGO_BUILD_JOBS=4",
        "--env", "CARGO_PROFILE_RELEASE_DEBUG=0", "--env", "CORTEX_WORKSPACE_CHANNEL=dev",
        "--entrypoint", "/bin/sh", builder.IMAGE, "-ec",
        "cd /build/work; npm --prefix apps/cortex-workspace-desktop run build; "
        "cargo build --offline --release --locked -p cortex-workspace-desktop -p workspace-service; "
        "cp /build/target/release/cortex-workspace-desktop /build/pkg/usr/bin/cortex-workspace-dev; "
        "cp /build/target/release/workspace-service /build/pkg/usr/bin/cortex-workspace-service-dev"]).returncode


if __name__ == "__main__":
    raise SystemExit(main())
