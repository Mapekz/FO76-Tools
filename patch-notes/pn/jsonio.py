"""Reading and writing the pipeline's JSON artifacts."""

from __future__ import annotations

import json
from pathlib import Path
from typing import Any


def read(path: str | Path) -> Any:
    """Parse a UTF-8 JSON file."""
    with open(path, encoding="utf-8") as f:
        return json.load(f)


def write(path: str | Path, payload: Any) -> None:
    """Write `payload` as indented UTF-8 JSON with a trailing newline,
    creating the parent directory."""
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("w", encoding="utf-8") as f:
        json.dump(payload, f, indent=2, ensure_ascii=False)
        f.write("\n")
