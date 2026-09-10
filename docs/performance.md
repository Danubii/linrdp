# Desktop presentation performance

## VNC tile scheduling and CopyRect

The ZRLE decoder emits one image event per 64×64 tile. A full 1920×1080
rectangle produces 510 events. The previous limit of 64 events per UI tick
required at least eight ticks to consume that image, even with every tile
already queued. VNC now processes queued events until 4 ms of work has elapsed,
with a secondary cap of 4096 events per tick. Every event remains ordered;
partial images and CopyRect dependencies are never discarded. The time budget
is checked after each event. Small Raw events (including ZRLE tiles up to
64×64) are applied in one iteration: a complete 4K frame takes 2040 tile
iterations, not 10,200 tile/substep iterations. Large Raw rectangles use batches of 16 rows
before processing later events; a following CopyRect or resize cannot overtake
unfinished pixels. Other single expensive events can still exceed the budget.

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

## VNC pipeline regression and measurements

ZRLE now reads from a 32 KiB decompressed buffer. An isolated release benchmark
of three-byte reads over a synthetic 1080p payload measured 43.335 → 18.161 ms.
This uses the vendored crate's release profile and measures decompression plus
reads/assertions, not an entire ZRLE frame or network FPS.

The frontend scaler caches fixed-point coordinate maps and target pixels.
Raw updates and CopyRect destinations explicitly invalidate source-row spans;
only target spans depending on those pixels are recomputed, including both
bilinear neighbors. There is no source snapshot or full-source comparison.
Native-size presentation bypasses scaling; pending damage remains until a
scaled presentation consumes it. Resize invalidates the cache, including a
same-sized desktop reset. Each image is capped at 16 million pixels and 8192
pixels per dimension.

The bilinear pixel calculation packs two channels into independent 32-bit
lanes of a u64, preserving the original integer rounding. A regression compares
all 65,536 fraction pairs for contrasting and maximum channel values with the
four-channel scalar implementation. `benchmark_vnc_blend` isolates that kernel;
it does not measure a complete frame.

Wayland buffers are persistently mapped shared memory. Changed row runs are
copied directly into released buffers without per-frame file writes. Surface
damage is calculated against the last submitted image, independently of the
older buffer selected for reuse. This distinction handles A → B → A changes
without stale compositor content. Busy buffers remain untouched; a full pool
retains the existing redraw/retry behavior. Fullscreen changes coalesce into
one contiguous copy and one damage rectangle.

ZRLE solid tiles and RLE runs use doubling block copies after a single palette
validation per run. The persistent stream and malformed-input checks remain.

Local isolated release measurements for this second pass:

| Workload | Previous path | New path |
| --- | ---: | ---: |
| 4K full-change buffer submission | 10.300 ms | 4.663 ms |
| Expand 2040 solid ZRLE tiles | 119.312 ms | 1.063 ms |

These alternate execution order over six batches and report the upper median.
The first includes both damage comparisons and copying but excludes compositor
presentation. The second only measures solid-tile expansion, not compression,
transport or arbitrary website content, using the vendored crate's release
profile. Neither result is a remote FPS measurement.

A previous row-snapshot implementation's sparse-change 1080p → 720p benchmark measured 9.521 ms for an independent
full-image scalar reference versus 0.623 ms for the cached renderer. The
reference includes output allocation and is not the compiled minifb C scaler;
this demonstrates avoided work on sparse damage, not a general 15× FPS gain.

```sh
cargo test --manifest-path vendor/vnc-rs/Cargo.toml --release --locked benchmark_buffered_inflate -- --ignored --nocapture
cargo test --release -p fjern benchmark_vnc_scaled_damage --locked -- --ignored --nocapture
cargo test --manifest-path vendor/minifb/Cargo.toml --release --locked --lib benchmark_fullscreen_submission -- --ignored --nocapture
cargo test --manifest-path vendor/vnc-rs/Cargo.toml --release --locked --lib benchmark_block_runs -- --ignored --nocapture
cargo build --release -p fjern --locked
python3 tools/vnc_pipeline_smoke.py target/release/fjern
python3 tools/vnc_pipeline_smoke.py target/release/fjern --4k-scroll
```

The graphical smoke test opens a native client against a loopback server. It
exercises large Raw updates, overlapping CopyRect, persistent ZRLE, an
ExtendedDesktopSize change and subsequent refresh dimensions. It deliberately
ends the server connection and checks client termination. A local run passed
and reported a maximum event batch of 2.32 ms. It does not compare screenshots
or measure scanout; pixel equivalence is covered by unit tests.

For real-server diagnostics:

```sh
FJERN_VNC_STATS=1 ./target/release/fjern vnc workstation.example 5900
```

Every two seconds, stderr reports completed server updates per second (including
empty updates), paint attempts per second, maximum UI event-batch time, total
scaling time in the interval, and whether a Raw rectangle remains pending.
Paint attempts are not confirmed compositor frames. These counters do not
measure network latency or isolate decoder queue wait time. Real WayVNC video
FPS still requires a repeatable workload and before/after comparison.

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
