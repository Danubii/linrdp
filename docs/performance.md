# Desktop presentation performance

## VNC tile scheduling and CopyRect

The ZRLE decoder emits one image event per 64×64 tile. A full 1920×1080
rectangle produces 510 events. The previous limit of 64 events per UI tick
required at least eight ticks to consume that image, even with every tile
already queued. VNC now processes queued events until 4 ms of work has elapsed,
with a secondary cap of 4096 events per tick. Every event remains ordered;
partial images and CopyRect dependencies are never discarded. The time budget
is checked after each event, so a single expensive event can exceed it.

CopyRect now moves rows directly within the framebuffer using overlap-safe
copies. Downward moves run bottom-up; upward moves run top-down. This removes
the temporary rectangle allocation and the extra copy. An exhaustive small-grid
test compares all valid rectangle moves with a snapshot reference.

An optimized local CPU benchmark of a 1920×1064 rectangle scrolled down by
16 pixels measured 3.141 ms per update with the previous implementation and
0.283 ms with the new one. It alternates execution order across six batches
of 100 updates and reports the upper median. Reproduce with:

```sh
cargo test --release -p fjern benchmark_vnc_copy_rect --locked -- --ignored --nocapture
```

This is CPU copy time, not measured remote FPS. WayVNC's chosen encoding,
network transfer, decoding and compositor timing still affect the visible
result. CopyRect improvements apply only when the server sends CopyRect.
The existing 16 ms incremental refresh request cadence is unchanged.

Event processing enters the async runtime once per UI tick instead of once per
tile. The event count and time limits still apply, and cursor refresh requests
are awaited within the same runtime call.

Pixel conversion uses a bounded destination row and paired source/destination
iterators. A separate 1080p CPU benchmark measured 0.520 → 0.479 ms for 64-pixel
row segments and 0.380 → 0.324 ms for full-width rows. These are isolated
conversion costs, excluding allocation, transport, decoding and display. The
benchmark alternates old/new order across six batches of 100 images and checks
identical output. A regression checks nonzero tile offsets, untouched borders,
odd row widths and removal of the unused high byte. Reproduce with:

```sh
cargo test --release -p fjern benchmark_vnc_unpack_pixels --locked -- --ignored --nocapture
```

## RDP presentation

The default profile negotiates RGB565 bitmap updates with interleaved RLE and
fast-path output. This branch adds an optional [H.264 graphics profile](h264.md).
Client-side presentation optimizations do not
reduce network bandwidth or change the server's encoding rate.

## Snapshot scheduling

The decoder applies every protocol update to its authoritative framebuffer.
When the UI has consumed the pending snapshot, the receiver composites the
current pixels and cursor into a reusable staging buffer outside the display
mutex. Buffer ownership is swapped through one pending slot into the UI.
While that slot is occupied, incoming deltas continue to update the decoder;
they do not trigger redundant full-screen copies. The next snapshot includes
all accumulated changes. This is bounded coalescing, not packet dropping or
frame interpolation. One already queued snapshot may precede newer updates.

At the negotiated native resolution, the UI sends its pixel buffer directly to
the window backend. Scaling uses one horizontal lookup table per frame and
clears only letterbox borders. The native backend can still perform its own
copy into compositor storage; this is not an end-to-end zero-copy pipeline.

The UI polls at a target of 120 Hz and only uploads changed images or native
repaints. Network reads have an 8 ms idle poll budget, and TCP_NODELAY avoids
Nagle buffering of small outgoing input messages. These settings bound parts
of client scheduling, not network latency or actual remote FPS. Keyboard and
mouse-button transitions retain their existing ordered queues.

On Wayland, buffer ownership follows each `wl_buffer.release`: a buffer becomes
busy on submission, and resizing installs the replacement buffer's release
state. The compositor pool is capped at three buffers; when all are busy,
presentation is retried after event processing instead of allocating more
buffers or overwriting one still in use. Pending native configure events and
X11 exposure events request a repaint even if the remote image is unchanged.

## Reproducing CPU measurements

```sh
cargo test --release -p fjern benchmark_ -- --ignored --nocapture --test-threads=1
```

The viewport benchmark compares the previous renderer with the production
renderer, checks exact pixel equality, alternates execution order, and reports
the median of five batches of 100 frames. One development-machine run:

| Remote → window | Previous | Optimized |
| --- | ---: | ---: |
| 1920×1080 → 1920×1080 | 3.330 ms | 0.351 ms |
| 1920×1080 → 1280×720 | 1.533 ms | 0.534 ms |
| 1920×1080 → 1600×1000 | 2.313 ms | 0.806 ms |
| 1280×720 → 1920×1080 | 3.363 ms | 0.974 ms |

The actual viewer bypasses the native-size renderer entirely. A synthetic
1080p burst of 1024 deltas with UI consumption every 64 deltas reduced snapshot
copies from 1024 to 17 (343.4 ms to 8.7 ms in the same run). This intentionally
models a busy decoder and slower consumer; it is not a typical workload or
an end-to-end speedup claim. The regression also verifies accumulated pixels,
buffer ownership, dimensions after resize and input readiness.

The native Wayland lifecycle test also passed against the development machine's
compositor: it exhausted the three-buffer pool without dispatching releases,
then verified that event processing allowed the final image and a resized image
to reach busy shared-memory buffers. It does not measure physical scanout or
prove tear-free output on every compositor. Reproduce it in a graphical session
(it briefly opens a small test window):

```sh
cargo test --manifest-path vendor/minifb/Cargo.toml --locked --lib \
  native_wayland_busy_pool_retries_final_image -- --ignored --nocapture
```

These measurements exclude Windows encoding, network transfer, bitmap
decompression and compositor display timing. A before/after Windows scrolling
or video session has not yet established an actual FPS improvement.

## Better network compression

A new private codec cannot be used against an unmodified Windows RDP server.
The interoperable route is the RDP Graphics Pipeline Extension, including its
dynamic channel, capability negotiation, required codecs, surfaces, frame
acknowledgements and compression framing. H.264 modes are selected through
that extension's capabilities, independently of bitmap-codec capabilities.
See Microsoft's [graphics capability negotiation specification](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-rdpegfx/31c6e2b1-335b-4a75-9454-bb2309958c21).

The experimental profile implements version 8.1 with software AVC420 decoding.
Hardware decoding and AVC444 remain future work. See [H.264](h264.md) for the
supported codecs and limits of current Windows validation.

## Native Wayland submission

The Wayland backend now submits packed window-sized input directly to its shared
memory writer. Previously it always ran the scalar resizer, even after the viewer
had already produced the exact window dimensions. This extra full-screen pass
affected both bitmap and experimental graphics sessions. Inputs with different
sizes or padded strides retain the existing scaling path.

An isolated optimized build of the previous C scaler took 3.107–3.688 ms per
1080p frame and 12.463–13.112 ms per 4K frame in three batches of 100 calls after
other builds completed. The new native-size path eliminates that pass; it still
copies into compositor storage. These measurements do not establish remote FPS.

A live Wayland regression checks the submitted pixel contents, verifies that the
intermediate scaling buffer is untouched for native-size input, and verifies the
padded-stride fallback. The existing compositor-buffer exhaustion and resize test
also passes. Run both with:

```sh
cargo test --manifest-path vendor/minifb/Cargo.toml --release native_ -- --ignored --nocapture --test-threads=1
```

For interactive performance testing, use the optimized binary:

```sh
cargo run --release -p fjern -- tui
```

A plain `cargo run` builds without release optimizations and is unsuitable for
comparing decoding or animation performance.
