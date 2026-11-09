import { it, expect } from "vitest";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

it("detects production pin drift with a complete, otherwise valid workspace fixture", () => {
  const source = String.raw`
import importlib.util,json,pathlib,shutil,tempfile,tomllib
root=pathlib.Path.cwd()
spec=importlib.util.spec_from_file_location("pins",root/"scripts/check_workspace_pins.py")
pins=importlib.util.module_from_spec(spec)
spec.loader.exec_module(pins)
with tempfile.TemporaryDirectory(prefix="cw-pin-check-") as directory:
    target=pathlib.Path(directory)
    workspace=tomllib.loads((root/"Cargo.toml").read_text())["workspace"]
    files=["Cargo.toml","Cargo.lock","rust-toolchain.toml"]
    files += [member+"/Cargo.toml" for member in workspace["members"]]
    files += [str(desktop/name) for desktop in pins.DESKTOPS for name in ["package.json","package-lock.json"]]
    files += ["pinned-dependencies.json"]+[str(pins.EDITOR/name) for name in ["package.json","package-lock.json"]]
    for name in files:
        out=target/name
        out.parent.mkdir(parents=True,exist_ok=True)
        shutil.copyfile(root/name,out)
    assert pins.check(target)==[]
    def drift(name,old,new,diagnostic):
        path=target/name
        original=path.read_text()
        assert old in original
        path.write_text(original.replace(old,new,1))
        assert any(diagnostic in error for error in pins.check(target))
        path.write_text(original)
        assert pins.check(target)==[]
    drift(str(pins.DESKTOP/"src-tauri/Cargo.toml"),'=2.10.1','^2.10.1',"exact =x.y.z")
    drift(str(pins.DESKTOP/"package.json"),'"@tauri-apps/cli": "2.10.1"','"@tauri-apps/cli": "2.10.2"',"reviewed exact pin")
    lock=target/pins.DESKTOP/"package-lock.json"
    original=lock.read_text()
    data=json.loads(original)
    data["packages"]["node_modules/@tauri-apps/cli"]["version"]="2.10.0"
    lock.write_text(json.dumps(data))
    assert any("package-lock drift" in error for error in pins.check(target))
    lock.write_text(original)
    assert pins.check(target)==[]
    drift("Cargo.toml",'default-members = ["crates/workspace-protocol"]','default-members = ["'+pins.APP+'"]',"not a default member")
print("Complete production pin fixture and four drift cases passed")
`;
  const result = spawnSync("python3", ["-B", "-c", source], { cwd: fileURLToPath(new URL("../../..", import.meta.url)), encoding: "utf8" });
  expect(result.status, result.stdout + result.stderr).toBe(0);
});
