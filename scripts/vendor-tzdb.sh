#!/usr/bin/env bash
# Regenerates vendor/jiff-tzdb from an IANA tzdata release: scripts/vendor-tzdb.sh 2026f
# Needs curl, gpg, make, cc and cargo (Linux or macOS).
set -euo pipefail
v=${1:?usage: vendor-tzdb.sh <tzdata version>}
jiff_rev=0.2.38
out=$(cd "$(dirname "$0")/../vendor/jiff-tzdb" && pwd)
work=$(mktemp -d); trap 'rm -rf "$work"' EXIT; cd "$work"

for f in "tzdata$v.tar.gz" "tzdata$v.tar.gz.asc" "tzcode$v.tar.gz"; do
  curl -fsSLO "https://data.iana.org/time-zones/releases/$f"
done
gpg --keyserver hkps://keys.openpgp.org --recv-keys 7E3792A9D8ACF7D633BC1588ED97E90E62AA7E34
gpg --verify "tzdata$v.tar.gz.asc" "tzdata$v.tar.gz"
mkdir tz && tar -xzf "tzdata$v.tar.gz" -C tz && tar -xzf "tzcode$v.tar.gz" -C tz
make -s -C tz zic

git clone -q https://github.com/BurntSushi/jiff.git && git -C jiff checkout -q "$jiff_rev"
cd jiff
cargo run -q --release -p jiff-cli -- generate zoneinfo --zic "$work/tz/zic" "$work/tz" "$work/zoneinfo"
cargo run -q --release -p jiff-cli -- generate jiff-tzdb "$work/zoneinfo"
cp crates/jiff-tzdb/tzname.rs crates/jiff-tzdb/concatenated-zoneinfo.dat "$out/"
sed -i.bak "s/tzdata [0-9a-z]*\./tzdata $v./" "$out/Cargo.toml" && rm "$out/Cargo.toml.bak"
echo "vendor/jiff-tzdb now carries tzdata $v"
