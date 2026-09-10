# Security policy

## Reporting a vulnerability

Please report vulnerabilities privately through GitHub Security Advisories for
this repository. Do not open a public issue containing exploit details,
credentials, certificate material or host information. If private reporting is
not enabled, open a minimal public issue asking the maintainers to enable a
private reporting channel, without including sensitive details.

The latest released version and the current default branch receive security
fixes. Older development releases may require upgrading.

## Security model

- Passwords are requested locally after server identity verification. They are
  not accepted in command-line arguments, saved in profiles or written to logs.
- TLS is never silently downgraded. Certificate pins are scoped to a host and
  port, preserve validity and signature checks, and a changed pin is rejected.
- Fjern treats remote input, clipboard text and clipboard files as untrusted.
  Paths, file sizes and protocol allocations are bounded and validated.
- Configuration and certificate-trust files must be private (`0600`) inside a
  private directory (`0700`). Symlinks and linked trust files are rejected.
- Project crates forbid unsafe Rust. Any future exception requires explicit
  security review and documentation.

Fjern's file-transfer implementation confines staged remote files to temporary
storage and validates paths before publication. Users should still inspect
remote clipboard content before opening or executing it.
