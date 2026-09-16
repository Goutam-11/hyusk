#!/usr/bin/env bash
# Build and install the native Hyusk Workflows application for this user.
set -euo pipefail

PROJECT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
WORKSPACE_DIR="$(cd "$PROJECT_DIR/.." && pwd)"
BINARY="$WORKSPACE_DIR/target/release/hyusk-workflows"
BIN_DIR="${HOME}/.local/bin"
APPLICATIONS_DIR="${XDG_DATA_HOME:-$HOME/.local/share}/applications"

echo "Building Hyusk Workflows..."
cargo build --release --manifest-path "$PROJECT_DIR/Cargo.toml" --bin hyusk-workflows

mkdir -p "$BIN_DIR" "$APPLICATIONS_DIR"
install -m 0755 "$BINARY" "$BIN_DIR/hyusk-workflows"
install -m 0644 \
  "$PROJECT_DIR/data/io.github.hyusk.Workflows.desktop" \
  "$APPLICATIONS_DIR/io.github.hyusk.Workflows.desktop"

if command -v update-desktop-database >/dev/null 2>&1; then
  update-desktop-database "$APPLICATIONS_DIR" >/dev/null 2>&1 || true
fi

echo "Installed Hyusk Workflows. Open it from the app grid or run:"
echo "  hyusk-workflows"
