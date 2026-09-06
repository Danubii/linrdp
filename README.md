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

The first development tool performs the initial RDP security negotiation:

```sh
cargo run -p linrdp -- probe 192.0.2.10
cargo run -p linrdp -- probe my-computer.example 3389
cargo run -p linrdp -- probe ::1 3389
```

Replace the example address with your RDP host. This sends one negotiation
request and reports the server's chosen security mode, then disconnects.
It does **not** establish TLS, authenticate the server, send credentials or
display a desktop. Legacy RDP security is rejected. TCP connection attempts
share a five-second budget; reading the response has a separate five-second
deadline. System DNS resolution is outside these deadlines.

The codec and transport have synthetic/loopback tests. Real Windows and Linux
server interoperability is not yet verified. No installable distro packages
have been published.

See [architecture](docs/architecture.md), [roadmap](docs/roadmap.md), and
[contributing](CONTRIBUTING.md). Licensed under [MIT](LICENSE).
