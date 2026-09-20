# Nexus Agent Icon

Two white terminals and a blue connector form an N, reflecting the shared
workspace that brings local coding agents together.

| File | Use |
| --- | --- |
| `nexus-icon.svg` | Editable master, with a 512 x 512 viewBox |
| `nexus-icon.png` | 1024 x 1024 RGBA export with transparent outer margins |
| `nexus-icon.icns` | macOS icon, with standard and Retina representations up to 1024 px |
| `nexus-icon.ico` | Windows icon, with 16, 24, 32, 48, 64, 128, and 256 px representations |
| `nexus-status-item.svg` | macOS menu bar master: the mark alone, without the tile |
| `nexus-status-item.png` | 36 x 36 RGBA template export for the macOS menu bar status item |

The menu bar export drops the graphite tile and keeps only the N, so AppKit can
use its alpha channel as a template image and tint it for light and dark menu
bars. It is exported at 36 px for an 18 pt status item.

The palette follows the desktop theme: graphite `#151515`, white `#F5F5F5`,
and the existing highlight blue `#477AF5`. The tile edge uses `#303030`.

Keep the square aspect ratio and transparent margins. Render from the SVG
master when exporting other sizes; the mark has no font or external asset
dependencies.
