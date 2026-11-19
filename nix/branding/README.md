# Iliada branding

The Iliada mark is a left-facing Corinthian helmet (περικεφαλαία) in bronze,
with a red horsehair crest seated on the dome. Every file below is built from
the same hand-authored helmet and crest paths, so they always match.

## Files

| File | What it is |
| --- | --- |
| `iliada.svg` | Main color mark, transparent background, 256×256. Installed as the `iliada` icon by `nix/modules/branding.nix`. |
| `iliada-icon.svg` | App / launcher icon: the mark on a deep navy squircle. |
| `iliada-mono.svg` | Single-color mark using `currentColor`. Legible down to 16 px. |
| `iliada-lockup.svg` | Mark plus "Iliada" wordmark, for light backgrounds. |
| `iliada-lockup-dark.svg` | Same lockup, for dark backgrounds. |
| `apps/iliada-store-gui/res/org.iliada.Store.svg` | The Store icon: a stoa of the agora (pediment, meander, columns, amphorae) on the navy squircle. |
| `apps/iliada-settings/res/org.iliada.Assistant.svg` | Homer (the assistant): his bearded profile with a red fillet on a bronze coin, eye closed (Homer was blind). |
| `apps/iliada-settings/res/org.iliada.Settings.svg` | Settings: a bronze gear with a meander ring and a red boss. |
| `apps/iliada-settings/res/org.iliada.Setup.svg` | The Setup Assistant: a laurel wreath around a red alpha (the beginning). |
| `apps/iliada-installer/res/org.iliada.Installer.svg` | The installer: a Greek ship with a red-striped sail setting out. |
| `logo.txt` | fastfetch ASCII art. `$1` is the crest color, `$2` the helmet color. |
| `fastfetch.jsonc` | fastfetch config that uses `logo.txt`. |
| `png/` | Raster exports: icon at 16 to 1024 px, mark and mono mark at 512 px, lockups at 1600 px wide. |
| `tools/generate.py` | Regenerates all SVGs, PNGs and `logo.txt` (needs `cairosvg` and `pillow`). |

The wordmark is drawn as geometric outlines, so no font is needed.

## Palette

| Role | Hex |
| --- | --- |
| Bronze light | `#F6D38E` |
| Bronze mid | `#DDA34E` |
| Bronze shadow | `#A5671F` |
| Bronze sheen | `#FFF1CC` (35% opacity) |
| Crest light | `#EE5442` |
| Crest mid | `#CF3228` |
| Crest shadow | `#9E1F18` |
| Navy glow | `#22396A` |
| Navy | `#14254A` |
| Navy deep | `#0B1630` |
| Ink (wordmark on light) | `#18213A` |
| Paper (wordmark on dark) | `#F4EFE6` |

## Usage

* Use `iliada.svg` wherever the OS logo appears on its own; use `iliada-icon.svg`
  where a launcher or store expects a filled tile.
* Use `iliada-mono.svg` for tiny sizes, the boot splash, panel / tray icons and
  anywhere only one color is allowed. Set the color with CSS `color` (or the
  `color` attribute on the `<svg>`).
* Keep clear space around the mark of at least the crest's thickness on every side.
* Don't recolor the crest and helmet independently, stretch the mark, or add
  outlines or drop shadows to `iliada.svg`.
* After editing the geometry in `tools/generate.py`, run
  `python3 nix/branding/tools/generate.py` and commit the regenerated files.
