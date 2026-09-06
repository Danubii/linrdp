# Architecture

## Scope

LinRDP is a Linux client with an original Rust implementation of RDP.
Existing RDP servers handle remote sessions. No server, relay, account service,
or custom wire protocol belongs in this repository.

## Boundaries

- `linrdp-proto`: wire encoding/decoding, CredSSP state, MCS/GCC setup,
  desktop activation, bounded bitmap decoding and pointer compositing. It is
  independent of UI and networking. IronRDP graphics supplies interleaved RLE
  decompression; LinRDP owns the session protocol and capability negotiation.
- `linrdp`: diagnostics and the `connect` executable, verified rustls transport,
  sspi-rs NTLM authentication, hidden credential prompting, and native display.
  See [TLS and trust](tls.md), [CredSSP status](credssp.md) and
  [desktop scope](desktop.md).
- The desktop receiver runs on a worker thread. The main thread owns minifb's
  native window and event loop. A bounded receive buffer and one shared latest
  framebuffer prevent an unbounded queue of network packets or rendered frames.
- Future UI work: graphical connection form, certificate review, interactive
  keyboard/mouse forwarding and session management. The current minifb viewer
  establishes first rendering on Wayland; X11 still needs real-host validation.

Use established cryptography, TLS and codec libraries where appropriate;
an original RDP engine does not mean inventing cryptography or video codecs.

## Engineering rules

Treat remote bytes as untrusted. Bound allocations, validate lengths before
indexing, and return errors for unsupported messages. Keep parsers testable
without a network. Avoid unsafe code in project crates.

Authenticated sessions require certificate validation and an explicit trust
policy. Do not silently downgrade security, log secrets,
or put passwords in process arguments. Credential storage must be opt-in and
use the desktop's secret service.

Network negotiation alone does not authenticate the host. A diagnostic result
must never imply that TLS or login succeeded.

## Protocol references

Implementation follows Microsoft's public specifications, with references near
the corresponding modules:

- [MS-RDPBCGR](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-rdpbcgr/)
- [MS-RDPEGFX](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-rdpegfx/)
- [MS-CSSP](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-cssp/)

## Distribution

Target native Arch packaging (PKGBUILD) and Debian packaging first. Packaging
recipes must be built and installation-tested before being advertised as usable.
AUR publication and official distribution repository inclusion are separate
steps; neither is implied by publishing a GitHub repository.
