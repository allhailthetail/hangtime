# Hangtime icon pack

Master glyph: headset with boom mic on a blue gradient (`#3B93FF` to
`#1A4FD0`). Dark glyph variant included for light-background placements.

## Where each file goes

- `windows/hangtime.ico`: multi-size ICO (16 to 256). Embedded in the
  exe by `crates/hangtime/build.rs`, and from there used by the MSI.
  `Square44x44Logo.png`, `Square150x150Logo.png` and `StoreLogo-50.png`
  are for MSIX/Store packaging.
- `macos/Hangtime.iconset`: run `iconutil -c icns Hangtime.iconset` on a
  Mac to produce `Hangtime.icns`. Artwork sits inside the 824px safe
  square with the standard macOS squircle margin.
- `macos/tray`: `trayTemplate.png` and `trayTemplate@2x.png`.
  Monochrome, black with alpha, so macOS tints them. Name must keep the
  "Template" suffix for automatic light/dark adaptation.
- `linux/hicolor`: install under `/usr/share/icons/hicolor` (or
  `~/.local/share/icons/hicolor`). Run `gtk-update-icon-cache` on the
  hicolor folder after install. Use the `Icon=hangtime` key in
  `.desktop` files.
- `ios/AppIcon-1024.png`: App Store marketing icon. Full bleed, RGB, no
  transparency. Xcode generates the other sizes from the asset catalog.
- `android/res`: copy the `mipmap-*` folders into `app/src/main/res`.
  The adaptive XML references the background and foreground layers.
  The foreground keeps the glyph inside the 66% safe zone so launcher
  masks (circle, squircle, teardrop) don't clip it.
  `playstore-icon-512.png` is for the Play Console listing.
- `tray/`: same monochrome template as macOS, usable for Windows and
  Linux tray icons too (use a white variant there on dark taskbars).

## Source files

All masters are vector SVG in each platform's `source/` folder and in
`source/`. Edit those and re-run `build.py` to regenerate.
