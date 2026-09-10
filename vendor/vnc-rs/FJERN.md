# Local changes

Vendored from vnc-rs 0.5.3 (MIT OR Apache-2.0).

`client/connection.rs`: update the stored screen dimensions when delivering
SetResolution through either recv_event or poll_event. Subsequent Refresh and
FullRefresh requests must cover the current server desktop, not ServerInit's
original size. Verified with a loopback RFB DesktopSize update and inspection
of the following refresh request. The client also exposes ExtendedDesktopSize
notifications and SetDesktopSize requests for dynamic single-screen resizing.
RFB ExtendedDesktopSize status 4 is exposed as a pending/forwarded request,
rather than incorrectly reporting it as a rejection. Successful
DesktopResizeAvailable events now also update stored refresh dimensions in both
receive paths; the wire-level regression checks incremental and full requests.

The transport reserves decoder-queue capacity before reading, so freeing a slot
wakes it without waiting for unrelated input. Concurrent read/write futures
keep reception live during a slow write. A five-second write deadline and
cancellable shutdown terminate stalled connections. Input enqueue is nonblocking:
a full queue returns an error rather than silently losing key transitions or
freezing the UI. Limits are 32 network blocks (64 KiB each), 4096 output events,
256 input messages and 1 MiB per outgoing clipboard message. Events are bounded
by a shared 64 MiB payload budget as well as event count. This permits a full
4K frame of small ZRLE tiles without allowing thousands of large Raw frames.
The decoder's in-progress event and the UI's dequeued event are additional to
that queue budget; it is not a total process-memory limit.
FramebufferUpdated marks decoded server-update boundaries for optional metrics.

`codec/zlib.rs`: a 32 KiB reader buffers decompressed bytes. Tests cover multiple
sync-flushed rectangles in one zlib stream, buffer boundaries, truncation and
trailing decoded data. `codec/zrle.rs` validates compressed sizes, rectangle
dimensions, palette indices and run lengths before allocating/copying. Raw and
cursor dimensions are also bounded. The shared allocation helper now initializes
its memory instead of constructing a Vec with uninitialized elements. These
changes do not certify all unused upstream codecs for hostile-server input.

Run the regression suite with:

```sh
cargo test --manifest-path vendor/vnc-rs/Cargo.toml --locked --lib
```

`client/auth.rs`: read the entire RFB 3.7/3.8 security offer and ignore unknown
alternatives when None or VNC password is offered. If there is no supported
choice, report the original numeric list. RFB 3.3 security values no longer
truncate from u32 to u8. Authentication result values are matched safely instead
of transmuting arbitrary server data into a two-variant enum.

`client/connector.rs`: reject a failed RFB 3.8 None SecurityResult rather than
proceeding to ServerInit. The unknown-security diagnostic spelling is corrected.

Unit test: `cargo test --manifest-path vendor/vnc-rs/Cargo.toml --lib unknown_offers`.
Loopback test: `python3 tools/vnc_smoke.py --auth --unknown` offers `[129, 2]`
and independently verifies that the client selects password authentication.
