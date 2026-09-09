# Terminal connection screen

Run `linrdp` with no arguments in an interactive terminal, or run `linrdp tui`
explicitly, to open the terminal connection screen. With redirected input or
output, no arguments continue to print command-line help. Existing commands such
as `linrdp connect` remain available for scripts and direct use.

The screen follows the familiar Remote Desktop Connection order: Computer,
User, then Connect. Tab and Shift+Tab move between controls, Enter activates the
focused control, arrow keys move through saved connections, and Esc exits.
Ctrl+U clears the focused text field or profile-name prompt. Open
Options to set the port, initial size, dynamic resolution, clipboard sharing,
and certificate trust. Trust uses system roots by default; an advanced connection
can select a CA file. Manual certificate fingerprints remain available to the
explicit command-line interface, not as a terminal-form field.
On the H.264 branch, Options also includes an H.264 switch, off by default.
It requests the experimental graphics profile and is saved with the connection;
the server still selects the actual codec. See [H.264](h264.md).

When default system trust rejects only an unknown issuer or a certificate name
mismatch, the terminal interface opens a fresh credential-free RDP/TLS probe. It
verifies the certificate's TLS signature and validity, then shows the destination,
subject, issuer, validity period and complete SHA-256 fingerprint before any
password prompt. Cancel is the default. Connect once retries a fresh connection
with that exact certificate; Trust and save does the same and records the pin
only after the pinned TLS handshake succeeds. The latter does not wait for login
or the desktop session to finish.

Expiry, invalid signatures, network errors and authentication errors never open
the approval dialog. Explicit `--ca` and `--cert-sha256` connections remain
strict and never fall back to discovery. A saved certificate mismatch also stops
with an error: LinRDP does not silently replace the pin or prompt to renew it.
The noninteractive CLI never performs certificate discovery or trust prompting.

Approved certificates are stored separately in
`$XDG_CONFIG_HOME/linrdp/known_hosts.json`, falling back to
`$HOME/.config/linrdp/known_hosts.json`. Each pin applies only to its host and
port. The directory uses mode 0700 and files use mode 0600; updates are locked
and atomic. Invalid trust data produces an error instead of being reset.

Save creates a named connection. Edit loads the selected connection and changes
Save to Update; Save as creates a separate copy. Delete always asks for
confirmation. After a connection closes or fails, LinRDP restores the terminal
screen and returns to the connection screen with the result. The terminal is
restored before the hidden password prompt, network connection, or desktop
window starts.

Profiles contain connection settings only. LinRDP never saves passwords. It
stores at most 100 validated profiles in
`$XDG_CONFIG_HOME/linrdp/profiles.json`, or
`$HOME/.config/linrdp/profiles.json` when `XDG_CONFIG_HOME` is unset. The
directory and file use private permissions. Updates use a temporary file in the
same directory, flush it, and atomically replace the profile file. A missing file
means there are no saved connections; malformed, oversized, duplicate, or
otherwise invalid data produces an explicit error instead of being ignored.

The interface uses the terminal's own foreground and background colors, with
bold and reverse-video focus. It therefore follows the active Omarchy terminal
palette without changing user configuration.

## Application launcher

The repository includes [`contrib/linrdp.desktop`](../contrib/linrdp.desktop).
After installing the `linrdp` executable somewhere on `PATH`, install the
launcher for the current user with:

```sh
install -Dm644 contrib/linrdp.desktop \
  "$HOME/.local/share/applications/linrdp.desktop"
```

The launcher uses `Terminal=true` and `Exec=linrdp tui`, so desktop launchers,
including Omarchy's launcher, start the terminal connection screen. This is an
optional user action; the project does not modify desktop configuration or
install system files automatically. The remote desktop uses an ordinary native
window whose decorations and window actions are managed by Omarchy.
