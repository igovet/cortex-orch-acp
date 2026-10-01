import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "scripts"))
import cortex_paseo_clone as clone  # noqa: E402


def _home(path, server_id, keypair):
    path.mkdir()
    (path / "server-id").write_text(server_id)
    (path / "daemon-keypair.json").write_text(keypair)
    (path / "config.json").write_text("{}")
    (path / "paseo.pid").write_text("{}")
    (path / "agents").mkdir()
    (path / "agents" / "server-id").write_text("nested data is not identity")


def test_clone_never_copies_production_daemon_identity(tmp_path):
    source, target = tmp_path / "prod", tmp_path / "dev"
    _home(source, "srv_prod", '{"prod":1}')
    clone.clone_home(source, target)
    assert not (target / "server-id").exists()
    assert not (target / "daemon-keypair.json").exists()
    assert not (target / "paseo.pid").exists()
    assert (target / "config.json").read_text() == "{}"
    assert (target / "agents" / "server-id").read_text() == "nested data is not identity"


def test_clone_keeps_dev_identity_but_drops_a_copied_production_one(tmp_path):
    source, target = tmp_path / "prod", tmp_path / "dev"
    _home(source, "srv_prod", '{"prod":1}')
    _home(target, "srv_dev", '{"prod":1}')  # keypair previously copied from production
    clone.clone_home(source, target)
    assert (target / "server-id").read_text() == "srv_dev"
    assert not (target / "daemon-keypair.json").exists()
