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
Authenticated sessions and Linux server
interoperability remain unverified. No installable distro packages have been published.

See [architecture](docs/architecture.md), [roadmap](docs/roadmap.md), and
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
