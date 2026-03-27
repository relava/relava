#!/usr/bin/env bash
#
# Build the Relava GUI and deploy to ~/.relava/gui/.
#
# Usage:
#   ./gui/scripts/deploy.sh            # Build and copy to ~/.relava/gui/
#   ./gui/scripts/deploy.sh /path/dir  # Build and copy to custom directory
#
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
GUI_DIR="$(dirname "$SCRIPT_DIR")"
DEST="${1:-$HOME/.relava/gui}"

echo "Building GUI..."
(cd "$GUI_DIR" && npm run build)

echo "Deploying to $DEST..."
mkdir -p "$DEST"
# Remove old assets but keep the directory
rm -rf "${DEST:?}"/*
cp -r "$GUI_DIR/dist/"* "$DEST/"

echo "Done. GUI deployed to $DEST"
