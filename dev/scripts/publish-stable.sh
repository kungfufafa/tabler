#!/usr/bin/env bash
set -euo pipefail

tag=$1
shift
version=${tag#v}

for asset in "Tabler_${version}_universal.dmg" "Tabler_${version}_x64-setup.exe" "Tabler_${version}_x64_en-US.msi"; do
  if [ ! -s "installers/$asset" ]; then
    printf 'Missing release installer: %s\n' "$asset" >&2
    exit 1
  fi
done

# GitHub CLI creates a draft while it uploads assets. Resume an interrupted upload, and leave
# completed releases alone on reruns.
if draft=$(gh release view "$tag" --json isDraft --jq .isDraft 2>/dev/null); then
  if [ "$draft" = true ]; then
    gh release upload "$tag" installers/* --clobber
    gh release edit "$tag" --draft=false --prerelease=false --verify-tag --latest
  fi
else
  gh release create "$tag" installers/* --verify-tag --latest --title "Tabler $version" "$@"
fi
