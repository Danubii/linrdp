# Session behavior

The `connect` command currently opens one read-only remote desktop window.
Closing that window disconnects without requesting sign-out. Other UI behavior
described below remains planned unless explicitly marked implemented.
The login diagnostic stops after successful early authorization. The separate
`session-probe` continues through basic settings exchange and channel setup.

## Initial experience

Start with one remote desktop per window. Multiple independent connections can
later run concurrently, each owning its transport, authentication context,
rendering state and input queue. Tabs can host the same session views later.
Focus must determine which connection receives keyboard input; changing focus
must release held remote keys. Clipboard integration must be scoped explicitly
to the active connection.

Closing a window should disconnect the client. Signing out is a separate,
explicit action because it ends the remote user's applications. The host's
session policies determine whether disconnected work remains available and for
how long; the client cannot promise indefinite persistence.

Reconnect should attempt to resume existing work where supported. Automatic
reconnection needs bounded backoff, cancellation and separate handling for
transport loss, authentication rejection and certificate changes. Do not retry
password failures automatically. Reconnection cookies are secrets and must not
be logged or stored as ordinary connection preferences.

## Later options

- Tabs and multiple independent windows.
- Fullscreen, dynamic resolution and selected local monitors.
- Saved connection profiles, with optional OS keyring credential storage.
- RemoteApp where the server publishes applications.
- Explicit administrative or session-sharing modes where supported and permitted.

These options require their respective protocol extensions and host testing.
A client connection is not the same as a server-side user session. The host
controls whether a connection resumes an existing session, creates one or is
refused. Concurrent user capacity depends on host edition, deployment and
policy. LinRDP does not override those decisions.

GNOME distinguishes desktop sharing from remote login. xrdp supports
reconnecting to existing sessions. Treat these as separate compatibility test
cases, rather than assuming all Linux RDP hosts have identical behavior.

## Implementation order and evidence

1. TPKT/X.224 data framing: implemented with independent wire-vector, malformed
   input, fragmentation and size-boundary tests; integrated into `session-probe`.
2. MCS/GCC settings exchange and channel setup: implemented over authenticated
   TLS, tested against Windows with the user and I/O channels.
3. Client information, valid-client licensing, capabilities and session activation: implemented.
4. Bitmap display in one window: implemented and visually verified against Windows.
   Keyboard/pointer input forwarding remains planned.
5. Disconnect/reconnect behavior against real hosts, then concurrent connections.

Authentication and early authorization have passed on a Windows test host.
Basic settings, channel setup, Client Info, licensing, activation and first
desktop display have also passed on that host. Closing the viewer exits with
status 0. Automatic reconnection and multi-session management are not implemented.

## References

- [RDP connection sequence](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-rdpbcgr/023f1e69-cfe8-4ee6-9ee0-7e759fb4e4ee)
- [MCS Connect Initial framing](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-rdpbcgr/db6713ee-1c0e-4064-a3b3-0fac30b4037b)
- [Remote Desktop Services roles](https://learn.microsoft.com/en-us/windows-server/remote/remote-desktop-services/rds-roles)
- [GNOME desktop sharing](https://help.gnome.org/gnome-help/sharing-desktop.html)
- [xrdp features](https://github.com/neutrinolabs/xrdp)
