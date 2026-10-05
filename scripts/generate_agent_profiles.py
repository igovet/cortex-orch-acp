#!/usr/bin/env python3
"""Render canonical worker skills and references from one shared source."""
from __future__ import annotations

import argparse
import json
from pathlib import Path
import re


ROOT = Path(__file__).resolve().parents[1]
PLUGIN = ROOT / "."
SOURCES = PLUGIN / "agent-sources"
WORKER_REFERENCES = (
    "code-and-evidence.md",
    "interactive-resources.md",
    "report-publication.md",
)
VERIFICATION_SOURCES = {
    "write": "verification-write.md",
    "read_only": "verification-read-only.md",
    "consultant": "verification-consultant.md",
}
SPECIALIZED = (
    "Profile title",
    "Role and responsibility",
    "When to use this profile",
    "Specialist workflow",
    "Quality criteria",
    "Report and handoff",
)


def sections(markdown: str) -> dict[str, str]:
    lines = markdown.rstrip().splitlines()
    if not lines or not lines[0].startswith("# "):
        raise ValueError("specialization must start with one profile title")
    result = {"Profile title": lines[0]}
    for chunk in re.split(r"(?m)^## ", "\n".join(lines[1:]))[1:]:
        heading, separator, body = chunk.partition("\n")
        if not separator or heading in result:
            raise ValueError("invalid or duplicate specialization section")
        result[heading] = body.strip()
    if set(result) != set(SPECIALIZED):
        raise ValueError("specialization sections do not match the canonical template")
    return result


def expected_skills(plugin: Path = PLUGIN) -> dict[Path, bytes]:
    source_root = plugin / "agent-sources"
    template = (source_root / "worker-protocol.md").read_text()
    profiles = json.loads((plugin / "profiles.json").read_text())["profiles"]
    rendered: dict[Path, bytes] = {}
    for profile in profiles:
        access_class = profile.get("access_class")
        if access_class not in VERIFICATION_SOURCES:
            raise ValueError(f"invalid access class for {profile['name']}")
        source = profile["source"]
        report_template = profile["report_template"]
        if report_template not in {"general", "planning", "investigation", "implementation", "verification", "documentation", "synthesis"}:
            raise ValueError(f"invalid report template for {profile['name']}")
        values = sections((source_root / source).read_text())
        selected_template = ((source_root / "consultant-protocol.md").read_text()
                             if access_class == "consultant" else template)
        prompt = selected_template.replace("{{Default report template}}", report_template)
        prompt = prompt.replace("{{Evidence and verification}}",
                               (source_root / VERIFICATION_SOURCES[access_class]).read_text().strip())
        for key in SPECIALIZED:
            prompt = prompt.replace("{{" + key + "}}", values[key])
        if "{{" in prompt or "}}" in prompt:
            raise ValueError("unresolved worker protocol placeholder")
        name = 'worker-' + Path(source).stem
        description = 'Cortex delegated specialist only: ' + profile['description']
        frontmatter = '---\nname: ' + name + '\ndescription: ' + json.dumps(description) + '\n---\n\n'
        footer = '\n<!-- END OF COMPLETE CORTEX WORKER SKILL -->\n'
        text = frontmatter + prompt.rstrip() + '\n' + footer
        rendered[plugin / 'skills' / name / 'SKILL.md'] = text.encode()
    return rendered


def expected_worker_references(plugin: Path = PLUGIN) -> dict[Path, bytes]:
    source_root = plugin / "agent-sources" / "references"
    rendered = {}
    for skill_path in expected_skills(plugin):
        reference_root = skill_path.parent / "references"
        for filename in WORKER_REFERENCES:
            rendered[reference_root / filename] = (source_root / filename).read_bytes()
    return rendered


def check(plugin: Path = PLUGIN) -> None:
    skills = expected_skills(plugin)
    if set((plugin / "skills").glob("worker-*/SKILL.md")) != set(skills):
        raise ValueError("generated worker skill set differs from source catalogue")
    expected = dict(skills)
    references = expected_worker_references(plugin)
    if set((plugin / "skills").glob("worker-*/references/*.md")) != set(references):
        raise ValueError("generated worker reference set differs from source catalogue")
    expected.update(references)
    for path, body in expected.items():
        if path.read_bytes() != body:
            raise ValueError(f"generated worker skill or reference is stale: {path}")


def write(plugin: Path = PLUGIN) -> None:
    generated = (
        expected_skills(plugin)
        | expected_worker_references(plugin)
    )
    for path, body in generated.items():
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(body)


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("action", choices=("check", "write"))
    args = parser.parse_args()
    try:
        check() if args.action == "check" else write()
    except (OSError, ValueError, KeyError, json.JSONDecodeError) as exc:
        raise SystemExit(f"Agent profile generation failed: {exc}") from None
    print(f"Worker skills {args.action}: 23 compact prompts with shared references")
