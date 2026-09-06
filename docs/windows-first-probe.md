# First Windows host check

Date: 2026-09-06. Client commit: `b3d9ff2fe466d88cfe7b64e480727b9dc0d58764`.
Target: user-provided Windows host on the local network, TCP port 3389.
The private address is intentionally omitted. Windows edition/build and
certificate configuration have not yet been recorded.

| Check | Result |
| --- | --- |
| RDP negotiation (`linrdp probe`) | Exit 0; server selected CredSSP (NLA) with early authorization |
| TLS using system trust (`linrdp tls`) | Exit 1; `invalid peer certificate: UnknownIssuer` |
| Credentials sent | None |
| NLA authentication / desktop | Not implemented or tested |

The first negotiation exchange interoperates with this host. Certificate
validation correctly prevents an untrusted connection from continuing. This
does not establish that the certificate is self-signed, nor that its hostname
or expiry would pass validation once its issuer is trusted.

## Follow-up certificate inspection

A separate Python/OpenSSL diagnostic retrieved the presented public certificate
after RDP negotiation. Certificate verification was disabled only in that
inspection process; no credentials or CredSSP messages were sent. The LinRDP
client's verifier was not changed, and the certificate was not trusted or
installed. The inspection negotiated TLS 1.3, which is not evidence of a
verified LinRDP TLS connection.

The presented certificate has matching subject/issuer common names, no X.509
extensions (including no Subject Alternative Name), and a validity interval
covering the test date. Matching names alone do not prove a self-signature.
Hostnames and certificate fingerprints are omitted from this public report.

Adding this certificate with `--ca` alone will not resolve the current SAN-based
identity check. Next: compare the SHA-256 fingerprint directly on the Windows
host through a trusted channel. Then either configure a certificate with a
matching SAN or implement explicit per-host certificate pinning with signature
verification. Do not automatically trust a certificate retrieved from the
network. No verified connection has yet succeeded.
