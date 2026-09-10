# Vendored dependencies

Fjern carries source dependencies only where local behavior is required. The
long-term preference is to upstream each patch and return to crates.io releases.

## minifb

- Name: `minifb`
- Upstream: https://github.com/emoon/rust_minifb
- Pinned version: 0.28.0 (see `Cargo.lock`)
- Why vendored: Wayland buffer lifecycle and keyboard-shortcut inhibition fixes.
- Local modifications: documented in [`minifb/FJERN-PATCH.md`](minifb/FJERN-PATCH.md).
- Tests: the `fjern_buffer_tests` and `fjern_keyboard_tests` modules, run by CI.
- Upstream issue/PR: not yet filed.
- Removal condition: an upstream release contains the required behavior and the native regression tests pass.
- License: MIT OR Apache-2.0; notices are retained in `vendor/minifb`.

## vnc-rs

- Name: `vnc-rs`
- Upstream: https://github.com/White-Oak/vnc-rs
- Pinned version: 0.5.3 (see `Cargo.lock`)
- Why vendored: bounded protocol handling and interoperability fixes.
- Local modifications: documented in [`vnc-rs/FJERN.md`](vnc-rs/FJERN.md).
- Tests: Fjern's VNC transport/viewer tests and the local smoke server.
- Upstream issue/PR: not yet filed.
- Removal condition: the changes ship upstream and the Fjern suite passes unchanged.
- License: MIT OR Apache-2.0; notices are retained in `vendor/vnc-rs`.
