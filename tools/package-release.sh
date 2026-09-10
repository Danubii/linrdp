#!/usr/bin/env bash
set -euo pipefail

version="${1:-v$(cargo metadata --no-deps --format-version 1 | sed -n 's/.*"name":"fjern","version":"\([^"]*\)".*/\1/p')}"
target="${2:-x86_64-linux}"
output="${3:-dist}"
archive="fjern-${version}-${target}"

if [[ ! "$version" =~ ^v[0-9]+\.[0-9]+\.[0-9]+([.-][0-9A-Za-z.-]+)?$ ]]; then
  echo "invalid release version: $version" >&2
  exit 2
fi

mkdir -p "$output"
staging="$(mktemp -d "${TMPDIR:-/tmp}/fjern-release.XXXXXX")"
trap 'rm -rf "$staging"' EXIT

cargo build --workspace --release --locked
install -Dm755 target/release/fjern "$staging/$archive/fjern"
install -Dm644 contrib/fjern.desktop "$staging/$archive/fjern.desktop"
install -Dm644 contrib/icons/fjern.svg "$staging/$archive/fjern.svg"
install -Dm644 LICENSE "$staging/$archive/LICENSE"
install -Dm644 README.md "$staging/$archive/README.md"
tar --sort=name --mtime='UTC 1970-01-01' --owner=0 --group=0 --numeric-owner \
  -C "$staging" -czf "$output/$archive.tar.gz" "$archive"
(cd "$output" && sha256sum "$archive.tar.gz" > "$archive.tar.gz.sha256")
