#!/usr/bin/env bash
# Installs or updates GlazeWM on macOS from the latest GitHub release.
#
# The app is only ad-hoc signed. Downloading via `curl` doesn't set the
# quarantine attribute, so Gatekeeper doesn't block the app on launch.
#
# Usage: curl -fsSL https://raw.githubusercontent.com/SigmaGrindset/glazewm/main/resources/scripts/install.sh | bash

set -euo pipefail

REPO="${GLAZEWM_REPO:-SigmaGrindset/glazewm}"
DMG_URL="https://github.com/$REPO/releases/latest/download/glazewm-macos.dmg"
BUNDLE_ID="io.glzr.glazewm"
INSTALL_DIR="/Applications"
APP_PATH="$INSTALL_DIR/GlazeWM.app"

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "This script only supports macOS." >&2
  exit 1
fi

tmp_dir="$(mktemp -d)"
mount_dir="$tmp_dir/mount"
is_mounted=false

# Detaches the DMG and removes temporary files.
cleanup() {
  if [[ "$is_mounted" == true ]]; then
    hdiutil detach "$mount_dir" -quiet || true
  fi

  rm -rf "$tmp_dir"
}

trap cleanup EXIT

# Runs the given command, using `sudo` if the install location isn't
# writable by the current user (e.g. for non-admin users).
run_privileged() {
  if [[ -w "$INSTALL_DIR" && ( ! -e "$APP_PATH" || -O "$APP_PATH" ) ]]; then
    "$@"
  else
    sudo "$@"
  fi
}

echo "Downloading $DMG_URL..."
curl -fL --progress-bar -o "$tmp_dir/glazewm.dmg" "$DMG_URL"

mkdir -p "$mount_dir"
hdiutil attach "$tmp_dir/glazewm.dmg" -nobrowse -readonly -quiet -mountpoint "$mount_dir"
is_mounted=true

# Gracefully exit the running instance, so that it runs its shutdown
# cleanup and the new version can be launched.
if pgrep -xq glazewm; then
  echo "Exiting running GlazeWM instance..."
  "$mount_dir/GlazeWM.app/Contents/MacOS/glazewm-cli" command wm-exit > /dev/null || true

  for _ in {1..20}; do
    pgrep -xq glazewm || break
    sleep 0.5
  done

  if pgrep -xq glazewm; then
    echo "GlazeWM is still running. Quit it manually and re-run this script." >&2
    exit 1
  fi
fi

echo "Installing to $APP_PATH..."
run_privileged rm -rf "$APP_PATH"
run_privileged ditto "$mount_dir/GlazeWM.app" "$APP_PATH"

# Ad-hoc signatures change with every build, so an accessibility permission
# granted to a previous build no longer applies. Clear the stale entry, so
# that macOS prompts for the permission again.
tccutil reset Accessibility "$BUNDLE_ID" > /dev/null 2>&1 || true

echo "Launching GlazeWM..."
open "$APP_PATH"

echo
echo "Done. On first launch after an install or update, enable GlazeWM under"
echo "System Settings > Privacy & Security > Accessibility, then launch it again."
