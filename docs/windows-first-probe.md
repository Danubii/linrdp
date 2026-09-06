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
network. No verified connection had succeeded at this stage.

## Explicit certificate pin test

Implementation: `cf3e56e`. After being informed that the fingerprint had not been
compared directly on Windows, the user chose to proceed with the previously
observed certificate for this test. No independent fingerprint confirmation is
claimed. The SHA-256 value was supplied explicitly for this invocation, without
installing certificates or persisting trust.

| Check | Result |
| --- | --- |
| `linrdp tls <host> 3389 --cert-sha256 <selected-fingerprint>` | Exit 0 |
| RDP negotiation | CredSSP (NLA) with early authorization |
| TLS | TLS 1.3, `TLS13_AES_256_GCM_SHA384` |
| Verification | Exact leaf SHA-256 match, certificate validity, TLS handshake signature |
| CA/SAN validation | Replaced by the explicit certificate pin |
| Credentials sent | None |

This establishes a working LinRDP TLS handshake with the host presenting the
selected certificate and proving possession of its private key. It does not
establish independently confirmed host identity, NLA login or desktop support.
Local validation also passed 42 tests, Clippy, formatting and a release build.

## Credential-free NLA probe

Implementation: `d9c33ef`. The new `nla-probe` command was tested against the
same Windows host with the same explicitly selected certificate pin. This
adds a real CredSSP request/response over TLS without attempting account login.

| Check | Result |
| --- | --- |
| `linrdp nla-probe <host> 3389 --cert-sha256 <selected-fingerprint>` | Exit 0 |
| TLS | TLS 1.3, `TLS13_AES_256_GCM_SHA384` |
| CredSSP | Server version 6 |
| NTLM | Type 2 challenge received after the credential-free Type 1 token |
| Challenge policy | Required signing, sealing, extended-session security, target info, 128-bit security and key exchange present |
| Credentials / Type 3 response sent | None |

The complete login path passes local tests using real TLS and NTLM contexts,
including binding, credential delegation and early authorization. Those peers
are synthetic fixtures, not Windows account validation. Real-host account login
has not been attempted. Local checks passed 56 tests, Clippy, formatting and a
release build. Host identity remains based on the previously selected pin,
without independent fingerprint confirmation on Windows.
