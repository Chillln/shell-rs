#!/usr/bin/env bash
# Write the signed update manifest of one release for one channel.
#
#   packaging/make-manifest.sh <version> <channel> <notes.md> <dist-dir> <out.json>
#
# <dist-dir> holds the release's packages under their published names.
# MINISIGN_KEY_FILE is the secret key; MINISIGN_PASSWORD its password, if it
# has one. SHELLRS_MIRROR_BASE, when set (e.g. the GitHub release download
# URL of a public repository), adds a second address for every package.
#
# The output is the envelope src/update/manifest.rs reads: the manifest's
# exact text and its minisign signature, whose trusted comment
# `shellrs-manifest <channel> <version>` the client checks. The format only
# ever gains fields; packaging/manifest.example.json shows it.
set -euo pipefail

version=$1
channel=$2
notes=$3
dist=$4
out=$5

base="https://dl.shellrs.com/releases/$version"
mirror=${SHELLRS_MIRROR_BASE:-}

mac_arm="ShellRS-$version-macos-aarch64.app.zip"
mac_intel="ShellRS-$version-macos-x86_64.app.zip"
dmg_arm="ShellRS-$version-macos-aarch64.dmg"
dmg_intel="ShellRS-$version-macos-x86_64.dmg"
win="ShellRS-$version-windows-x86_64-setup.exe"
linux="ShellRS-$version-linux-x86_64.AppImage"

sha256() {
    if command -v sha256sum >/dev/null; then
        sha256sum "$1" | cut -d' ' -f1
    else
        shasum -a 256 "$1" | cut -d' ' -f1
    fi
}

# 构建单个软件包的 asset JSON 条目
asset() {
    local file="$dist/$1"
    local size
    size=$(wc -c <"$file" | tr -d ' ')
    jq -n \
        --arg url "$base/$1" \
        --arg mirror "${mirror:+$mirror/$1}" \
        --argjson size "$size" \
        --arg sha256 "$(sha256 "$file")" \
        '{urls: ([$url] + (if $mirror == "" then [] else [$mirror] end)), size: $size, sha256: $sha256}'
}

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

# 初始化空的 assets 和 installers 映射
assets="{}"
installers="{}"

# 仅当对应平台的软件包实际存在时，才写入清单
if [ -f "$dist/$mac_arm" ]; then
    assets=$(jq --argjson a "$(asset "$mac_arm")" '. + {"macos-aarch64": $a}' <<<"$assets")
    installers=$(jq --arg u "$base/$dmg_arm" '. + {"macos-aarch64": $u}' <<<"$installers")
fi

if [ -f "$dist/$mac_intel" ]; then
    assets=$(jq --argjson a "$(asset "$mac_intel")" '. + {"macos-x86_64": $a}' <<<"$assets")
    installers=$(jq --arg u "$base/$dmg_intel" '. + {"macos-x86_64": $u}' <<<"$installers")
fi

if [ -f "$dist/$win" ]; then
    assets=$(jq --argjson a "$(asset "$win")" '. + {"windows-x86_64": $a}' <<<"$assets")
    installers=$(jq --arg u "$base/$win" '. + {"windows-x86_64": $u}' <<<"$installers")
fi

if [ -f "$dist/$linux" ]; then
    assets=$(jq --argjson a "$(asset "$linux")" '. + {"linux-x86_64": $a}' <<<"$assets")
    installers=$(jq --arg u "$base/$linux" '. + {"linux-x86_64": $u}' <<<"$installers")
fi

jq -n -j \
    --arg version "$version" \
    --arg channel "$channel" \
    --arg published "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
    --rawfile notes "$notes" \
    --arg notes_url "https://shellrs.com/changelog#$version" \
    --argjson assets "$assets" \
    --argjson installers "$installers" \
    '{
        schema: 1,
        channel: $channel,
        version: $version,
        published_at: $published,
        minimum_version: null,
        rollout: 1.0,
        notes: $notes,
        notes_url: $notes_url,
        assets: $assets,
        installers: $installers
    }' >"$work/manifest.json"

printf '%s\n' "${MINISIGN_PASSWORD:-}" | minisign -S \
    -s "$MINISIGN_KEY_FILE" \
    -m "$work/manifest.json" \
    -x "$work/manifest.json.minisig" \
    -t "shellrs-manifest $channel $version"

jq -n \
    --rawfile manifest "$work/manifest.json" \
    --rawfile signature "$work/manifest.json.minisig" \
    '{manifest: $manifest, signature: $signature}' >"$out"
echo "$out"
