# Platform validation

“Verified” means exercised on a real host. Unit or protocol tests alone are not
recorded as host verification.

| Feature | Omarchy / Hyprland | Other Wayland | X11 | Notes |
|---|---|---|---|---|
| RDP Windows desktop | Verified | Not verified | Not verified | Real Windows host |
| VNC WayVNC | Verified | Not verified | N/A | Real Omarchy host |
| Keyboard, including Danish layout | Verified | Expected | Not verified | Compositor dependent |
| Super-key capture | Verified | Experimental | N/A | Hyprland shortcut inhibition |
| Mouse and scrolling | Verified | Expected | Not verified | |
| Text clipboard | Verified | Expected | Not verified | |
| RDP file clipboard | Verified | Expected | Not verified | VNC file transfer unsupported |
| Dynamic RDP resolution | Verified | Expected | Not verified | One monitor only |
| VNC resize and scaling | Verified | Expected | Not verified | |
| H.264 / AVC420 | Experimental | Experimental | Experimental | Software decoding |
| Multi-monitor | Not supported | Not supported | Not supported | |
| ARM64 | Not verified | Not verified | Not verified | No release claim |

Fjern is an independent project and is not an official Omarchy component.

## Local validation record — 2026-09-10

Environment:

- Fjern commit: `2ed0b83` plus the VNC fixture length correction
- Omarchy: `4.0.3-1`
- Hyprland: `0.56.2` (`efb50993780079460b0cbed1363e2166a2de1d9f`)
- Kernel: `7.2.3-arch1-3`
- Session: native Wayland (`wayland-1`)
- Display: BOE eDP-1, 1920×1080 at 60 Hz, scale 1

Validated locally with the packaged release binary and the loopback VNC
fixture:

| Check | Result | Evidence / limitation |
|---|---|---|
| Package contents | Pass | Binary, desktop file, SVG icon, license and README installed under the expected paths |
| Desktop entry syntax | Pass | `desktop-file-validate` reports no errors |
| CLI launch | Pass | Packaged binary reports `fjern 0.2.0` |
| Native Wayland window | Pass | Hyprland reports `xwayland: false` |
| Tiled behavior | Pass | Window mapped tiled and accepted input focus |
| Compositor resize | Pass | Active tiled window changed size without disconnecting |
| Fullscreen transition | Pass | Entered 1920×1080 fullscreen and returned to its tiled size |
| VNC presentation path | Pass | Negotiated Raw, CopyRect, ZRLE and desktop-resize encodings |
| Server resize | Pass | Client refreshed at the fixture's new 80×48 framebuffer size |
| Window close | Partial | Client and fixture exit successfully, but vendored minifb logs attached Wayland-proxy warnings during teardown |
| Launcher installation | Blocked | Package is valid, but system installation requires an interactive administrator password |
| Real WayVNC host | Not run | No WayVNC process or reachable test service was available |
| Real Windows RDP host | Not run | No RDP test service was available |
| Keyboard, pointer and clipboard | Not run | The loopback fixture cannot verify remote input semantics or clipboard integration |

This record supplements, rather than replaces, the earlier real-host evidence
linked from the feature documentation. A release candidate still needs an
installed-package launcher check and current real-host RDP/WayVNC validation.
