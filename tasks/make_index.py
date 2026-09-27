#!/usr/bin/env python3
"""Regenerate tasks/INDEX.md from the front matter of every task file.

Run after creating a task or changing its status:  python3 tasks/make_index.py
Agents never edit INDEX.md by hand, so parallel agents can't conflict on it.
"""
import pathlib, re

HERE = pathlib.Path(__file__).parent
ORDER = ["in-progress", "review", "todo", "blocked", "done"]
LABEL = {"todo": "À faire", "in-progress": "En cours", "review": "En review", "done": "Fait", "blocked": "Bloqué"}


def front_matter(path):
    text = path.read_text(encoding="utf-8")
    m = re.match(r"---\n(.*?)\n---", text, re.S)
    if not m:
        return None
    meta = {}
    for line in m.group(1).splitlines():
        if ":" in line:
            k, v = line.split(":", 1)
            meta[k.strip()] = v.split("#")[0].strip().strip('"')
    meta["file"] = path.name
    return meta


tasks = [t for p in sorted(HERE.glob("T-*.md")) if (t := front_matter(p))]
lines = ["# Index des tâches", "", "_Généré par `python3 tasks/make_index.py` — ne pas éditer à la main._", ""]
for status in ORDER:
    group = [t for t in tasks if t.get("status") == status]
    if not group:
        continue
    lines += [f"## {LABEL[status]} ({len(group)})", "", "| id | tâche | domaine | priorité | dépend de | branche |", "|---|---|---|---|---|---|"]
    for t in sorted(group, key=lambda t: (t.get("priority", "P9"), t["id"])):
        lines.append(f"| [{t['id']}]({t['file']}) | {t.get('title','')} | {t.get('area','')} | {t.get('priority','')} | {t.get('depends_on','')} | {t.get('branch','')} |")
    lines.append("")
(HERE / "INDEX.md").write_text("\n".join(lines), encoding="utf-8")
print(f"{len(tasks)} tasks indexed")
