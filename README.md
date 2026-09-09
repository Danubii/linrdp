# LinRDP

A simple native Linux remote-desktop client: enter a computer address, connect,
and get to work. Think VLC for remote desktops. Open source from the first
commit.

**Early development. RDP against Windows and VNC against WayVNC are working and
tested on real hosts.**

We are building our own RDP engine in Rust, interoperating with existing
Windows and Linux RDP servers. This project contains only a client.

LinRDP is developed first for [Omarchy](https://omarchy.org/) and its
Wayland/Hyprland desktop. Native windows, compositor shortcut capture, dynamic
resolution, clipboard integration and packaging are designed and tested with
that environment as the primary Linux target. Other Wayland desktops and X11
remain compatibility targets. LinRDP is an independent project and is not an
official Omarchy component.

## Direction

- A small native interface with useful defaults and clear errors.
- Responsive input, crisp text, correct keyboard layouts, and display scaling.
- Bidirectional text and RDP file clipboard, saved connections, and
  straightforward reconnection.
- RDP for Windows/Linux hosts and VNC for WayVNC and compatible RFB servers.
- Omarchy-first Wayland integration, with broader Wayland and X11 compatibility.
- Arch Linux packages and Debian/Ubuntu packages when the client is usable.

60 FPS is an initial performance target when the host and network permit it,
not a compatibility claim or a guarantee. Latency and frame pacing matter too.

## First-desktop viewer

Running `linrdp` with no arguments in an interactive terminal opens a compact,
keyboard-first connection screen. `linrdp tui` opens it explicitly. Enter a
Computer and User, connect, or manage saved non-secret profiles; Options exposes
port, initial size, dynamic resolution, clipboard, and certificate trust. Direct
CLI commands remain available, and noninteractive no-argument use still prints
help. For an unknown Windows certificate, the terminal interface can display its
verified details and fingerprint before the password prompt, then connect once
or save that exact trust decision. See
[terminal UI, trust, profile storage, and launcher installation](docs/terminal-ui.md).

```sh
cargo run --release -p linrdp -- connect my-computer.example --user 'MACHINE\tester' --ca /path/to/lab-ca.pem
```

Use the same explicit certificate-pin option as the diagnostics when appropriate.
Use `--size 1920x1080` to select the initial resolution (default 1024×768,
32-bit color). LinRDP requests Windows font smoothing and desktop composition
to preserve ClearType text. Dynamic resolution is implemented and enabled by default; use
`--dynamic-resolution off` to retain local scaling only. After the initial
connection, a window resize requests a matching remote resolution through the
Display Control channel when the server makes it available. Local scaling remains
the fallback when it does not, and only a single monitor is supported. This path
has protocol and local tests. A Windows-host run confirmed repeated grow and
shrink changes, restoration to the initial size, keyboard input and Unicode text
clipboard after resizing. Closing the window
disconnects without signing out. Basic keyboard, three mouse buttons and vertical
scrolling are implemented. Input goes to the focused session window.
Physical key forwarding, shifted keys, Tab, Omarchy/Super shortcuts and Danish
text input have been exercised against the current test hosts. IME remains
future compatibility work.
Wayland text and file copy/paste are enabled by default; use `--clipboard off`
to disable sharing. See [clipboard behavior and limits](docs/clipboard.md) and
[desktop scope and validation](docs/desktop.md). Local presentation improvements
and reproducible CPU benchmarks are described in [performance](docs/performance.md).
This branch also offers experimental `--graphics h264` (or H.264 in TUI Options).
Bitmap remains the default; see [H.264 negotiation and validation](docs/h264.md).

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

To perform TLS and verify the server certificate after RDP negotiation:

```sh
cargo run -p linrdp -- tls my-computer.example
cargo run -p linrdp -- tls my-computer.example 3389 --ca /path/to/lab-ca.pem
```

The `tls` command uses system trust roots by default. `--ca` replaces them with
the certificates in the specified PEM file for that invocation; it does not
modify the system trust store. Obtain this file through a trusted channel.
The certificate must be valid for the hostname or IP you supplied and must not
be expired. Self-signed hosts are not automatically trusted.

For an explicitly approved certificate without matching SANs, use
`--cert-sha256 <fingerprint>` instead of `--ca`. The fingerprint is the SHA-256
of the full DER leaf certificate (64 hex digits, optionally separated by colons
or hyphens). This pins the exact certificate for this invocation, replacing
CA/name checks while retaining validity and TLS signature verification.
It does not save trust or automatically accept changed certificates.
See [TLS and trust](docs/tls.md) for the trust policy.

TLS 1.2 and 1.3 are enabled, with a separate five-second handshake deadline.
Success means certificate verification and TLS completed, **not** that NLA,
user authentication or a desktop session succeeded. The command disconnects
after the handshake and never sends credentials.

The codec and transport have synthetic/loopback tests. The
[first Windows host check](docs/windows-first-probe.md) passed RDP negotiation and
TLS 1.3 with an explicitly selected certificate pin. System trust rejected the
issuer; the selected pin was not independently confirmed on Windows.
Windows authentication, activation, 32-bit bitmap display, dynamic resizing and
first desktop display have passed. WayVNC interoperability on an Omarchy host
has passed for TLS, authentication, resizing, keyboard capture, pointer input
and text clipboard. No installable distro packages have been published.

See the [changelog](CHANGELOG.md), [architecture](docs/architecture.md), [roadmap](docs/roadmap.md), and
[contributing](CONTRIBUTING.md). Licensed under [MIT](LICENSE).

## Experimental NLA diagnostics

```sh
cargo run -p linrdp -- nla-probe my-computer.example --ca /path/to/lab-ca.pem
cargo run -p linrdp -- login my-computer.example --user 'MACHINE\tester' --ca /path/to/lab-ca.pem
```

Use `--cert-sha256 <fingerprint>` instead of `--ca` for an explicitly approved
certificate pin. `nla-probe` obtains an NTLM challenge without credentials.
`login` prompts for a hidden password locally **after TLS verification**, then
attempts NTLM CredSSP and TLS binding once. It supports bare usernames or
`DOMAIN\username`, not UPN/Kerberos. Never put passwords in command arguments.

When the server supports early authorization, the result is checked. Otherwise,
the client reports credential delegation without claiming confirmed login.
Both commands disconnect afterward; no desktop session is started.

The full login exchange passes loopback tests using real TLS and NTLM crypto.
A real Windows standard-account test also passed NTLM authentication, CredSSP
binding and early authorization with an explicitly selected certificate pin.
See the [Windows test report](docs/windows-first-probe.md),
[CredSSP status](docs/credssp.md) and the
[test host guide](docs/test-hosts.md).

### Session setup diagnostic

```sh
cargo run -p linrdp -- session-probe my-computer.example --user 'MACHINE\tester' --ca /path/to/lab-ca.pem
```

This performs the same one-attempt login as `login`, then exchanges MCS/GCC
settings and joins the user and I/O channels over TLS. An explicitly selected
certificate pin can be used instead of `--ca`. The diagnostic requests a fixed
1024×768 desktop with 32-bit color and no static virtual channels. It disconnects
before client information, licensing or desktop activation; it does not display
a desktop. This path has passed against the Windows test host.

See [session behavior](docs/sessions.md) and [MCS/GCC scope](docs/mcs.md).

## VNC and WayVNC

[VNC connections](docs/vnc.md) are available through `linrdp vnc` or the
RDP/VNC selector in the TUI. The client supports VeNCrypt, saved certificate
trust, ZRLE/Raw/CopyRect graphics, server resizing, keyboard capture, pointer
input, scrolling and bidirectional text clipboard. The local client cursor
remains visible; start WayVNC without `--render-cursor` so the host cursor is not
composited into the framebuffer. Standard RFB does not provide MSTSC-style
clipboard file transfer; server-specific file extensions remain future work.
