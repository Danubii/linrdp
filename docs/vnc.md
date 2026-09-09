# VNC connections

Select the RDP/VNC button on the terminal connection screen to choose VNC.
The default port changes to 5900. Set a custom port in Options and save the
connection normally. VNC profiles accept an optional username and never store a
password. Existing profiles without a protocol setting remain RDP connections.

```sh
cargo run --release -p linrdp -- vnc workstation.example 5900
cargo run --release -p linrdp -- vnc workstation.example 5900 --user alice
cargo run --release -p linrdp -- tui
```

The VNC adapter uses the [vnc-rs engine](https://docs.rs/vnc-rs/0.5.3/vnc/).
It is separate from RDP negotiation, TLS trust, CredSSP and graphics codecs.
Choosing RDP for a server that sends an RFB banner produces an explicit message
suggesting VNC; the client never silently switches protocols or credentials.

The adapter supports VeNCrypt 0.2 with X509Plain, X509Vnc, X509None, TLSPlain,
TLSVnc and TLSNone, plus classic VNC password authentication and None. It prefers
certificate-authenticated TLS and never retries a weaker method after a TLS or
authentication failure. Unencrypted VeNCrypt Plain is refused. A password is
requested locally only when needed and is never accepted on the command line.

System-trusted X.509 certificates require no prompt. For an otherwise valid
self-signed, unknown-issuer or hostname-mismatched certificate, the client shows
its SHA-256 fingerprint and validity period and requires interactive approval for
that connection before sending credentials. Expired and not-yet-valid
certificates are rejected. Anonymous VeNCrypt TLS encrypts the connection but
cannot authenticate the server, and the client reports this explicitly.

When the server advertises ExtendedDesktopSize, stable local window changes send
a single-screen SetDesktopSize request after a short debounce. Server-rounded
sizes do not create request feedback. Servers without the extension use local
linear scaling with the desktop aspect ratio preserved, and pointer coordinates
exclude the letterbox bars. Wheel deltas are accumulated and rate-limited before
being translated into VNC button pulses. RDP clipboard/file transfer, Display
Control and H.264 settings do not apply to VNC.

On Wayland, `Ctrl+Alt+Shift+Enter` captures compositor shortcuts for the VNC
window so combinations such as Super+key reach the remote desktop. The window
title confirms capture. `Ctrl+Alt+Shift+Escape` always releases it; both control
chords are consumed locally and cannot leave remote modifiers held down. If a
server rejects ExtendedDesktopSize, the client reports the protocol reason and
uses aspect-preserving bilinear scaling instead.

Validation includes workspace tests, formatting, Clippy and a release build.
A local RFB 3.8 fixture verified unauthenticated access, the hidden password
prompt and the exact DES challenge response, Raw images, overlapping CopyRect,
ZRLE, server resize and clean client disconnect. The TUI protocol selector and
VNC Options were also exercised interactively. A real VeNCrypt X509Plain server
with a self-signed certificate completed explicit certificate approval, hidden
password entry and authentication, then opened and sustained the desktop window.

The vendored vnc-rs 0.5.3 dependency preserves its MIT/Apache licenses. A local
fix updates refresh-request dimensions after DesktopSize events; the fixture
observed requests change from 64x64 to 80x48. Frontend pixel/rectangle limits
are enforced, but upstream codec allocations are not fully bounded before
frontend validation. This remains an experimental adapter, not a claim of
complete VNC extension or hostile-server compatibility.

To repeat the local wire-level smoke test, run `python3 tools/vnc_smoke.py`
and connect to the printed loopback port. Add `--auth` to test password
authentication with the synthetic password `fixture` (requires OpenSSL with
its legacy provider). Close the client window after the blue desktop appears.
The fixture checks that refresh requests use the new size after server resize.

Servers may offer vendor-specific security types alongside standard ones. The
client now reads the whole list and selects a supported method instead of failing
on an unknown alternative. A loopback `[129, 2]` offer successfully completed
password authentication, rendering and resize. An offer without type 1 or 2
still fails explicitly with the full numeric list; this does not implement
security type 129 itself. Vendor type assignments can overlap; see the
[IANA RFB registry](https://www.iana.org/assignments/rfb).
