#!/bin/sh
# Installs time-cron @VERSION@ into ~/.local/bin after checking the archive's SHA-256.
# curl -fsSL https://github.com/nowmyagentcan/time-cron/releases/latest/download/install.sh | sh
set -eu
version=@VERSION@
base=${TIME_CRON_BASE:-https://github.com/nowmyagentcan/time-cron/releases/download/$version}
case "$(uname -s)-$(uname -m)" in
  Linux-x86_64) target=linux-x64 ;;
  Linux-aarch64 | Linux-arm64) target=linux-arm64 ;;
  Darwin-x86_64) target=macos-x64 ;;
  Darwin-arm64) target=macos-arm64 ;;
  *) echo "time-cron: no build for $(uname -s) $(uname -m)." >&2; exit 1 ;;
esac
archive=time-cron-$version-$target.tar.gz
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
curl -fsSL -o "$tmp/$archive" "$base/$archive"
curl -fsSL -o "$tmp/SHA256SUMS" "$base/SHA256SUMS"
expected=$(awk -v f="$archive" '$2 == f { print $1 }' "$tmp/SHA256SUMS")
if command -v sha256sum >/dev/null; then actual=$(sha256sum "$tmp/$archive"); else actual=$(shasum -a 256 "$tmp/$archive"); fi
if [ -z "$expected" ] || [ "${actual%% *}" != "$expected" ]; then
  echo "time-cron: the SHA-256 of $archive doesn't match SHA256SUMS; nothing was installed." >&2
  exit 1
fi
tar -xzf "$tmp/$archive" -C "$tmp" time-cron
dir=$HOME/.local/bin
mkdir -p "$dir"
mv "$tmp/time-cron" "$dir/time-cron"
"$dir/time-cron" --version
case ":$PATH:" in
  *":$dir:"*) ;;
  *) echo "Add $dir to your PATH: export PATH=\"\$HOME/.local/bin:\$PATH\"" ;;
esac
