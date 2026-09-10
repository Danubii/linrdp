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
