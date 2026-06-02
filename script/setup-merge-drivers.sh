#!/usr/bin/env bash
# Register openWarp custom merge driver + enable rerere.
# Run once after first clone. Subsequent upstream merges (merge / cherry-pick / rebase) will:
# 1. Automatically keep local versions for paths marked with merge=waz-ours in .gitattributes
# 2. Let rerere record conflict resolutions, so identical conflicts are auto-resolved in the future
set -euo pipefail

git config merge.waz-ours.name "Always keep openWarp version (custom driver)"
git config merge.waz-ours.driver true
git config rerere.enabled true
git config rerere.autoupdate true

echo "openWarp merge drivers + rerere configured."
echo "  rerere.enabled        = $(git config --get rerere.enabled)"
echo "  rerere.autoupdate     = $(git config --get rerere.autoupdate)"
echo "  merge.waz-ours   = $(git config --get merge.waz-ours.driver)"
