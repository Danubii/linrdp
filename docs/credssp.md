# CredSSP implementation status

The protocol crate encodes/decodes TSRequest and implements the v5/v6 TLS binding
stage behind an authentication-provider interface. This is not a working login
client yet, and the diagnostic commands do not send these envelopes.

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
- Fresh 32-byte nonces from the OS cryptographic random source and directional
  SHA-256 binding hashes, including the required null-terminated labels.
- A binding state machine: ready, awaiting server proof, verified, delegated,
  failed. Every operation error makes the exchange terminal. Credentials cannot
  reach the sealing provider through this API before the server proof verifies.
- Constant-time comparison of the unsealed server hash. Reflected client hashes,
  wrong keys/nonces, changed negotiated versions and unexpected fields fail.
- Extraction of the SubjectPublicKey BIT STRING contents from the verified TLS
  leaf certificate. The entire certificate and SPKI wrapper are not hashed.

The binding stage requires a completed authentication context implementing
`Protection`. That provider must perform real signing/sealing, verify signatures
before releasing plaintext and enforce directional keys and sequence numbers.
Tests use a scripted provider, which is not authentication or encryption.
Callers must supply the verified TLS key and complete token exchange first.
`Delegated` means an outbound message was constructed, not that login succeeded.

## Remaining before login

1. SPNEGO and an authentication provider for NTLM/Kerberos token exchange.
2. A real provider's signing/sealing implementation connected to the binding stage.
3. Token-exchange state handling, transport deadlines and integration with the
   existing binding state machine.
4. Protected password entry and credential encoding/zeroization.
5. TLS stream integration and real Windows/Linux interoperability tests.

`auth_info` and `pub_key_auth` are opaque fields, not cryptographic protection.
Encoding them does not seal their contents or authenticate their origin. The
wire codec does not decide which fields are legal at each exchange phase; the
binding state machine enforces its own phase rules, and token exchange still
needs corresponding enforcement.

## References

- [TSRequest, MS-CSSP 2.2.1](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-cssp/6aac4dea-08ef-47a6-8747-22ea7f6d8685)
- [NegoData, MS-CSSP 2.2.1.1](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-cssp/9664994d-0784-4659-b85b-83b8d54c2336)
- [Sequencing, MS-CSSP 3.1.5](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-cssp/385a7489-d46b-464c-b224-f7340e308a5c)
- [RustCrypto DER](https://docs.rs/der/0.7.10/der/)
