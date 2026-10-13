import os, io, shutil, cairosvg
from PIL import Image

OUT = "/mnt/user-data/outputs/hangtime-icons"
BLUE_TOP, BLUE_BOT = "#3B93FF", "#1A4FD0"
SLATE = "#0F1B33"

def glyph(fg, scale=1.0):
    # Headset (headband, ear cups) with boom mic. Native bounds ~x186-838, y188-865.
    return f'''<g transform="translate(512,512) scale({scale}) translate(-512,-526)">
  <path d="M236 560 C236 340 376 220 512 220 C648 220 788 340 788 560" fill="none" stroke="{fg}" stroke-width="64" stroke-linecap="round"/>
  <path d="M300 690 C300 800 370 830 470 830" fill="none" stroke="{fg}" stroke-width="48" stroke-linecap="round"/>
  <rect x="186" y="520" width="130" height="190" rx="52" fill="{fg}"/>
  <rect x="708" y="520" width="130" height="190" rx="52" fill="{fg}"/>
  <rect x="410" y="795" width="120" height="70" rx="35" fill="{fg}"/>
</g>'''

def grad(id_, c1, c2):
    return f'<linearGradient id="{id_}" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="{c1}"/><stop offset="1" stop-color="{c2}"/></linearGradient>'

def svg(body, defs=""):
    return f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1024 1024"><defs>{defs}</defs>{body}</svg>'

def render(svg_text, path, size):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    cairosvg.svg2png(bytestring=svg_text.encode(), write_to=path, output_width=size, output_height=size)

def save_svg(svg_text, path):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w") as f:
        f.write(svg_text)

# ---------- Master ----------
master_bg = svg(
    f'<rect x="0" y="0" width="1024" height="1024" rx="0" fill="url(#g)"/>' + glyph("#FFFFFF", 0.72),
    grad("g", BLUE_TOP, BLUE_BOT))
save_svg(master_bg, f"{OUT}/source/hangtime-master.svg")
save_svg(svg(glyph("#FFFFFF", 1.0)), f"{OUT}/source/hangtime-glyph-white.svg")
save_svg(svg(glyph(SLATE, 1.0)), f"{OUT}/source/hangtime-glyph-dark.svg")

# ---------- macOS: 824px squircle inside 1024 canvas (Apple template) ----------
mac = svg(
    '<rect x="100" y="100" width="824" height="824" rx="185" fill="url(#g)"/>' + glyph("#FFFFFF", 0.66),
    grad("g", BLUE_TOP, BLUE_BOT))
save_svg(mac, f"{OUT}/macos/source/hangtime-macos.svg")
iconset = f"{OUT}/macos/Hangtime.iconset"
for base in [16, 32, 128, 256, 512]:
    render(mac, f"{iconset}/icon_{base}x{base}.png", base)
    render(mac, f"{iconset}/icon_{base}x{base}@2x.png", base * 2)
render(mac, f"{OUT}/macos/Hangtime-1024.png", 1024)

# macOS menu bar template (monochrome; macOS tints it)
tray_svg = svg(glyph("#000000", 1.0))
for s in [16, 32]:
    render(tray_svg, f"{OUT}/tray/trayTemplate{'' if s==16 else '@2x'}.png", s)

# ---------- Windows: full-bleed rounded tile ----------
win = svg(
    '<rect x="0" y="0" width="1024" height="1024" rx="200" fill="url(#g)"/>' + glyph("#FFFFFF", 0.72),
    grad("g", BLUE_TOP, BLUE_BOT))
save_svg(win, f"{OUT}/windows/source/hangtime-windows.svg")
win_sizes = [16, 24, 32, 48, 64, 128, 256]
frames = []
for s in win_sizes:
    p = f"{OUT}/windows/png/hangtime-{s}.png"
    render(win, p, s)
    frames.append(Image.open(p).convert("RGBA"))
render(win, f"{OUT}/windows/png/hangtime-256.png", 256)
frames[-1].save(f"{OUT}/windows/hangtime.ico", format="ICO", sizes=[(s, s) for s in win_sizes])
# Store-style tiles
render(win, f"{OUT}/windows/png/StoreLogo-50.png", 50)
render(win, f"{OUT}/windows/png/Square150x150Logo.png", 150)
render(win, f"{OUT}/windows/png/Square44x44Logo.png", 44)

# ---------- Linux: freedesktop hicolor theme ----------
lin = svg(
    '<rect x="0" y="0" width="1024" height="1024" rx="230" fill="url(#g)"/>' + glyph("#FFFFFF", 0.72),
    grad("g", BLUE_TOP, BLUE_BOT))
save_svg(lin, f"{OUT}/linux/hicolor/scalable/apps/hangtime.svg")
for s in [16, 22, 24, 32, 48, 64, 128, 256, 512]:
    render(lin, f"{OUT}/linux/hicolor/{s}x{s}/apps/hangtime.png", s)

# ---------- iOS: 1024 full-bleed, no transparency, no rounded corners (OS applies mask) ----------
ios = svg(
    '<rect x="0" y="0" width="1024" height="1024" fill="url(#g)"/>' + glyph("#FFFFFF", 0.7),
    grad("g", BLUE_TOP, BLUE_BOT))
save_svg(ios, f"{OUT}/ios/source/hangtime-ios.svg")
render(ios, f"{OUT}/ios/AppIcon-1024.png", 1024)
# Flatten to RGB to guarantee no alpha channel for App Store
im = Image.open(f"{OUT}/ios/AppIcon-1024.png").convert("RGB")
im.save(f"{OUT}/ios/AppIcon-1024.png")

# ---------- Android ----------
and_bg = svg('<rect width="1024" height="1024" fill="' + BLUE_BOT + '"/>')
and_fg = svg(glyph("#FFFFFF", 0.78))  # transparent background, inside 66% safe zone
save_svg(and_bg, f"{OUT}/android/source/ic_launcher_background.svg")
save_svg(and_fg, f"{OUT}/android/source/ic_launcher_foreground.svg")
and_full = svg('<rect width="1024" height="1024" fill="' + BLUE_BOT + '"/>' + glyph("#FFFFFF", 0.62))

# Adaptive layers (108dp base)
adaptive = {"mdpi": 108, "hdpi": 162, "xhdpi": 216, "xxhdpi": 324, "xxxhdpi": 432}
legacy = {"mdpi": 48, "hdpi": 72, "xhdpi": 96, "xxhdpi": 144, "xxxhdpi": 192}
for d, s in adaptive.items():
    render(and_bg, f"{OUT}/android/res/mipmap-{d}/ic_launcher_background.png", s)
    render(and_fg, f"{OUT}/android/res/mipmap-{d}/ic_launcher_foreground.png", s)
for d, s in legacy.items():
    render(and_full, f"{OUT}/android/res/mipmap-{d}/ic_launcher.png", s)
    render(and_full, f"{OUT}/android/res/mipmap-{d}/ic_launcher_round.png", s)
render(and_full, f"{OUT}/android/playstore-icon-512.png", 512)

# Adaptive icon XML
xml_dir = f"{OUT}/android/res/mipmap-anydpi-v26"
os.makedirs(xml_dir, exist_ok=True)
for name in ["ic_launcher.xml", "ic_launcher_round.xml"]:
    with open(f"{xml_dir}/{name}", "w") as f:
        f.write('''<?xml version="1.0" encoding="utf-8"?>
<adaptive-icon xmlns:android="http://schemas.android.com/apk/res/android">
    <background android:drawable="@mipmap/ic_launcher_background"/>
    <foreground android:drawable="@mipmap/ic_launcher_foreground"/>
</adaptive-icon>
''')

# ---------- Cleanup and manifest ----------
readme = """# Hangtime icon pack

Master glyph: headset with boom mic on a blue gradient (#3B93FF to #1A4FD0).
Dark glyph variant included for light-background placements.

## Where each file goes

- windows/hangtime.ico: multi-size ICO (16 to 256). Set as the app icon in your build config.
  Square44x44Logo.png, Square150x150Logo.png and StoreLogo-50.png are for MSIX/Store packaging.
- macos/Hangtime.iconset: run  iconutil -c icns Hangtime.iconset  on a Mac to produce Hangtime.icns.
  Artwork sits inside the 824px safe square with the standard macOS squircle margin.
- macos/tray: trayTemplate.png and trayTemplate@2x.png. Monochrome, black with alpha, so macOS tints them.
  Name must keep the "Template" suffix for automatic light/dark adaptation.
- linux/hicolor: install under /usr/share/icons/hicolor (or ~/.local/share/icons/hicolor).
  Run gtk-update-icon-cache on the hicolor folder after install. Use the Icon=hangtime key in .desktop files.
- ios/AppIcon-1024.png: App Store marketing icon. Full bleed, RGB, no transparency. Xcode generates the other sizes from the asset catalog.
- android/res: copy mipmap-* folders into app/src/main/res. The adaptive XML references the background and foreground layers.
  Foreground keeps the glyph inside the 66% safe zone so launcher masks (circle, squircle, teardrop) don't clip it.
  playstore-icon-512.png is for the Play Console listing.
- tray/: same monochrome template as macOS, usable for Windows and Linux tray icons too (use a white variant there on dark taskbars).

## Source files
All masters are vector SVG in each platform's source/ folder and in source/. Edit those and re-run build.py to regenerate.
"""
with open(f"{OUT}/README.md", "w") as f:
    f.write(readme)

shutil.copy("/home/claude/hangtime/build.py", f"{OUT}/source/build.py")
