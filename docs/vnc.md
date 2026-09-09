# VNC connections

Select the RDP/VNC button on the terminal connection screen to choose VNC.
The default port changes to 5900. Set a custom port in Options and save the
connection normally. VNC profiles do not require a username and never store a
password. Existing profiles without a protocol setting remain RDP connections.

```sh
cargo run --release -p linrdp -- vnc workstation.example 5900
cargo run --release -p linrdp -- tui
```

The VNC adapter uses the [vnc-rs engine](https://docs.rs/vnc-rs/0.5.3/vnc/).
It is separate from RDP negotiation, TLS trust, CredSSP and graphics codecs.
Choosing RDP for a server that sends an RFB banner produces an explicit message
suggesting VNC; the client never silently switches protocols or credentials.

This initial adapter uses ordinary VNC over TCP, without transport encryption.
It supports servers with no authentication or classic VNC password authentication;
TLS/VeNCrypt and server-specific login schemes are not supported. Use a trusted
network or an existing secure tunnel. The password is requested locally only when
needed and is not accepted as a command-line option.

The server determines the desktop size. Local window resizing scales the image;
it does not request a new remote desktop size. RDP clipboard/file transfer,
Display Control, certificate pins and H.264 settings do not apply to VNC.

Validation passes 221 workspace tests, formatting, Clippy and a release build.
A local RFB 3.8 fixture verified unauthenticated access, the hidden password
prompt and the exact DES challenge response, Raw images, overlapping CopyRect,
ZRLE, server resize and clean client disconnect. The TUI protocol selector and
VNC Options were also exercised interactively. The user's actual VNC host has
not yet been tested because its address and port were not supplied.

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
