# First Windows host check

Date: 2026-09-06. Client commit: `b3d9ff2fe466d88cfe7b64e480727b9dc0d58764`.
Target: user-provided Windows host on the local network, TCP port 3389.
The private address is intentionally omitted. Windows edition/build and
certificate configuration have not yet been recorded.

| Check | Result |
| --- | --- |
| RDP negotiation (`fjern probe`) | Exit 0; server selected CredSSP (NLA) with early authorization |
| TLS using system trust (`fjern tls`) | Exit 1; `invalid peer certificate: UnknownIssuer` |
| Credentials sent | None |
| NLA authentication / desktop | Not implemented or tested |

The first negotiation exchange interoperates with this host. Certificate
validation correctly prevents an untrusted connection from continuing. This
does not establish that the certificate is self-signed, nor that its hostname
or expiry would pass validation once its issuer is trusted.

## Follow-up certificate inspection

A separate Python/OpenSSL diagnostic retrieved the presented public certificate
after RDP negotiation. Certificate verification was disabled only in that
inspection process; no credentials or CredSSP messages were sent. The Fjern
client's verifier was not changed, and the certificate was not trusted or
installed. The inspection negotiated TLS 1.3, which is not evidence of a
verified Fjern TLS connection.

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
| `fjern tls <host> 3389 --cert-sha256 <selected-fingerprint>` | Exit 0 |
| RDP negotiation | CredSSP (NLA) with early authorization |
| TLS | TLS 1.3, `TLS13_AES_256_GCM_SHA384` |
| Verification | Exact leaf SHA-256 match, certificate validity, TLS handshake signature |
| CA/SAN validation | Replaced by the explicit certificate pin |
| Credentials sent | None |

This establishes a working Fjern TLS handshake with the host presenting the
selected certificate and proving possession of its private key. It does not
establish independently confirmed host identity, NLA login or desktop support.
Local validation also passed 42 tests, Clippy, formatting and a release build.

## Credential-free NLA probe

Implementation: `d9c33ef`. The new `nla-probe` command was tested against the
same Windows host with the same explicitly selected certificate pin. This
adds a real CredSSP request/response over TLS without attempting account login.

| Check | Result |
| --- | --- |
| `fjern nla-probe <host> 3389 --cert-sha256 <selected-fingerprint>` | Exit 0 |
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

## First account login attempt

Date: 2026-09-06. The user supplied an account and password for the same
Windows host. The password was entered through the hidden terminal prompt,
not command-line arguments or repository files. Account details are omitted.

An initial connection closed while waiting at the password prompt. The next
connection reached the authentication response, but the unsigned-only ASN.1
status decoder rejected an INTEGER. After extending the decoder to accept
signed 32-bit NTSTATUS representations as well as positive unsigned values,
one further attempt returned `0xc000006d` (`STATUS_LOGON_FAILURE`).

TLS verification with the selected certificate pin succeeded. Windows account
login did not succeed, and neither credential delegation nor desktop activation
was reached. The failure alone does not distinguish incorrect credentials,
account naming or client NTLM interoperability. No further account retries were
made. Confirm the Windows account authority and inspect the corresponding
Windows logon failure event before choosing the next authentication test.

The decoder fix retains strict DER validation and rejects values outside the
signed/unsigned 32-bit range. Independent signed-status and overflow vectors
were added. All 58 tests, Clippy and the release build passed locally.

## Standard-user authentication and authorization check

Date: 2026-09-06. Client: `631ba91`. A user-provided standard test account
was tested once with the same explicit certificate pin. Account identifiers
and credentials are omitted from this public report.

TLS verification, NTLM authentication and verification of the server's CredSSP
binding proof completed. The client sent sealed delegated credentials, then
received early authorization result `0x00000005` (`AUTHZ_ACCESS_DENIED`) and
exited with failure. No desktop session was started.

This establishes real Windows authentication and binding interoperability,
but not permission to start an RDP session. Check membership in Remote Desktop
Users and the effective remote-logon allow/deny policies on the host before
retesting. Administrator membership is not required for an ordinary desktop
user with the appropriate remote-logon rights.

References: [early authorization processing](https://winprotocoldoc.z19.web.core.windows.net/MS-RDPBCGR/%5BMS-RDPBCGR%5D-220903.pdf),
[Microsoft RDS authorization troubleshooting](https://learn.microsoft.com/en-us/troubleshoot/windows-server/remote/troubleshooting-access-denied-and-user-not-authorized-rds-issues).

## Successful standard-user authorization

Date: 2026-09-06. Client implementation: `631ba91` (documentation HEAD
`bf0cfdc`). After the user reported granting RDP access to the standard test
account, one further login attempt against the same host exited successfully.
The previously selected certificate pin was used again; it remains without
independent fingerprint confirmation. No account identifiers or passwords are
included in this report.

| Check | Result |
| --- | --- |
| RDP negotiation | CredSSP (NLA) with early authorization |
| TLS | TLS 1.3, `TLS13_AES_256_GCM_SHA384`; explicit certificate pin verified |
| NTLM authentication and server CredSSP binding proof | Succeeded |
| Sealed credential delegation | Sent |
| Server early authorization | Succeeded (`0x00000000`) |
| Client exit code | 0 |
| Desktop session | Not started; diagnostic disconnects after authorization |

This confirms the complete authentication and early authorization path against
this Windows test host. It does not validate MCS/GCC setup, session activation,
graphics or input, which remain unimplemented. This was a real-host diagnostic
run with the existing release binary; no implementation changes were required.

## Successful MCS/GCC settings and channel setup

Date: 2026-09-06. The `session-probe` implementation accompanying this report
was tested once with the existing standard account and selected certificate
pin. Private host/account identifiers and credentials are omitted.

| Check | Result |
| --- | --- |
| TLS, CredSSP binding and early authorization | Succeeded |
| MCS Connect Initial / GCC settings response | Accepted |
| Server core version | `0x00080011` (wire value; not an OS build identification) |
| MCS Attach User | User channel 1004 assigned |
| User channel join | Confirmed for channel 1004 |
| I/O channel join | Confirmed for channel 1003 |
| Client exit code | 0 |
| Requested display | 1024×768, 16-bit color; not yet rendered |
| Desktop activation / input / graphics | Not performed |

This validates basic settings exchange and the two mandatory channel joins
against the Windows host over the authenticated TLS connection. The diagnostic
stops before Client Info, licensing and capability exchange. No user desktop
was activated. Pin trust remains explicitly selected and was not independently
confirmed on Windows. Local validation passed 69 tests, Clippy, formatting and
a release build.

## First native desktop display

Date: 2026-09-06. The `connect` viewer and fast-path implementation accompanying
this report were tested against the same Windows host and standard account.
The client ran on Arch Linux with Hyprland/Wayland. The precise Windows edition
and build were not collected. Private captures and credentials are excluded
from the repository; certificate-pin provenance remains as described above.

| Check | Result |
| --- | --- |
| TLS / NTLM CredSSP / early authorization | Succeeded |
| MCS/GCC and mandatory channel joins | Succeeded |
| Client Info / valid-client licensing | Accepted |
| Demand Active / Confirm Active / synchronization / control / Font Map | Completed |
| First bitmap | Displayed in the native Fjern window |
| Negotiated display | 1024×768, 16-bit RGB565 |
| Output profile | Fast-path bitmap updates, interleaved RLE enabled, bulk compression disabled |
| Visual inspection | Actual Windows wallpaper, desktop icons and taskbar |
| Local display scaling | Aspect ratio preserved in a 949×1045 window |
| Static desktop / normal window close | Remained connected through idle; close exited with status 0 |
| Manual reconnect | Authenticated again and displayed the Windows desktop |
| Keyboard / mouse forwarding | Not implemented or tested |

A slow-path-only activation initially produced no image. A temporary FreeRDP
3.30.0 reference client displayed the desktop with its default fast-path
settings; a configuration disabling both fast-path input and output was black.
Enabling fast-path **output** in Fjern resolved the first-image failure while
leaving its input transport unchanged. The reference client is not a project
runtime dependency and was not used to render Fjern's verified image.

The viewer closes the connection without sending logoff. This test does not
establish automatic reconnection, application-state persistence, Linux-server
compatibility, interactive usability or performance/FPS guarantees. minifb's
Wayland backend printed proxy-cleanup warnings on normal closure; exit status
was still 0. Local checks passed 79 tests, formatting, Clippy with warnings
denied, and a locked release build.

## Basic interactive input

Date: 2026-09-06. The same authenticated Windows session was tested from the
Hyprland/Wayland client with the input implementation accompanying this report.
The initial US keyboard profile and 1024×768 RGB565 desktop remained unchanged.

| Check | Result |
| --- | --- |
| Ctrl+Escape / Escape | Opened and dismissed the Windows Start menu |
| Mouse position and left click | Clicked the Start button in a centered, scaled viewport |
| Short keyboard events | Complete `notepad` search text arrived after switching to ordered callbacks |
| Text entry | `fjern input test 123` appeared in a new Notepad document |
| Focus loss with Shift held | Focus moved to another local window before Shift release; subsequent `a` arrived lowercase |
| Normal viewer close | Exit status 0; no logoff requested |
| Local validation | 87 tests, formatting, Clippy with warnings denied, locked release build |

The first polling-only keyboard attempt missed short taps. The final keyboard
path uses minifb callbacks and preserves both edges and their order independently
of rendering. A bounded callback queue rejects overflow, and the network worker
tracks sent key/button state for best-effort release on closure. Mouse buttons
are still sampled per UI frame; very short clicks remain a limitation.

Keyboard injection used temporary Wayland test tools rather than bypassing the
client's window input path. Screenshots stayed local. Tests do not establish
Danish-layout support, IME, all special keys, X11 input compatibility or a latency
benchmark. See [desktop input scope](desktop.md#basic-interactive-input).

### Wheel and synthetic burst checks

Vertical Wayland wheel input moved the scratch document downward from its first
line. Temporary event counters and a rolling checksum matched all 479 expected
scancode presses and releases in a 60-line synthetic typing test, but Notepad
lost characters for effectively simultaneous press/release events. Sending
one input event per PDU did not resolve this; the final client keeps ordered
batches. Temporary counters/checksums were removed from the implementation.

A FreeRDP 3.30.0 reference run also lost characters with zero-dwell XTest events
(376 characters displayed versus 479 expected). A reference run with 5 ms between
edges delivered the 479-character sequence. The reference required XTest rather
than wtype because its X11 physical-key mapping interpreted wtype's temporary
keymap differently. This narrows the evidence to this host/application/test
method; it does not prove a Windows-wide limit or exclude Fjern timing issues.
Rapid synthetic bursts remain a documented compatibility limitation.

A follow-up corrected outgoing Share Data `uncompressedLength` to the payload
length, added a wire assertion, suppressed meaningless modifier/lock-key repeats,
and checked cancellation between queued input batches. These changes do not
claim to fix the synthetic burst behavior. The temporary test document was
cleared after testing; private captures and test injection helpers are not
project dependencies or public artifacts.


## Resolution, clipboard and shifted keys

On 2026-09-06, the Windows host accepted requested desktop dimensions of
1920×1080 and 1280×800 at 16-bit color. These were connection-time settings.
Single-monitor dynamic resolution has since been implemented, with local scaling
as its fallback. A later Windows-host run negotiated Display Control and changed
the actual remote framebuffer through 960×1056, 860×1056, 1160×1056, 1024×768,
1280×800, 900×600 and 1280×800. Each change was confirmed after the server
deactivated and reactivated the desktop. The dynamic-channel transport required
outgoing `drdynvc` fragments to omit `SHOW_PROTOCOL`; clipboard fragments
continue to use it.

After the final resize, Windows received keyboard input producing `aA`, Tab and
`b`; copying it back yielded exactly `aA\tb`. Unicode multiline clipboard text
also completed an exact Linux→Windows→Linux round trip, including
`ÆØÅ/æøå`. File clipboard transfers were not repeated during this dynamic-
resolution run; the earlier file evidence below remains valid and separate.

Unicode clipboard text passed in both directions, including Danish characters
and line breaks. A folder copied in Nautilus was pasted in Windows File Explorer,
then copied back and pasted in another Nautilus directory. All file SHA-256
hashes and relative paths matched, including a 150,001-byte binary, a Danish
filename, an empty file and an empty directory. The transfer required fixes for
virtual-channel priority handling, SHOW_PROTOCOL on outgoing fragments, and the
Windows four-byte trailer outside CLIPRDR dataLen. No shared drive was used.
See [clipboard scope](clipboard.md) for limits and remaining work.

Windows Notepad displayed lowercase and shifted uppercase letters with an actual
Tab; copying the result back yielded exactly `aA\tb` (four characters).
The Wayland library's original effective-symbol lookup failed a native XKB
regression for shifted letters and Shift+Tab. The vendored base-level lookup
passes that test, including modifier changes between press and release.

Holding Shift while holding A also produced repeated uppercase letters; releasing
Shift and typing B produced a lowercase final character. The copied-back result
matched 19 uppercase A characters followed by lowercase b.
