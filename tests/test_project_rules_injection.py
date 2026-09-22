import json

from cortex_runtime.delegation import live_adapters


def test_untrusted_claude_project_gets_its_rules(tmp_path, monkeypatch):
    home, project = tmp_path / "home", tmp_path / "proj"
    home.mkdir(); project.mkdir()
    (project / "AGENTS.md").write_text("code word TANGERINE-42")
    monkeypatch.setenv("HOME", str(home))
    assert "TANGERINE-42" in live_adapters.project_rules_for_claude(str(project))
    (home / ".claude.json").write_text(json.dumps({"projects": {str(tmp_path): {"hasTrustDialogAccepted": True}}}))
    assert live_adapters.project_rules_for_claude(str(project)) == ""
    assert live_adapters.project_rules_for_claude(None) == ""
