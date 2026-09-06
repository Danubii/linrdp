# First-desktop viewer

`linrdp connect <host> [port] --user <account> [trust-option]` performs TLS,
NTLM CredSSP, MCS/GCC, Client Info, valid-client licensing, capabilities and
activation, then opens a native read-only display. The password is prompted
locally after certificate verification. Connection setup still starts from
the terminal; there is no graphical connection form yet.

The initial profile requests 1024×768 at 16 bits per pixel. The decoder supports
raw bottom-up RGB565 bitmaps with row padding and interleaved RLE bitmaps, with
and without compression headers. The server controls the negotiated dimensions
within a bounded 16-million-pixel budget. Window resizing scales the existing
framebuffer while preserving its aspect ratio; it does not renegotiate the remote resolution.

The client requires successful licensing, Demand Active, server synchronization,
control cooperation/grant and Font Map before accepting bitmap output. It sends
Confirm Active, synchronization, control requests, Font List, input-state
synchronization and an initial refresh request when the server supports it.
Deactivation disables graphics until another capability/activation exchange.
The advertised 24-bit color pointer cache is bounded to 20 entries of at most
32×32 pixels; masks, hotspots, cache references and screen clipping are checked.

Network processing runs separately from native window/event handling. Partial
TPKT and fast-path frames survive idle read timeouts. Incomplete frames, activation and the
first bitmap have deadlines; an active static desktop can remain idle. Closing
the window cancels the receiver and closes the socket without sending logoff.
Only the latest display is shared with the UI, avoiding an unbounded frame queue.
Credentials in Client Info and the enclosing outgoing frame use zeroizing
buffers. They are released before the display loop begins. Screen contents and
reconnection cookies are never automatically written to disk.

## Scope and evidence

Local tests cover activation ordering, malformed/truncated packets, raw and RLE
color/orientation, bitmap limits, pointer masks and caches, plus existing TLS,
NTLM and MCS tests. Fast-path tests cover partial framing, coalesced packets,
fragment ordering, size limits and bitmap reconstruction. On 2026-09-06, the
Windows test host completed licensing and activation and displayed its actual
desktop in the native Wayland window at 1024×768. The wallpaper, icons and
taskbar were visually verified. Closing the window exited successfully after
an idle period. See the [Windows report](windows-first-probe.md).

The initial slow-path-only profile activated but received no bitmap from this
host. Advertising fast-path output and decoding its bitmap updates resolved
the first-display failure. Both output transports are now supported; bulk
compression remains disabled. Fast-path fragment reassembly is bounded to
65,535 bytes; larger multifragment updates are not advertised.

The viewer uses minifb for the native window and ironrdp-graphics exclusively for
interleaved bitmap RLE decompression. RDP session sequencing, wire parsing,
capability negotiation and compositing are implemented in LinRDP. These
libraries do not replace the protocol engine. Transitive dependencies may
contain additional codecs/PDU types which this profile does not use.

This first profile does not implement interactive keyboard/mouse forwarding,
clipboard, audio, general RDS CAL license acquisition, drawing orders, bulk
compression, graphics-pipeline codecs, dynamic resolution,
RemoteApp or automatic reconnection. Unsupported required messages stop the
connection explicitly. The pointer is composited when Windows supplies a
position; the local OS pointer remains local in this read-only viewer.

## Building on Linux

A C toolchain and pkg-config are needed for native dependencies. minifb includes
Wayland and X11 backends, with dynamically loaded display libraries. A running
graphical session is required for `connect`; protocol tests do not require one.
The minifb Wayland backend currently emits proxy-cleanup warnings on window
closure on the tested compositor; the process still exits with status 0.

## References

- [RDP connection sequence](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-rdpbcgr/023f1e69-cfe8-4ee6-9ee0-7e759fb4e4ee)
- [MS-RDPBCGR](https://winprotocoldoc.z19.web.core.windows.net/MS-RDPBCGR/%5BMS-RDPBCGR%5D-220903.pdf), sections 2.2.1.11–22, 2.2.7, 2.2.9.1, 2.2.10.1.1.4 and 3.2.5.3
- [minifb](https://docs.rs/minifb/0.28.0/minifb/)
- [IronRDP graphics](https://github.com/Devolutions/IronRDP/tree/master/crates/ironrdp-graphics)
