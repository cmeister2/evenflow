#!/usr/bin/env bash
# Verify a prebuilt executable away from the source tree without Rust on PATH.
set -euo pipefail

if [[ $# -ne 1 ]]; then
    printf 'usage: bash scripts/release-smoke.sh PATH_TO_EVENFLOW\n' >&2
    exit 2
fi

binary_path=$1
smoke_root=$(mktemp -d)
trap 'rm -rf "$smoke_root"' EXIT
mkdir -p "$smoke_root/bin" "$smoke_root/empty-path" "$smoke_root/project"
cp "$binary_path" "$smoke_root/bin/evenflow"
chmod +x "$smoke_root/bin/evenflow"
env_command=$(command -v env)
python_command=$(command -v python3)

cat > "$smoke_root/project/clean.rs" <<'RUST'
fn clean() {
    // Complete the ordered transaction only after readiness is established.
    if ready() {
        prepare();
        write();
        commit();
    }
}
RUST

cat > "$smoke_root/project/lint.rs" <<'RUST'
fn lint() {
    if ready() {
        prepare();
        write();
        commit();
    }
}
RUST

cat > "$smoke_root/project/broken.rs" <<'RUST'
fn broken( {
RUST

# Resolve helper commands before clearing the executable's environment and PATH.
cd "$smoke_root/project"
"$env_command" -i PATH="$smoke_root/empty-path" "$smoke_root/bin/evenflow" check clean.rs --strict --format json > "$smoke_root/clean.json"

set +e
"$env_command" -i PATH="$smoke_root/empty-path" "$smoke_root/bin/evenflow" check lint.rs --strict --format json > "$smoke_root/lint.json"
lint_status=$?
"$env_command" -i PATH="$smoke_root/empty-path" "$smoke_root/bin/evenflow" check broken.rs --format json > "$smoke_root/broken.json"
broken_status=$?
set -e

if [[ $lint_status -ne 1 || $broken_status -ne 2 ]]; then
    printf 'unexpected smoke exit codes: lint=%s broken=%s\n' "$lint_status" "$broken_status" >&2
    exit 1
fi

"$python_command" - "$smoke_root" <<'PYTHON'
import json
import pathlib
import sys

root = pathlib.Path(sys.argv[1])
clean = json.loads((root / "clean.json").read_text())
lint = json.loads((root / "lint.json").read_text())
broken = json.loads((root / "broken.json").read_text())
for report in (clean, lint, broken):
    assert report["schema_version"] == 1
    assert report["summary"]["selected_files"] == 1
assert clean["summary"]["analysis_completed"]
assert clean["diagnostics"] == []
assert lint["summary"]["analysis_completed"]
assert any(d["rule"] == "FC001" and d["severity"] == "error" for d in lint["diagnostics"])
assert not broken["summary"]["analysis_completed"]
assert any(d["rule"] == "EF000" and d["category"] == "parsing" for d in broken["diagnostics"])
print("Standalone release smoke passed (no Cargo, rustc, or parser installation on PATH).")
PYTHON
