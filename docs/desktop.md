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
framebuffer; it does not renegotiate the remote resolution.

The client requires successful licensing, Demand Active, server synchronization,
control cooperation/grant and Font Map before accepting bitmap output. It sends
Confirm Active, synchronization, control requests, Font List, input-state
synchronization and an initial refresh request when the server supports it.
Deactivation disables graphics until another capability/activation exchange.
The advertised 24-bit color pointer cache is bounded to 20 entries of at most
32×32 pixels; masks, hotspots, cache references and screen clipping are checked.

Network processing runs separately from native window/event handling. Partial
TPKT frames survive idle read timeouts. Incomplete frames, activation and the
first bitmap have deadlines; an active static desktop can remain idle. Closing
the window cancels the receiver and closes the socket without sending logoff.
Only the latest display is shared with the UI, avoiding an unbounded frame queue.
Credentials in Client Info and the enclosing outgoing frame use zeroizing
buffers. They are released before the display loop begins. Screen contents and
reconnection cookies are never automatically written to disk.

## Scope and evidence

Local tests cover activation ordering, malformed/truncated packets, raw and RLE
color/orientation, bitmap limits, pointer masks and caches, plus existing TLS,
NTLM and MCS tests. Windows has completed the valid-client licensing and
activation sequence. The first real bitmap is still under investigation: the
test host reports `LOGON_MSG_BUMP_OPTIONS` (session contention), then closes the
connection without a displayed bitmap. A black window is not counted as a
successful desktop display.

The viewer uses minifb for the native window and ironrdp-graphics exclusively for
interleaved bitmap RLE decompression. RDP session sequencing, wire parsing,
capability negotiation and compositing are implemented in LinRDP. These
libraries do not replace the protocol engine. Transitive dependencies may
contain additional codecs/PDU types which this profile does not use.

This first profile does not implement interactive keyboard/mouse forwarding,
clipboard, audio, general RDS CAL license acquisition, drawing orders, bulk
compression, fast-path output, graphics-pipeline codecs, dynamic resolution,
RemoteApp or automatic reconnection. Unsupported required messages stop the
connection explicitly. The pointer is composited when Windows supplies a
position; the local OS pointer remains local in this read-only viewer.

## Building on Linux

A C toolchain and pkg-config are needed for native dependencies. minifb includes
Wayland and X11 backends, with dynamically loaded display libraries. A running
graphical session is required for `connect`; protocol tests do not require one.

## References

- [RDP connection sequence](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-rdpbcgr/023f1e69-cfe8-4ee6-9ee0-7e759fb4e4ee)
- [MS-RDPBCGR](https://winprotocoldoc.z19.web.core.windows.net/MS-RDPBCGR/%5BMS-RDPBCGR%5D-220903.pdf), sections 2.2.1.11–22, 2.2.7, 2.2.9.1.1, 2.2.10.1.1.4 and 3.2.5.3
- [minifb](https://docs.rs/minifb/0.28.0/minifb/)
- [IronRDP graphics](https://github.com/Devolutions/IronRDP/tree/master/crates/ironrdp-graphics)
