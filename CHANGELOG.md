# Changelog

All notable user-facing changes are documented here.

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
