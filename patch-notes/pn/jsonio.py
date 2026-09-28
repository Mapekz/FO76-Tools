"""Reading and writing the pipeline's JSON artifacts."""

from __future__ import annotations

import json
import os
import tempfile
from pathlib import Path
from typing import Any


def read(path: str | Path) -> Any:
    """Parse a UTF-8 JSON file."""
    with open(path, encoding="utf-8") as f:
        return json.load(f)


def write(path: str | Path, payload: Any) -> None:
    """Write `payload` as indented UTF-8 JSON with a trailing newline,
    creating the parent directory. The file appears whole or not at all: it
    is written to a temporary file of this call's own beside the target and
    renamed over it, so neither an interrupted write nor a concurrent writer
    of the same artifact leaves it truncated or mixed."""
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    fd, tmp_name = tempfile.mkstemp(dir=path.parent, prefix=f".{path.name}.", suffix=".tmp")
    tmp = Path(tmp_name)
    try:
        try:
            f = os.fdopen(fd, "w", encoding="utf-8")
        except BaseException:
            os.close(fd)
            raise
        with f:
            json.dump(payload, f, indent=2, ensure_ascii=False)
            f.write("\n")
        os.replace(tmp, path)
    finally:
        tmp.unlink(missing_ok=True)
