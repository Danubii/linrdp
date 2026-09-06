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

Next: obtain the host's public certificate or issuing CA certificate through a
trusted channel, then repeat `tls` with `--ca`. Use a hostname or IP covered by
the certificate's subject alternative names. Do not disable verification.
