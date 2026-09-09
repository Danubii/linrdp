# Local changes

Vendored from vnc-rs 0.5.3 (MIT OR Apache-2.0).

`client/connection.rs`: update the stored screen dimensions when delivering
SetResolution through either recv_event or poll_event. Subsequent Refresh and
FullRefresh requests must cover the current server desktop, not ServerInit's
original size. Verified with a loopback RFB DesktopSize update and inspection
of the client's subsequent FramebufferUpdateRequest messages.
