# First-desktop viewer

`linrdp connect <host> [port] --user <account> [trust-option]` performs TLS,
NTLM CredSSP, MCS/GCC, Client Info, valid-client licensing, capabilities and
activation, then opens a native desktop display. The password is prompted
locally after certificate verification. Connection setup still starts from
the terminal; there is no graphical connection form yet.

This branch also offers an opt-in [H.264 graphics profile](h264.md). The
description below covers the default bitmap profile unless stated otherwise.

The initial profile requests 1024×768 and a 32-bit color session by default.
`--size WIDTHxHEIGHT` selects 200–8192 pixels per dimension, bounded by the
16-million-pixel budget. Windows font smoothing and desktop composition are
enabled explicitly, preserving ClearType subpixel detail that RGB565 discards.
Windows has accepted both 1920×1080 and 1280×800. The decoder supports raw
16-, 24-, and 32-bit bottom-up bitmaps with row padding and interleaved RLE
bitmaps, with and without compression headers. The server controls the negotiated dimensions
within a bounded 16-million-pixel budget.

Dynamic resolution is implemented for one monitor and defaults to on. Use
`--dynamic-resolution on|off` to select it explicitly; `--size` still controls
the initial connection dimensions. When enabled, LinRDP negotiates the Display
Control dynamic virtual channel and, after an activated desktop has painted,
coalesces window changes for 300 ms before requesting the latest size. Requests
are limited to 200–8192 pixels per dimension, the server-advertised display area,
and LinRDP's 16,777,216-pixel cap. Odd window widths round down to the even width
required by the protocol. Only one request is outstanding at a time.

The centered, aspect-preserving local scaler remains active throughout. It is
the fallback when dynamic resolution is disabled, the server does not make the
channel available, the window size is outside the negotiated limits, or a remote
change is not confirmed within five seconds. The implementation and synthetic
protocol behavior are tested locally. A Windows-host check confirmed this remote
framebuffer sequence: 960×1056, 860×1056, 1160×1056, 1024×768, 1280×800,
900×600 and 1280×800. Every change completed deactivation/reactivation and was
confirmed by the resulting remote dimensions. After the final resize, keyboard
input produced exact `aA\tb` text and Unicode multiline clipboard text made an
exact Linux→Windows→Linux round trip, including `ÆØÅ/æøå`. Clipboard file
transfer was not repeated in this run; its earlier evidence remains separate.
Other Windows versions and RDP servers remain compatibility work.

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
Snapshot production follows UI consumption: the decoder applies every delta,
but does not copy the full desktop for every packet while a snapshot is pending.
The UI takes ownership of the pixel buffer and native-size presentation bypasses
the local scaler. Unchanged images are not uploaded unless the native window
needs repainting. The event loop targets 120 Hz; this is not a guarantee of
120 distinct remote frames per second. See [performance](performance.md).
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

This first profile does not implement audio, general RDS CAL license acquisition, drawing orders, bulk
compression, graphics-pipeline codecs, multi-monitor display,
RemoteApp or automatic reconnection. Unsupported required messages stop the
connection explicitly. The pointer is composited when Windows supplies a
position; the local OS pointer remains visible. Cursor shape/hotspot integration
with local input is not complete.

## Basic interactive input

The focused, activated session accepts keyboard scancodes, three mouse buttons,
pointer movement and vertical wheel input through TLS-protected slow-path Input
PDUs. Native event queues preserve keyboard and mouse-button press/release order,
including very short clicks and double-click edges completed between rendered
frames. The native pointer queue, UI event buffer and network input queue are
bounded; overflow fails the session instead of silently dropping an edge or key
release. An 8 ms receive poll budget lets outgoing input progress while the
remote desktop is static. This is a scheduling choice, not an end-to-end latency
guarantee. The desktop socket uses TCP_NODELAY for small input writes.

Focus loss releases keys and buttons. Already-held keys/buttons are ignored on
focus entry until released; minimizing and normal closure also release tracked
input. Closing waits for the worker's bounded best-effort release write before
closing the socket. Transport failure cannot guarantee delivery of releases.
Clicks outside the centered desktop are ignored. Leaving the desktop during a
drag releases its button at the last valid position; reentry does not click
again until the physical button has been released.

Queued mouse edges are implemented and locally tested, but an actual Windows GUI
double-click has not yet been verified. The current US scancode mapping covers
ordinary keys, modifiers, navigation, keypad and F1–F12. Pause, Print Screen,
F13–F15, IME/Unicode text composition, layout selection and lock-state
synchronization with the local desktop remain future work. Compositor shortcuts
remain local where intercepted. Danish keyboards and X11 input have not been
tested against a real host.

minifb exposes different wheel units on Wayland and X11. The viewer identifies
its actual native backend, reverses Wayland's downward-positive axis and uses
15 axis units per wheel step; X11 already provides signed steps. Fractional
movement accumulates into bounded RDP wheel events. Horizontal scrolling and
extra mouse buttons are not advertised.

A synthetic burst of 479 characters with effectively simultaneous press/release
events lost characters in Windows Notepad despite matching outgoing event counts
and a scancode-sequence checksum. FreeRDP reproduced losses with zero-dwell
XTest input; a reference run with 5 ms between edges delivered the text. This
does not establish a universal rate limit or rule out LinRDP timing issues.
Very fast synthetic input remains outside the verified compatibility claim.

Windows host checks verified Start-menu keyboard shortcuts, a correctly placed
mouse click, text in Notepad and lowercase typing after losing focus while Shift
was held. See the [input report](windows-first-probe.md#basic-interactive-input).

## Clipboard and Wayland keyboard correction

Text and file copy/paste use the CLIPRDR channel; see [clipboard](clipboard.md)
for usage, staging behavior, limits and verified file-manager transfers.

A targeted minifb 0.28.0 vendor patch resolves Wayland key transitions at the
base XKB level. Upstream's effective-symbol lookup could miss shifted letters,
Shift+Tab and releases after modifier changes. Character callbacks retain their
effective symbols. A native XKB regression fails with the original lookup and
passes with the correction. Windows Notepad also displayed `aA`, an actual Tab,
and a following lowercase `b`, which were copied back and checked byte-for-byte.
See [vendor patch](../vendor/minifb/LINRDP-PATCH.md).

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
