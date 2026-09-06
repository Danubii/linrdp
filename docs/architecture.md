# Architecture

## Scope

LinRDP is a Linux client with an original Rust implementation of RDP.
Existing RDP servers handle remote sessions. No server, relay, account service,
or custom wire protocol belongs in this repository.

## Boundaries

- `linrdp-proto`: wire encoding and decoding, independent of UI and networking.
- `linrdp`: the development executable, including negotiation and verified TLS
  diagnostics. Its TLS module wraps rustls; see [TLS and trust](tls.md).
- Future session layer: transport, TLS, CredSSP, connection state and channels.
- Future native UI: connection form, credentials, session rendering and input.

Select a UI toolkit after a rendering/input spike on Wayland and X11.
Use established cryptography, TLS and codec libraries where appropriate;
an original RDP engine does not mean inventing cryptography or video codecs.

## Engineering rules

Treat remote bytes as untrusted. Bound allocations, validate lengths before
indexing, and return errors for unsupported messages. Keep parsers testable
without a network. Avoid unsafe code in project crates.

Before implementing authenticated sessions, establish certificate validation
and a deliberate trust flow. Do not silently downgrade security, log secrets,
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
