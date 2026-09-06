# Roadmap

All items below are planned unless explicitly marked complete.

## 0 — Foundation

- [x] Public repository, MIT license, Rust workspace and CI.
- [x] TPKT/X.224 security negotiation codec and diagnostic command.
- [x] Malformed input tests and local transport tests.

## 1 — First authenticated session

- [x] TLS diagnostic with certificate validation and explicit PEM trust.
- [x] Explicit per-invocation SHA-256 certificate pinning for SAN-less hosts.
- Graphical certificate review and persistent per-host trust handling.
- [x] CredSSP TSRequest codec, bounded framing and peer version/status policy.
- [x] CredSSP v5/v6 TLS binding state machine behind a provider interface.
- [x] NTLM provider, TLS integration and hidden-password login diagnostic.
- [x] Credential-free NLA probe against Windows.
- [x] Verify actual Windows NTLM account authentication and CredSSP binding.
- Verify successful early authorization with an RDP-authorized account.
- MCS/GCC connection setup, capabilities and session activation.
- Basic bitmap output and keyboard/pointer input.
- Demonstrate an actual Windows desktop session before calling this usable.

## 2 — Simple desktop client

- Native connection window: address, connect, credentials.
- Session window, resize, fullscreen and useful connection errors.
- Validate against Windows RDP, xrdp and GNOME Remote Desktop.
- Test Danish/US keyboards, Wayland/X11, scaling and network interruption.

## 3 — Everyday use

- Clipboard, audio, recent connections and opt-in keyring integration.
- Graphics pipeline and hardware-assisted decoding where supported.
- Measure delivered FPS, frame pacing, memory and input-to-display latency.
- Compare with an established client on identical hosts and networks.
- Build/install/remove tests for Arch and Debian/Ubuntu packages.

## Compatibility evidence

The [first Windows host check](windows-first-probe.md) passed RDP negotiation and
TLS 1.3 with an explicitly selected certificate pin, after system trust rejected
the issuer. The pin was not independently confirmed on Windows. A subsequent standard-account test completed NTLM authentication and CredSSP
binding, then received `AUTHZ_ACCESS_DENIED` during early authorization.
Successful session authorization remains unverified.
Record server OS/version, client display system, authentication mode, resolution,
codec and outcome for each future run.
Loopback fixtures establish protocol behavior only, not server interoperability.
Use the [test host guide](test-hosts.md) to prepare and record real host checks.
