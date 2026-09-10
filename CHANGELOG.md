# Changelog

All notable user-facing changes are documented here.

## Unreleased

- VNC batches event processing inside one async runtime call per UI tick and
  converts pixel rows through bounded slices to reduce per-pixel indexing work.

- VNC consumes queued image tiles within a 4 ms UI work budget instead of
  stopping after 64 events, reducing backlog for large ZRLE updates.
- VNC CopyRect applies overlapping moves directly in the framebuffer without
  allocating a temporary rectangle.

## 0.2.0 - 2026-09-10

LinRDP has been renamed to Fjern. The executable and application crate are now
named `fjern`; the protocol crate intentionally remains `linrdp-proto`.

On first startup, Fjern imports valid, privately permissioned `profiles.json`
and `known_hosts.json` files from `~/.config/linrdp` when
`~/.config/fjern` does not already exist. The import is atomic and leaves the
legacy directory untouched. Existing Fjern state always wins and certificate
pins are never merged or replaced automatically.

- Added deterministic Linux release bundles and an Arch Linux binary package.
- Added clean Arch CI coverage for build, tests, `makepkg` and `namcap`.
- Corrected the VNC smoke fixture's server-name length after the rebrand.
- Correctly treats RFB ExtendedDesktopSize status 4 as an asynchronously
  forwarded resize request rather than a rejection; local scaling and input
  mapping continue while waiting for a server layout update.
- Arrow keys can now leave text fields in the terminal UI, so the entire form
  can be navigated without tabbing through every control.

## 0.1.0 - 2026-09-09

The first development release provides a keyboard-first Linux client for RDP
and VNC, developed primarily against Omarchy, Windows RDP and WayVNC.

### Highlights

- Native resizable RDP and VNC desktop windows on Wayland.
- Saved connection profiles through an MSTSC-inspired terminal interface.
- Windows RDP authentication through TLS, NTLM CredSSP and NLA.
- 32-bit bitmap sessions, Windows font smoothing, dynamic resolution and local
  scaling fallback.
- Keyboard, pointer, wheel and captured Omarchy/Super shortcut forwarding.
- RDP clipboard text, files and directories in both directions.
- VNC VeNCrypt, classic authentication, ZRLE, Raw, CopyRect, server resizing,
  remembered certificate trust and bidirectional text clipboard.
- Optional experimental RDP H.264/AVC420 graphics.

### Current limits

- One remote monitor per connection.
- RDP H.264 uses software AVC420 decoding; bitmap graphics remain the default.
- VNC file transfer needs a server-specific extension and is not included in
  the standard RFB clipboard implementation.
- WayVNC must run without `--render-cursor` for a local-only client pointer.
- Packages for distribution repositories are not published yet.
