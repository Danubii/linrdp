# Arch packaging

`PKGBUILD` packages the tagged x86_64 binary release. Before publishing a tag,
build the archive in the same CI environment used for the release and replace
`sha256sums` with the digest of the resulting asset:

```sh
tools/package-release.sh v0.2.0
sha256sum dist/fjern-v0.2.0-x86_64-linux.tar.gz
```

The checked-in checksum validates the current local release candidate. It must
be compared with the uploaded GitHub asset before this PKGBUILD is submitted to
any package repository.

For a pre-release local package check, place the archive in `SRCDEST` and run:

```sh
cd packaging/arch
SRCDEST="$PWD/../../dist" makepkg --clean --cleanbuild --force
namcap PKGBUILD fjern-*.pkg.tar.zst
```

The package installs no Omarchy-specific configuration and has no Omarchy
runtime dependency.
