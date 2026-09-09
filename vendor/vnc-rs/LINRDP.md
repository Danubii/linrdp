# Local changes

Vendored from vnc-rs 0.5.3 (MIT OR Apache-2.0).

`client/connection.rs`: update the stored screen dimensions when delivering
SetResolution through either recv_event or poll_event. Subsequent Refresh and
FullRefresh requests must cover the current server desktop, not ServerInit's
original size. Verified with a loopback RFB DesktopSize update and inspection
of the client's subsequent FramebufferUpdateRequest messages.

`client/auth.rs`: read the entire RFB 3.7/3.8 security offer and ignore unknown
alternatives when None or VNC password is offered. If there is no supported
choice, report the original numeric list. RFB 3.3 security values no longer
truncate from u32 to u8. Authentication result values are matched safely instead
of transmuting arbitrary server data into a two-variant enum.

`client/connector.rs`: reject a failed RFB 3.8 None SecurityResult rather than
proceeding to ServerInit. The unknown-security diagnostic spelling is corrected.

Unit test: `cargo test --manifest-path vendor/vnc-rs/Cargo.toml --lib unknown_offers`.
Loopback test: `python3 tools/vnc_smoke.py --auth --unknown` offers `[129, 2]`
and independently verifies that the client selects password authentication.
