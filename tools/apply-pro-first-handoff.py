#!/usr/bin/env python3
"""Install only FIRST HANDOFF from a saved Solaris Pro audit response."""

from __future__ import annotations

import argparse
import re
import subprocess
from pathlib import Path


def extract_first_handoff(text: str) -> str:
    start = re.search(r"(?mi)^#{1,3}\s+FIRST HANDOFF\s*$", text)
    if start is None:
        raise SystemExit("missing FIRST HANDOFF heading")
    remainder = text[start.end() :]
    end = re.search(r"(?mi)^#{1,3}\s+DEFERRED\s*/\s*DO NOT DO\s*$", remainder)
    section = remainder[: end.start() if end else None].strip()
    fenced = re.fullmatch(r"```(?:markdown|md|text)?\s*\n(.*?)\n```", section, re.S)
    if fenced:
        section = fenced.group(1).strip()
    if not section:
        raise SystemExit("FIRST HANDOFF section is empty")
    return section + "\n"


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("response", type=Path)
    args = parser.parse_args()

    root = Path(
        subprocess.check_output(["git", "rev-parse", "--show-toplevel"], text=True).strip()
    )
    response = args.response.resolve()
    section = extract_first_handoff(response.read_text(encoding="utf-8"))
    subprocess.run(
        ["codexpro", "pro-apply", "--root", str(root), "--stdin"],
        input=section,
        text=True,
        check=True,
    )
    print(f"installed FIRST HANDOFF from {response} into {root / '.ai-bridge/current-plan.md'}")


if __name__ == "__main__":
    main()
