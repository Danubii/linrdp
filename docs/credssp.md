# CredSSP implementation status

The protocol crate now encodes and decodes the TSRequest envelope used during
CredSSP/NLA. This is not a working authentication client yet, and the diagnostic
commands do not send these envelopes.

## Implemented

- Explicit ASN.1 tags for the version, negotiation tokens, sealed credentials,
  sealed public-key binding, server status and 32-byte client nonce.
- DER encoding/decoding through RustCrypto's `der` library, with borrowed opaque
  byte fields. Debug output reports presence/counts without payloads.
- Partial-header inspection for future TLS stream framing. This reports the
  length of one message; it does not read sockets or consume subsequent frames.
- Strict field ordering, no duplicates/unknown fields, and no trailing data.
- A local limit of 1 MiB per message and 16 negotiation tokens, enforced on both
  encoding and decoding. Token decoding uses bounded storage.
- Unsigned NTSTATUS decoding, including the ASN.1 sign-padding byte when needed.
- A peer-policy helper that stops on any present error code and requires version
  5 or newer, negotiating at most version 6. Older messages can be decoded for
  diagnostics but are not accepted for authentication.
- TLS resumption disabled in preparation for CredSSP, per MS-CSSP 3.1.5.

## Remaining before login

1. SPNEGO and an authentication provider for NTLM/Kerberos token exchange.
2. Authentication-provider signing/sealing and TLS public-key binding, including
   random nonce generation and verification of the server's binding response.
3. A state machine that rejects unexpected messages, enforces deadlines and
   releases credentials only after the binding proof succeeds.
4. Protected password entry and credential encoding/zeroization.
5. TLS stream integration and real Windows/Linux interoperability tests.

`auth_info` and `pub_key_auth` are opaque fields, not cryptographic protection.
Encoding them does not seal their contents or authenticate their origin. The
wire codec does not decide which fields are legal at each exchange phase; the
future session state machine must enforce that.

## References

- [TSRequest, MS-CSSP 2.2.1](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-cssp/6aac4dea-08ef-47a6-8747-22ea7f6d8685)
- [NegoData, MS-CSSP 2.2.1.1](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-cssp/9664994d-0784-4659-b85b-83b8d54c2336)
- [Sequencing, MS-CSSP 3.1.5](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-cssp/385a7489-d46b-464c-b224-f7340e308a5c)
- [RustCrypto DER](https://docs.rs/der/0.7.10/der/)
