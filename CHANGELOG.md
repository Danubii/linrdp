# Changelog

All notable user-facing changes are documented here.

## 0.2.1 - 2026-09-10

- Preserve batched Wayland scroll deltas and final touchpad movement.
- Rename the Arch binary package to `fjern-bin` (the command remains `fjern`).
- Include dependency and Rust standard-library license notices in releases.

- Batch legacy RDP bitmap bursts before presentation to reduce partial-scroll
  updates; retain immediate completed-frame presentation for GFX.
- Replace stale queued RDP snapshots with newer eligible images and avoid
  per-command payload copies in the GFX/H.264 parser.

- Keep RDP reception responsive during slow writes with a bounded, ordered
  ciphertext queue and cancellation of stalled writes on close.
- Track row damage across recycled RDP snapshots and GFX outputs; cursor-only
  changes no longer trigger full desktop copies.
- Convert AVC420 regions directly into existing surfaces without a temporary
  full-frame RGB image, preserving reference decoding and color conversion.

- Apply small VNC tiles in one UI iteration and scale explicit damage spans
  without scanning or keeping a second source image.
- Submit Wayland frames through mapped shared memory with partial row updates
  and buffer-reuse-aware surface damage.
- Expand ZRLE solid tiles and RLE runs with block copies.

- Correct refresh dimensions after VNC ExtendedDesktopSize updates.
- Resume VNC reads immediately when decoder-queue capacity becomes available;
  handle slow writes independently with a deadline and explicit queue errors.
- Buffer ZRLE decompression and reject oversized payloads, invalid palette
  indices and runs before copying.
- Process large Raw images in row batches and cache bilinear scaling for
  unchanged rows, preserving interpolation and pointer mapping.
- Add optional VNC processing statistics and a graphical loopback regression.

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
