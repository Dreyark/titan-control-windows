#!/bin/sh
# Regenerates the pre-generated UI file used by build.rs when blueprint-compiler is not installed (Windows builds).
# Run it from the repository root after every change of src/ui/window.blp and commit src/ui/window.ui.
set -eu
# blueprint-compiler reads the file with the locale encoding on Windows; force UTF-8.
export PYTHONUTF8=1
cd "$(dirname "$0")/.."
"${BLUEPRINT_COMPILER:-blueprint-compiler}" compile src/ui/window.blp --output src/ui/window.ui
echo "Wrote src/ui/window.ui"
