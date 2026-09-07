# LinRDP vendor patch

Source: minifb 0.28.0 from crates.io, https://github.com/emoon/rust_minifb.
The upstream MIT and Apache-2.0 license files are retained.

The Wayland backend originally translated key transitions using the effective
XKB symbol. Its key table recognizes lowercase letters and Tab, so Shift could
hide a press, hide a release, or turn Tab into the unrecognized ISO_Left_Tab.
This patch resolves key transitions at level zero in the active layout;
character callbacks still use the effective symbol. Modifier transitions remain
separate. No compositor configuration is changed.

The Wayland and X11 backends also retain native left, middle and right mouse
button down/up events in a bounded 192-edge queue, including short clicks or
double-click sequences completed between application render frames. LinRDP
drains those ordered edges when polling input. Queue overflow is reported so the
session fails rather than silently losing a press or release. This behavior has
local regression coverage; an actual Windows GUI double-click has not yet been
verified.

The POSIX `needs_redraw` query lets idle clients skip identical uploads while
still answering Wayland configure events and repainting X11 exposure events.
X11 requests a redraw on configure, including events that leave dimensions
unchanged, and clears the request before dispatching new events after a blit.
Wayland keeps requesting a redraw until submission acknowledges the latest
configure serial. Its shared-memory pool also marks reused buffers busy again
and installs the replacement buffer's release state after a resize, preventing
writes into storage still owned by the compositor.
The pool is capped at three buffers. If all are busy, submission is deferred
while input and release events continue dispatching; `needs_redraw` stays set
until the latest image is submitted, including a final static frame.

Run the bounded-pool unit test with:

```sh
cargo test --manifest-path vendor/minifb/Cargo.toml --locked --lib linrdp_buffer_tests
```

An additional ignored test opens a small Wayland window and checks actual
release/retry behavior: append `-- --ignored --nocapture` to that command.

The regression test uses a real libxkbcommon state and an embedded keymap, without
a display server. It covers Shift+letter, Shift+Tab, shifted digits and modifier
changes between press and release. Run:

```sh
cargo test --manifest-path vendor/minifb/Cargo.toml --locked --lib linrdp_keyboard_tests
```

This is a targeted compatibility patch, not a complete physical-key or
international-layout implementation. Replace the vendored copy when an upstream
version supplies equivalent behavior and passes these regressions.
