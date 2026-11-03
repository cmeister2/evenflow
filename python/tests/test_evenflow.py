"""Exercise the installed Python library and both Python command entry points."""

import json
from pathlib import Path
import shutil
import subprocess
import sys

import evenflow
import pytest


@pytest.mark.parametrize("strict", [False, True])
def test_report_matches_cli(tmp_path, strict):
    """Native calls and commands must preserve diagnostics and enforcement."""
    source = tmp_path / "lint.rs"
    source.write_text("fn lint() { if ready() { prepare(); write(); commit(); } }\n")
    before = Path.cwd()
    report = evenflow.check([source.name], working_directory=tmp_path, strict=strict)
    assert Path.cwd() == before
    assert report["summary"]["analysis_completed"] is True
    assert any(item["rule"] == "FC001" for item in report["diagnostics"])
    executable = shutil.which("evenflow")
    assert executable is not None
    for command in ([sys.executable, "-m", "evenflow"], [executable]):
        result = subprocess.run(
            [*command, "check", source.name, "--format", "json", *(["--strict"] if strict else [])],
            cwd=tmp_path, capture_output=True, text=True, check=False,
        )
        assert result.returncode == int(strict)
        assert json.loads(result.stdout) == report


def test_configuration_and_default_selection(tmp_path):
    """Python uses the runner's configuration discovery and default paths."""
    (tmp_path / "clean.rs").write_text("fn clean() {}\n")
    assert evenflow.check(working_directory=tmp_path)["summary"]["analysis_completed"]
    (tmp_path / "evenflow.toml").write_text("unknown = true\n")
    report = evenflow.check(working_directory=tmp_path)
    assert not report["summary"]["analysis_completed"]
    assert any(item["category"] == "configuration" for item in report["diagnostics"])


def test_operational_failure(tmp_path):
    """Missing files return diagnostics in the library and status 2 in the CLI."""
    report = evenflow.check(["missing.rs"], working_directory=tmp_path)
    assert not report["summary"]["analysis_completed"]
    result = subprocess.run(
        [sys.executable, "-m", "evenflow", "check", "missing.rs", "--format", "json"],
        cwd=tmp_path, capture_output=True, text=True, check=False,
    )
    assert result.returncode == 2
    assert json.loads(result.stdout) == report


def test_version_and_cli_errors():
    """The wheel version and shared command parser remain available."""
    result = subprocess.run([sys.executable, "-m", "evenflow", "--version"], capture_output=True, text=True, check=True)
    assert result.stdout.strip() == f"evenflow {evenflow.__version__}"
    assert evenflow.main(["invalid-subcommand"]) == 2
