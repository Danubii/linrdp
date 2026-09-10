# Arch packaging

`PKGBUILD` packages the published v0.2.0 x86_64 Linux binary. The checked-in
checksum matches the GitHub release asset. No Rust compilation is needed.

## Install

With `git` and `base-devel` installed, clone the repository, review the
`PKGBUILD`, then build and install:

```sh
git clone https://github.com/zeq0r/fjern.git
cd fjern/packaging/arch
makepkg -si
```

The package includes the executable, desktop launcher, SVG icon, license and
release README. Runtime dependencies are declared in [PKGBUILD](PKGBUILD).

## Validate

From this directory, with `namcap` installed:

```sh
makepkg --clean --cleanbuild --force
namcap PKGBUILD fjern-0.2.0-1-x86_64.pkg.tar.zst
```

## Maintain a release

After publishing a release, download its archive and accompanying checksum.
Verify the archive, update `pkgver` and `sha256sums`, then regenerate `.SRCINFO`
with `makepkg --printsrcinfo > .SRCINFO`. Build with a fresh source directory to
exercise the published download URL and checksum.

Use the digest of the **published asset**, not a local build. Archive ordering
and timestamps are deterministic, but binaries built with different toolchains
and system libraries can differ. CI's local release candidate checks packaging
independently; it does not establish the published asset's checksum.

The package does not modify desktop configuration or require a particular
Linux desktop distribution.
