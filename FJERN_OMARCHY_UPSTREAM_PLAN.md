# Fjern — Rebrand, Packaging & Omarchy Upstream Readiness Plan

> **Project:** `zeq0r/linrdp` → **Fjern**
> **Current release:** `v0.1.0` (`LinRDP v0.1.0`)
> **Target release after rebrand:** `v0.2.0` (`Fjern v0.2.0`)
> **Primary platform:** Linux / Wayland, developed and validated first on Omarchy + Hyprland
> **Protocols:** RDP + VNC
> **Language:** Rust
> **License:** MIT
> **Goal:** Make Fjern a polished, secure, installable standalone Linux remote-desktop client that is ready to be proposed for inclusion in `omacom/omarchy-pkgs`.

---

## 0. Mission and fixed decisions

Implement the work in this document, not merely a proposal.

The product identity is now:

```text
Fjern
Native remote desktop for Linux.

RDP and VNC. Wayland first. Developed first for Omarchy.
```

The project must remain **independent of Omarchy**. Omarchy is the primary development and validation target, but Fjern must not require Omarchy to run.

### Naming decisions

| Scope | Name |
|---|---|
| Product | `Fjern` |
| CLI executable | `fjern` |
| Main application crate | `fjern` |
| GitHub repository | `zeq0r/fjern` |
| Arch package | `fjern` |
| Desktop application | `Fjern` |
| New config directory | `$XDG_CONFIG_HOME/fjern` or `~/.config/fjern` |
| Existing RDP protocol crate | **keep `linrdp-proto`** |

Do **not** rename `linrdp-proto` merely for branding. It implements the RDP protocol and the name is technically accurate.

Do not rewrite or delete the existing `v0.1.0` release. Keep it as historical evidence of the pre-rebrand project.

Preferred project description:

> Fjern is a native Linux remote-desktop client for RDP and VNC, developed first for Omarchy and its Wayland/Hyprland desktop.

Always keep the disclaimer:

> Fjern is an independent project and is not an official Omarchy component.

Avoid unverified claims about X11, multi-monitor, Windows Server, ARM64, H.264 stability, hardware acceleration or protocol compatibility.

---

# 1. Agent working method

Create a dedicated branch:

```bash
git checkout -b rebrand/fjern-upstream-readiness
```

Run and record the baseline before changing anything:

```bash
cargo build --workspace --locked
cargo test --workspace --locked
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
```

Enumerate all existing branding references:

```bash
rg -n --hidden \
  --glob '!target/**' \
  --glob '!.git/**' \
  'LinRDP|linrdp|LINRDP|zeq0r/linrdp'
```

Do not blindly replace every match. Categorize first:

1. Product/application identity → rename to Fjern.
2. Executable/package/desktop identity → rename to `fjern`.
3. User config/state path → migrate safely.
4. `linrdp-proto` technical crate/module → keep.
5. Historical changelog/release reference → normally keep.
6. Protocol-specific comments/test names → only rename if they refer to the application.
7. Vendored/upstream identifiers → preserve where required.

Use small, reviewable commits.

Suggested commits:

```text
refactor: rename application from LinRDP to Fjern
feat: migrate legacy LinRDP config to Fjern
docs: rebrand project and improve product presentation
build: add Arch Linux packaging
ci: validate Fjern on Arch Linux
docs: document vendored dependencies and security policy
build: add reproducible Fjern release workflow
docs: add Omarchy validation matrix and upstream guide
```

Do not mix unrelated protocol feature development into this branch unless needed to fix a regression.

---

# 2. Rename the application

## 2.1 Workspace layout

Current:

```text
crates/linrdp-proto
crates/linrdp
```

Target:

```text
crates/linrdp-proto
crates/fjern
```

Rename:

```bash
git mv crates/linrdp crates/fjern
```

Update root `Cargo.toml` workspace members accordingly.

Keep `linrdp-proto` as the RDP protocol-engine crate.

## 2.2 Cargo metadata

In `crates/fjern/Cargo.toml`:

- Change package name from `linrdp` to `fjern`.
- Update description.
- Update repository URL after GitHub rename.
- Keep MIT license.
- Keep `publish = false` unless crates.io publication becomes an explicit goal.

Suggested description:

```toml
description = "Native Linux remote desktop client for RDP and VNC, developed first for Omarchy"
```

Ensure:

```bash
cargo metadata --no-deps
```

shows `fjern` as the application package and `linrdp-proto` as the protocol package.

## 2.3 Binary and CLI

The installed executable must become:

```bash
fjern
```

Commands should work as:

```bash
fjern
fjern tui
fjern connect HOST
fjern vnc HOST
fjern probe HOST
fjern tls HOST
fjern nla-probe HOST
fjern login HOST
fjern session-probe HOST
```

Update:

- help text
- clap/manual argument parser strings
- usage examples
- error messages
- tests
- docs
- launcher command
- release scripts
- packaging

Do not retain a second `linrdp` binary indefinitely.

Optionally provide a **temporary compatibility shim** for one release only if it materially helps migration:

```bash
linrdp -> fjern
```

If implemented, clearly mark it deprecated and remove it in a later release.

---

# 3. Preserve user data during rename

This is critical.

Current state is stored under:

```text
~/.config/linrdp/
```

including at least:

```text
profiles.json
known_hosts.json
```

Target:

```text
~/.config/fjern/
```

## 3.1 Migration requirements

Implement non-destructive migration.

Rules:

1. If `~/.config/fjern/` exists, use it.
2. If Fjern config does not exist but legacy `~/.config/linrdp/` exists, migrate or import it.
3. Never silently overwrite an existing Fjern config.
4. Preserve file permissions.
5. Preserve trust decisions exactly.
6. Never weaken certificate validation during migration.
7. Never merge conflicting certificate pins automatically.
8. Do not delete the legacy config until a migration has completed safely.
9. Migration must be idempotent.
10. Add tests for migration scenarios.

Recommended approach:

- Create `~/.config/fjern/` with `0700`.
- Atomically copy/import supported files.
- Preserve config file mode `0600`.
- Validate imported JSON before activation.
- If both old and new state exist, prefer new state and report that legacy state was left untouched.
- Optionally leave a migration marker such as:

```text
~/.config/fjern/.migrated-from-linrdp
```

Do not include secrets in logs.

## 3.2 Tests

Add tests for:

- no legacy config
- successful migration
- migration rerun
- existing Fjern config
- corrupt legacy JSON
- unsafe permissions
- symlinked legacy files
- conflicting certificate pin state
- interrupted migration
- partial target directory

---

# 4. Desktop integration

Rename:

```text
contrib/linrdp.desktop
```

to:

```text
contrib/fjern.desktop
```

Suggested content:

```ini
[Desktop Entry]
Type=Application
Name=Fjern
Comment=Native remote desktop client for RDP and VNC
Exec=fjern
Terminal=true
Categories=Network;RemoteAccess;
Keywords=RDP;VNC;Remote Desktop;Wayland;Omarchy;
StartupNotify=true
```

Adjust `Terminal=` if/when a graphical connection manager replaces the TUI.

Add a proper application icon before upstream submission.

Recommended asset paths:

```text
contrib/icons/fjern.svg
contrib/icons/fjern-128.png
contrib/icons/fjern-256.png
```

Prefer SVG as source of truth.

If an application ID / Wayland app-id is available, use a stable identity and document it.

---

# 5. README rework

The README currently contains strong technical content but should lead with the product.

Target structure:

```text
# Fjern

Native remote desktop for Linux.

[screenshot / short demo]

RDP + VNC
Wayland first
Developed first for Omarchy

## Why Fjern?
## Features
## Install
## Quick start
## Omarchy / Hyprland
## RDP
## VNC / WayVNC
## Security and certificate trust
## Current limitations
## Development
## Architecture
## Contributing
```

Opening copy should be concise:

> Fjern is a native Linux remote-desktop client for RDP and VNC. It is developed first for Omarchy and Hyprland, with native Wayland behavior, compositor shortcut capture, dynamic resolution, clipboard integration and a small keyboard-first connection interface.

Then:

> Fjern is an independent project and is not an official Omarchy component.

Move deep diagnostics (`probe`, `tls`, CredSSP, MCS/GCC) further down or into docs.

Keep the technical documentation; only improve information hierarchy.

---

# 6. Add a SECURITY.md

Create:

```text
SECURITY.md
```

Cover:

- how to privately report vulnerabilities
- supported versions
- credential-handling principles
- certificate trust model
- remote input is untrusted
- remote clipboard is untrusted
- file transfer boundaries
- no silent TLS downgrade
- no credentials in command-line arguments
- no secrets in logs
- config/trust files require private permissions
- parser allocations and lengths are bounded
- unsafe Rust remains forbidden in project crates unless an explicit future exception is reviewed

Do not put a personal email address in the document unless intentionally desired.

Prefer GitHub Security Advisories as the private reporting path if enabled.

---

# 7. Document vendored dependencies

The workspace currently patches:

```toml
[patch.crates-io]
minifb = { path = "vendor/minifb" }
vnc-rs = { path = "vendor/vnc-rs" }
```

Create:

```text
vendor/README.md
```

For every vendored dependency document:

```text
Name:
Upstream:
Pinned upstream version/commit:
Why it is vendored:
Local modifications:
Tests covering the patch:
Upstream issue/PR:
Conditions for removing the vendor:
License:
```

Goals:

- A distro maintainer must understand why the patch exists.
- No hidden dependency.
- No accidental fork with undocumented behavior.
- Make upstreaming patches the preferred long-term path.

Verify licenses for all vendored code and keep license notices where required.

---

# 8. Arch Linux packaging

This is P0 for Omarchy.

Create:

```text
packaging/arch/PKGBUILD
```

Decide between two package strategies.

## Preferred initial strategy: binary release package

Publish a release artifact from Fjern upstream and package that artifact.

Target release artifact naming:

```text
fjern-v0.2.0-x86_64-linux.tar.gz
fjern-v0.2.0-x86_64-linux.tar.gz.sha256
```

Longer term add:

```text
fjern-v0.2.0-aarch64-linux.tar.gz
fjern-v0.2.0-aarch64-linux.tar.gz.sha256
```

Archive should contain at minimum:

```text
fjern
fjern.desktop
LICENSE
README.md
```

Prefer also:

```text
fjern.svg
```

The PKGBUILD must:

- install executable to `/usr/bin/fjern`
- install desktop file to `/usr/share/applications/fjern.desktop`
- install icon to the appropriate hicolor path
- install license under `/usr/share/licenses/fjern/`
- use fixed checksums
- declare all runtime dependencies
- pass `namcap` review, with documented exceptions
- build/install successfully in a clean Arch environment

## Source package option

Also consider `fjern-git` for AUR development use, but do not make `-git` the primary package once tagged releases exist.

---

# 9. Release workflow

Add or extend GitHub Actions to produce release artifacts.

Requirements:

- build with `--locked`
- run fmt
- run clippy with `-D warnings`
- run all tests
- build release binary
- bundle `.desktop`, icon, license and README
- generate SHA-256 checksum
- upload release assets on version tags
- fail the release if tests fail
- do not release from arbitrary unreviewed workspace state

Suggested tag:

```text
v0.2.0
```

Suggested release title:

```text
Fjern v0.2.0
```

Release notes must explicitly mention:

```text
LinRDP has been renamed to Fjern.
```

Include migration notes for existing `~/.config/linrdp`.

---

# 10. Add Arch CI

Existing Ubuntu CI should remain.

Add an Arch Linux job/container that validates:

```bash
cargo build --workspace --release --locked
cargo test --workspace --locked
```

and package creation:

```bash
makepkg --syncdeps --noconfirm
```

Then inspect the built package:

```bash
namcap PKGBUILD
namcap fjern-*.pkg.tar.zst
```

Where practical, install the generated package into a clean test container/VM and verify:

```bash
fjern --help
```

A real Wayland viewer test cannot be fully represented by a basic CI container, so document what remains part of manual Omarchy validation.

---

# 11. Omarchy validation matrix

Create:

```text
docs/omarchy.md
```

or:

```text
docs/platform-validation.md
```

Maintain a table that distinguishes:

- implemented
- locally tested
- verified on real host
- experimental
- not supported

Example:

| Feature | Omarchy/Hyprland | Other Wayland | X11 | Notes |
|---|---|---|---|---|
| RDP Windows desktop | Verified | Unknown/Tested | Unknown | Real Windows host |
| VNC WayVNC | Verified | Compatible target | N/A | Real Omarchy host |
| Keyboard | Verified | Expected | Needs validation | Include Danish layout |
| Super key capture | Verified | Compositor dependent | N/A | Omarchy/Hyprland |
| Mouse/scroll | Verified | Expected | Needs validation | |
| Text clipboard | Verified | Expected | Needs validation | |
| RDP file clipboard | Verified | Expected | Needs validation | |
| Dynamic RDP resolution | Verified | Expected | Needs validation | |
| VNC resize/scaling | Verified | Expected | Needs validation | |
| H.264 / AVC420 | Experimental | Experimental | Experimental | Software decode |
| Multi-monitor | Not supported | Not supported | Not supported | Do not claim |
| ARM64 | Not verified until tested | | | |

Do not mark a row "verified" based only on protocol/unit tests.

---

# 12. Omarchy-specific quality requirements

Fjern should feel native on Omarchy without hard-depending on it.

Validate:

- Hyprland focus behavior
- Super key forwarding/capture
- Alt+Tab behavior inside remote session
- local escape path from keyboard capture
- window resize
- fullscreen behavior
- floating/tiled behavior
- HiDPI scaling
- mixed-scale monitors if possible
- Danish keyboard layout
- clipboard text
- clipboard files
- reconnect after network drop
- certificate warning flows
- WayVNC behavior
- correct cursor behavior
- clean disconnect
- no accidental Windows logout on close
- launch from app launcher
- launch from terminal
- no required shell-specific config

Optional Omarchy integration helpers must be additive and isolated.

Do not hard-code Omarchy config paths unless the functionality is explicitly Omarchy-only.

---

# 13. Product UX improvements before upstream pitch

P0:

- clean application naming
- stable CLI name
- readable errors
- clear certificate prompts
- connection profiles
- reliable reconnect behavior
- clean launch via `.desktop`

P1:

- product icon
- screenshot/GIF in README
- connection profile selection polish
- friendly errors for common RDP/VNC failures
- version output:

```bash
fjern --version
```

Example:

```text
fjern 0.2.0
```

P2:

- graphical connection manager
- secret-service credential storage
- multi-monitor
- hardware accelerated H.264 decode
- broader Wayland compositor validation
- X11 validation

Do not block Omarchy package submission on every P2 feature.

---

# 14. Credential storage

Current profiles must remain non-secret.

If password persistence is added:

- opt-in only
- use the desktop secret service / Secret Service API
- never store passwords in `profiles.json`
- never write passwords to logs
- never expose password via argv
- do not invent custom reversible encryption backed by local config
- use a stable opaque credential identifier in the profile if needed

Add security tests around this before release.

---

# 15. Repository hygiene

Before upstream submission ensure the repository has:

```text
README.md
LICENSE
CONTRIBUTING.md
SECURITY.md
CHANGELOG.md
Cargo.toml
Cargo.lock
.github/workflows/
packaging/arch/PKGBUILD
contrib/fjern.desktop
contrib/icons/fjern.svg
vendor/README.md
docs/architecture.md
docs/platform-validation.md
```

Recommended GitHub repository metadata:

**Name**

```text
fjern
```

**Description**

```text
Native Linux remote desktop client for RDP and VNC. Wayland first, developed first for Omarchy.
```

Suggested topics:

```text
rdp
vnc
remote-desktop
linux
rust
wayland
hyprland
omarchy
wayvnc
```

Rename the repository through GitHub rather than creating an unrelated new repository, so GitHub redirects old URLs.

After rename, update every canonical URL in Cargo metadata, docs, badges and release scripts.

---

# 16. Naming migration audit

Run after rebrand:

```bash
rg -n --hidden \
  --glob '!target/**' \
  --glob '!.git/**' \
  'LinRDP|zeq0r/linrdp'
```

Every remaining match must be intentional.

Then:

```bash
rg -n --hidden \
  --glob '!target/**' \
  --glob '!.git/**' \
  '\blinrdp\b'
```

Expected remaining matches may include:

```text
linrdp-proto
historical changelog entries
legacy config migration code/tests
historical v0.1.0 references
```

Everything else should normally be renamed.

---

# 17. Backward compatibility tests

Specifically test that a user upgrading from v0.1.0 to v0.2.0:

1. installs Fjern
2. runs `fjern`
3. retains connection profiles
4. retains certificate trust state
5. does not need to manually move files
6. does not silently overwrite existing data
7. receives no misleading certificate warning due solely to rename
8. can remove legacy config manually after confirmed migration

Document the upgrade behavior in release notes.

---

# 18. Distribution verification

Before proposing inclusion:

## Clean Arch test

Use a clean Arch VM/container.

Verify:

```bash
makepkg -si
fjern --version
fjern --help
```

Verify launcher installation.

## Clean Omarchy test

Use a clean/current Omarchy installation.

Verify:

```text
install package
launch from application launcher
create profile
RDP to real Windows host
VNC to real WayVNC host
resize
keyboard
Super key
mouse
scroll
clipboard
disconnect
reconnect
restart Fjern
saved profile persistence
remembered certificate persistence
```

Record exact:

- Omarchy version
- Hyprland version
- kernel version
- Windows host version
- WayVNC version
- Fjern commit/tag

---

# 19. Omarchy package repository integration

The target is initially:

```text
omacom/omarchy-pkgs
```

Do not initially ask Omarchy core to replace FreeRDP or make Fjern a default system dependency.

First objective:

> Make Fjern installable through the Omarchy package ecosystem.

Omarchy package metadata will likely need a directory conceptually similar to:

```text
pkgbuilds/fjern/
├── PKGBUILD
└── .omarchy/
    └── package.json
```

Use the current `omarchy-pkgs` conventions at submission time.

Prefer their GitHub release-upstream mechanism once Fjern ships stable release assets.

Potential metadata direction:

```json
{
  "source": "local",
  "upstream": {
    "github": "zeq0r/fjern",
    "digests": true,
    "assets": {
      "x86_64": "fjern-v{pkgver}-x86_64-linux.tar.gz"
    }
  }
}
```

Do not copy this blindly. Match the exact current Omarchy package schema when preparing the PR.

If ARM64 release artifacts are not actually published and tested, do not advertise aarch64 support.

---

# 20. First Omarchy pitch

Do not begin with a PR if project inclusion has not been discussed and their workflow favors discussion first.

Suggested pitch:

> I've been developing Fjern, a native Rust remote-desktop client for RDP and VNC with Omarchy/Hyprland as its primary Linux target.
>
> RDP against Windows and VNC against WayVNC are working on real hosts. Omarchy-specific validation includes compositor shortcut capture, dynamic resolution, keyboard/mouse input and clipboard behavior.
>
> Fjern is an independent project rather than an Omarchy fork, and it now has tagged releases, an Arch package and clean installation on current Omarchy.
>
> Would there be interest in including Fjern in `omarchy-pkgs`?

Include:

- GitHub repo
- short demo/screenshot
- package install command
- exact Omarchy version tested
- concise feature list
- known limitations
- security model summary

Do not oversell.

---

# 21. Future Omarchy core integration

Only pursue this after Fjern has users and package inclusion is working.

Possible future integrations:

## Windows VM client

Potential:

```text
Omarchy Windows VM
       ↓
     Fjern
       ↓
      RDP
```

Do not request replacement of FreeRDP until Fjern has feature parity relevant to that workflow.

## RDP readiness probe

Fjern already has protocol-aware diagnostics.

Potential future usage:

```bash
fjern probe HOST
```

for determining whether an RDP service is actually ready rather than only checking an open TCP port.

This should only be proposed if it reduces complexity compared with existing Omarchy tooling.

## Omarchy launcher/plugin integration

Optional future integration:

- recent connections
- saved connections
- status shortcuts
- launch a Fjern profile
- WayVNC helper actions

Keep these integrations outside the core protocol engine.

---

# 22. Release acceptance criteria for Fjern v0.2.0

Do not cut `v0.2.0` until all mandatory items are complete.

## Mandatory

- [ ] Product renamed to Fjern
- [ ] Main crate renamed to `fjern`
- [ ] Binary named `fjern`
- [ ] `linrdp-proto` intentionally retained
- [ ] Config migration implemented
- [ ] Migration tests pass
- [ ] README rebranded
- [ ] `.desktop` renamed and verified
- [ ] icon added
- [ ] SECURITY.md added
- [ ] vendor/README.md added
- [ ] Arch PKGBUILD added
- [ ] clean Arch package build passes
- [ ] clean Omarchy install passes
- [ ] existing Ubuntu CI passes
- [ ] Arch CI passes
- [ ] release artifact workflow passes
- [ ] SHA-256 files generated
- [ ] `fjern --version` works
- [ ] no unintended LinRDP branding remains
- [ ] release notes document rename and migration
- [ ] RDP real-host smoke test passes
- [ ] WayVNC real-host smoke test passes

## Recommended

- [ ] README screenshot/GIF
- [ ] AUR package or AUR-ready PKGBUILD
- [ ] AppStream metadata
- [ ] GitHub Security Advisories enabled
- [ ] issue templates
- [ ] reproducibility notes
- [ ] package dependency review with `namcap`

---

# 23. Validation commands

Run all of these before finalizing:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo build --workspace --release --locked
```

Then:

```bash
cargo metadata --no-deps
```

Branding audit:

```bash
rg -n --hidden --glob '!target/**' --glob '!.git/**' 'LinRDP|zeq0r/linrdp'
rg -n --hidden --glob '!target/**' --glob '!.git/**' '\blinrdp\b'
```

Packaging:

```bash
cd packaging/arch
makepkg --syncdeps --noconfirm
namcap PKGBUILD
namcap fjern-*.pkg.tar.zst
```

Installed package:

```bash
fjern --version
fjern --help
```

If a clean Omarchy machine is available, perform manual real-host validation after package install.

---

# 24. Definition of done

This task is complete when:

1. The project is visibly and consistently branded as **Fjern**.
2. Existing LinRDP users can upgrade without losing config/trust state.
3. `linrdp-proto` remains a clean technical RDP engine crate.
4. Fjern produces reproducible tagged Linux release artifacts.
5. An Arch package can be built and installed cleanly.
6. CI validates both Rust quality and Arch packaging.
7. Security/reporting documentation is present.
8. Vendored modifications are documented.
9. Current Omarchy + Hyprland has been manually validated.
10. README presents Fjern as a product before presenting protocol internals.
11. Known limitations are explicit.
12. The project is ready for a concise `omacom/omarchy-pkgs` inclusion discussion/PR.

---

# 25. Priority order

Execute in this order:

```text
P0  Rename application/repository identity
P0  Safe config/trust migration
P0  Ensure full test suite passes
P0  Release/package asset naming
P0  Arch PKGBUILD
P0  Clean Arch + Omarchy validation
P0  SECURITY.md
P0  Document vendored patches

P1  README/product presentation
P1  Icon and desktop polish
P1  Arch CI
P1  Platform validation matrix
P1  v0.2.0 release
P1  Omarchy package pitch

P2  AUR
P2  AppStream
P2  ARM64
P2  GUI connection manager
P2  Secret Service credential persistence
P2  multi-monitor
P2  hardware accelerated H.264
P2  deeper Omarchy core integration
```

---

# 26. Constraints for the coding agent

While executing:

- Preserve security guarantees.
- Do not weaken TLS validation for convenience.
- Do not silently trust changed certificates.
- Do not place credentials in argv, files or logs.
- Preserve bounded parsing/allocation behavior.
- Keep `unsafe_code = "forbid"` for project crates.
- Do not claim support without tests.
- Do not remove existing protocol tests.
- Add regression tests for every migration behavior.
- Prefer small commits.
- Do not rewrite Git history unless explicitly instructed.
- Do not delete the `v0.1.0` release.
- Do not create a new unrelated repository; rename the existing repository.
- Do not make Fjern require Omarchy.
- Do not rename `linrdp-proto` unless explicitly instructed later.
- Before every release, run the full validation suite.
- If an implementation choice conflicts with security, backward compatibility or data preservation, stop that subtask and choose the safer design.

---

# Final target state

```text
zeq0r/fjern

Fjern
├── Native Rust application
├── RDP
│   └── linrdp-proto
├── VNC / WayVNC
├── Wayland / Hyprland integration
├── secure certificate trust
├── clipboard + file transfer
├── dynamic resolution
├── Arch package
├── tagged release artifacts
└── Omarchy-first validation
```

Positioning:

> **Fjern — Native remote desktop for Linux.**
> RDP and VNC. Wayland first. Developed first for Omarchy.

The immediate upstream objective is **inclusion in `omacom/omarchy-pkgs`**, not replacement of Omarchy's existing remote-desktop stack.
