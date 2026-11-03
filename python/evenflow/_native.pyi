"""Internal native extension interface."""

from pathlib import Path

__version__: str

def check_json(working_directory: Path, paths: list[Path], strict: bool) -> str: ...
def execute(arguments: list[str]) -> int: ...
