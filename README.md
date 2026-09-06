# LinRDP

A simple Linux RDP client: enter a computer address, connect, and get to work.
Think VLC for RDP. Open source from the first commit.

**Early development. No desktop sessions or graphical interface yet.**

We are building our own RDP engine in Rust, interoperating with existing
Windows and Linux RDP servers. This project contains only a client.

## Direction

- A small native interface with useful defaults and clear errors.
- Responsive input, crisp text, correct keyboard layouts, and display scaling.
- Clipboard, audio, saved connections, and straightforward reconnection.
- Wayland and X11 clients; Windows and Linux remote hosts.
- Arch Linux packages and Debian/Ubuntu packages when the client is usable.

60 FPS is an initial performance target when the host and network permit it,
not a compatibility claim or a guarantee. Latency and frame pacing matter too.

## Development

Install stable Rust using [rustup](https://rustup.rs/), then run:

```sh
cargo build --workspace --locked
cargo test --workspace --locked
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
```

See [architecture](docs/architecture.md), [roadmap](docs/roadmap.md), and
[contributing](CONTRIBUTING.md). Licensed under [MIT](LICENSE).
