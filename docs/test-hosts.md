# Preparing interoperability test hosts

Start with one Windows host, then add a Linux host. Keep normal RDP security
settings enabled. The current commands check negotiation and TLS only; they
cannot authenticate a user or display a desktop yet.

## Windows

Use a Windows installation that supports incoming Remote Desktop, with RDP and
Network Level Authentication enabled. Prepare a separate non-administrator test
account with permission to log on through Remote Desktop. Keep its password
local; the current client has no password-entry support.

Record the Windows edition/build, hostname/IP, RDP port and whether the test
account is local or domain-based. Note whether an existing client can connect.

## Linux

Use an existing xrdp or GNOME Remote Desktop installation. Record the distro,
version, RDP server/version, hostname/IP and port. For GNOME, distinguish desktop
sharing from remote login. Record the desktop environment and Wayland/X11 mode
on the remote host, and whether an existing client can connect.

## First checks

Run from the Linux development machine, substituting the real host and port:

```sh
cargo run -p linrdp -- probe rdp-host.example 3389
cargo run -p linrdp -- tls rdp-host.example 3389
```

If the host uses a private CA or self-signed certificate, obtain the public
certificate through a trusted channel and test explicit trust:

```sh
cargo run -p linrdp -- tls rdp-host.example 3389 --ca /path/to/trusted-public-certificate.pem
```

Never provide the private key. An explicitly trusted certificate must still
match the supplied hostname/IP and satisfy rustls's validation rules. Record a
validation failure as a test result; do not disable certificate verification.
See [TLS and trust](tls.md) for limitations.

For an explicitly approved certificate without SANs, replace `--ca <file>` with
`--cert-sha256 <fingerprint>`. Use the SHA-256 hash of the full DER certificate,
not its SHA-1 thumbprint or a public-key hash. Record how the fingerprint was
obtained and whether it was independently confirmed on the host. This mode
checks the exact certificate, its validity and TLS handshake signatures; it
replaces issuer-chain and name checks and does not persist trust.

## Result template

Copy this into a local test note. Redact private hostnames/IPs before publishing
results; do not commit credentials or private captures.

```text
Date:
LinRDP commit:
Client distro/version:
Client desktop and Wayland/X11:
Server OS/edition/build:
RDP server/version:
Remote login or desktop sharing:
Local or domain test account (no password):
Baseline client/version and connection result:
Probe result and exit code:
TLS result and exit code:
Trust source (system / explicit PEM / certificate pin):
Fingerprint provenance and independent confirmation (if pinned):
Certificate DNS/IP match:
Notes:
```
