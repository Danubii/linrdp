# Arch packaging

`PKGBUILD` builds the `fjern-bin` package from the published v0.2.1 x86_64 Linux binary. The checked-in
checksum matches the GitHub release asset. No Rust compilation is needed.

## Install

With `git` and `base-devel` installed, clone the repository, review the
`PKGBUILD`, then build and install:

```sh
git clone https://github.com/zeq0r/fjern.git
cd fjern/packaging/arch
makepkg -si
```

The package includes the executable, desktop launcher, SVG icon, license,
third-party notices and release README. The command remains `fjern`;
`provides` and `conflicts` prevent co-installation with another `fjern` package.
Runtime dependencies are declared in [PKGBUILD](PKGBUILD).

## Validate

From this directory, with `namcap` installed:

```sh
makepkg --clean --cleanbuild --force
namcap PKGBUILD fjern-bin-0.2.1-1-x86_64.pkg.tar.zst
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

## Release notices

`tools/collect-licenses.py` collects license, copyright and notice files from
the locked x86_64 Linux Cargo dependency sources, including nested native code
and a conservative superset of build/test dependencies. It also includes Rust
standard-library notices from the release toolchain. Missing crate notices
fail the release; version-specific supplements live in `tools/license-supplements`.
The collector requires Python 3 when building release archives, not when
installing `fjern-bin`. Run its regressions with:

```sh
python3 tools/test_collect_licenses.py
```

Installed notices live in `/usr/share/licenses/fjern-bin/third-party-licenses`.
