"""Re-downloads the spec corpora used by src/resilience_tests.rs.

    python tests/fixtures/fetch_fixtures.py

Writes commonmark-0.31.2.json and gfm-0.29.json next to this script, both as a list of
{"example": n, "section": str, "markdown": str}. Standard library only.
"""
import json
import re
import urllib.request
from pathlib import Path

HERE = Path(__file__).resolve().parent
COMMONMARK = "https://spec.commonmark.org/0.31.2/spec.json"
GFM = "https://raw.githubusercontent.com/github/cmark-gfm/master/test/spec.txt"
FENCE = "`" * 32


def fetch(url: str) -> str:
    with urllib.request.urlopen(url, timeout=60) as r:
        return r.read().decode("utf-8")


def gfm_examples(spec: str):
    """Parses spec.txt: examples sit between `FENCE example` and `FENCE`, markdown before the '.' line."""
    examples, section, n = [], "", 0
    lines = spec.split("\n")
    i = 0
    while i < len(lines):
        line = lines[i]
        heading = re.match(r"^#{1,6} (.*)", line)
        if heading:
            section = heading.group(1).strip()
        if line.startswith(FENCE + " example"):
            md = []
            i += 1
            while lines[i] != ".":
                md.append(lines[i])
                i += 1
            while lines[i] != FENCE:
                i += 1
            n += 1
            # The spec writes tabs as U+2192 (→) so they are visible.
            examples.append({"example": n, "section": section, "markdown": "\n".join(md).replace("→", "\t") + "\n"})
        i += 1
    return examples


if __name__ == "__main__":
    cm = [{"example": e["example"], "section": e["section"], "markdown": e["markdown"]} for e in json.loads(fetch(COMMONMARK))]
    (HERE / "commonmark-0.31.2.json").write_text(json.dumps(cm, ensure_ascii=False, indent=0), encoding="utf-8")
    gfm = gfm_examples(fetch(GFM))
    (HERE / "gfm-0.29.json").write_text(json.dumps(gfm, ensure_ascii=False, indent=0), encoding="utf-8")
    print(f"commonmark: {len(cm)} examples, gfm: {len(gfm)} examples")
