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

The regression test uses a real libxkbcommon state and an embedded keymap, without
a display server. It covers Shift+letter, Shift+Tab, shifted digits and modifier
changes between press and release. Run:

```sh
cargo test --manifest-path vendor/minifb/Cargo.toml --locked --lib linrdp_keyboard_tests
```

This is a targeted compatibility patch, not a complete physical-key or
international-layout implementation. Replace the vendored copy when an upstream
version supplies equivalent behavior and passes these regressions.
