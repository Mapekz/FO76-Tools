"""FormID tokens as the pipeline reads and writes them.

esm renders a FormID as `0x` plus 8 uppercase hex digits (`FormId::display`
in `esm/src/formid.rs`); drafts, assessments and user input may write fewer
digits or lowercase.
"""

from __future__ import annotations

import re
from typing import Any, Union

FormIdLike = Union[int, str]

#: A FormID exactly as esm renders it. Shape only: a flags value or a model
#: hash matches too, so decoded values use `patchnotes_lib.is_ref` instead.
RENDERED_RE = re.compile(r"^0x[0-9A-Fa-f]{8}$")

#: A rendered FormID inside prose.
IN_TEXT_RE = re.compile(r"0x[0-9A-Fa-f]{8}")

#: A `0x` token of 1-8 hex digits in any case, as a draft or user may write one.
TOKEN_RE = re.compile(r"^0[xX][0-9A-Fa-f]{1,8}$")

_HEX_DIGITS = frozenset("0123456789abcdefABCDEF")


def is_rendered(value: Any) -> bool:
    """Whether `value` is a FormID string as esm renders it."""
    return isinstance(value, str) and bool(RENDERED_RE.match(value))


def canonical(value: Any) -> str | None:
    """The rendered form of a `0x` token (any case, 1-8 digits), else None."""
    if isinstance(value, str) and TOKEN_RE.match(value.strip()):
        return display(value.strip())
    return None


def looks_like_formid(token: str) -> bool:
    """esm's selector rule (`looks_like_formid` in `esm/src/formid.rs`): a
    `0x` hex value, or a bare run of up to 8 hex digits, is a FormID;
    anything else is an EditorID."""
    token = token.strip()
    body = token[2:] if token[:2].lower() == "0x" else token
    return bool(body) and len(body) <= 8 and all(c in _HEX_DIGITS for c in body)


def to_int(value: FormIdLike) -> int:
    """The raw u32 of an int, a `0x` hex string, or a bare token, read as
    esm's `parse_formid` does: a bare token of up to 8 hex digits is hex
    (`"00568635"` is 0x00568635); anything longer is decimal."""
    if isinstance(value, int):
        return value
    token = value.strip()
    if token.lower().startswith("0x"):
        return int(token, 16)
    if token and len(token) <= 8 and all(c in _HEX_DIGITS for c in token):
        return int(token, 16)
    return int(token)


def display(value: FormIdLike) -> str:
    """`0x` plus 8 uppercase hex digits, as esm renders a FormID."""
    return f"0x{to_int(value):08X}"


def sort_key(value: Any) -> int:
    """A FormID's integer value for ordering; 0 for anything unparsable."""
    try:
        return value if isinstance(value, int) else to_int(str(value))
    except ValueError:
        return 0
