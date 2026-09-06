# MCS/GCC setup diagnostic

`session-probe` continues after NTLM CredSSP authentication on the same verified
TLS stream. Each read/write has a deadline and each TPKT frame is limited by its
16-bit length. Frames are read exactly, preserving any following TLS plaintext.
The ordinary `login` command still disconnects after authentication.

The current diagnostic sends MCS Connect Initial containing GCC core, security
and network data. The fixed profile requests 1024×768, 16-bit color and US keyboard
layout, uses the generic client name LinRDP, and requests no static virtual
channels. The security data specifies zero legacy encryption methods because
the transport is already TLS-protected. The selected protocol is included in
client core data.

The response decoder bounds BER and PER lengths, validates the server H.221 key,
requires one each of the core/security/network blocks, checks that the server
echoes the offered protocol flags and rejects legacy encryption on this TLS path.
It permits the two ignored user-data lengths specified by MS-RDPBCGR 3.2.5.3.4;
all enclosing frame lengths and the server block payload length are checked.
This is a narrow RDP codec, not a general ASN.1 BER/PER implementation.

After a valid response, the client sends Erect Domain and Attach User, then joins
the assigned user and I/O channels in sequence. Every confirmation must match
the expected user and channel. Rejection, truncation or mismatch ends the probe
without advancing to the next request. User and I/O channels must be distinct.

The shared setup code also supports one explicitly requested `cliprdr` channel
for `connect`, joining it after the user and I/O channels. A returned channel
that was not requested, a missing requested channel, or duplicate channel IDs
are rejected. `connect --size WIDTHxHEIGHT` supplies its chosen initial geometry.
The diagnostic keeps its fixed profile.

Current limits: only core, security and zero/one-static-channel network response
blocks are supported; unknown blocks are rejected. Fragmented PER lengths,
channel-join skipping and legacy RDP security are unsupported. This diagnostic
does not send Client Info, process licensing, negotiate capabilities, activate
a session, or exchange graphics/input. Requested geometry is not evidence of a
rendered desktop at that resolution.

Validation includes independently assembled response and channel vectors,
truncation and single-byte mutation coverage, invalid security and channel
responses, fragmented/coalesced stream reads and stop-on-error sequencing.
The Windows test report records separate real-host interoperability evidence.

References: [MCS Connect Initial](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-rdpbcgr/db6713ee-1c0e-4064-a3b3-0fac30b4037b),
[annotated server response](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-rdpbcgr/d23f7725-876c-48d4-9e41-8288896a19d3),
[MS-RDPBCGR sections 2.2.1.3–9, 3.2.5.3.4, 4.1.3–8](https://winprotocoldoc.z19.web.core.windows.net/MS-RDPBCGR/%5BMS-RDPBCGR%5D-220903.pdf).
