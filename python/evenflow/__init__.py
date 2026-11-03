"""Source-only Rust linting backed by the Evenflow Rust library."""

import json
import os
import sys
from pathlib import Path
from typing import Any, Iterable

from ._native import __version__, check_json, execute

__all__ = ["__version__", "check", "main"]


def check(
    paths: Iterable[str | os.PathLike[str]] = (),
    *,
    working_directory: str | os.PathLike[str] | None = None,
    strict: bool = False,
) -> dict[str, Any]:
    """Return the versioned diagnostic report without printing or changing cwd.

    Paths are resolved relative to working_directory (the current directory by
    default). Lint and operational failures are returned as diagnostics, using
    the same report schema as ``evenflow check --format json``.
    """
    directory = Path.cwd() if working_directory is None else Path(working_directory).absolute()
    return json.loads(check_json(directory, [Path(path) for path in paths], strict))


def main(args: Iterable[str] | None = None) -> int:
    """Run the shared Rust CLI and return its exit status (0, 1, or 2)."""
    return execute(["evenflow", *(sys.argv[1:] if args is None else args)])
