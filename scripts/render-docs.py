"""Build README gallery media from Muisland's offline renders.

Usage:
    Muisland.exe --render-previews preview
    Muisland.exe --render-motion motion
    python scripts/render-docs.py preview docs/images [motion]

Requires Pillow. The app renders every image itself (Direct2D, sample data),
so this script only arranges, compresses and animates them.
"""

import sys
from pathlib import Path
from PIL import Image

source = Path(sys.argv[1])
out = Path(sys.argv[2])
motion = Path(sys.argv[3]) if len(sys.argv) > 3 else None
out.mkdir(parents=True, exist_ok=True)
BACKDROP = (24, 26, 36)


def load(name):
    return Image.open(source / f"{name}.png").convert("RGB")


def row(images, gap=24, pad=24):
    w = sum(i.width for i in images) + gap * (len(images) - 1) + 2 * pad
    h = max(i.height for i in images) + 2 * pad
    board = Image.new("RGB", (w, h), BACKDROP)
    x = pad
    for image in images:
        board.paste(image, (x, pad + (h - 2 * pad - image.height) // 2))
        x += image.width + gap
    return board


def save(image, name, width=None):
    if width and image.width > width:
        image = image.resize(
            (width, round(image.height * width / image.width)), Image.Resampling.LANCZOS
        )
    image.save(out / name, optimize=True)
    print("wrote", out / name, image.size)


save(row([load("accounts"), load("accounts-arabic")]), "accounts.png", 1400)
save(row([load("preferences"), load("usage-settings")]), "preferences.png", 1400)
save(row([load("daylight"), load("sunset"), load("preferences-themes")]), "themes.png", 1600)
save(row([load("circle-hover")], pad=16), "circle-bubble.png", 900)
save(
    row([load(n) for n in ["circle", "high-usage", "circle-logo", "ocean-circle"]] + [load("pill")], gap=8),
    "islands.png",
    1400,
)

if motion:
    frames = sorted(motion.glob("frame-*.png"))[::2]
    images = []
    for path in frames:
        frame = Image.open(path).convert("RGB")
        frame = frame.resize((frame.width // 2, frame.height // 2), Image.Resampling.LANCZOS)
        images.append(frame.quantize(colors=128, method=Image.Quantize.MEDIANCUT, dither=Image.Dither.NONE))
    images[0].save(
        out / "interaction.gif",
        save_all=True,
        append_images=images[1:],
        duration=33,
        loop=0,
        optimize=True,
        disposal=1,
    )
    print("wrote", out / "interaction.gif", len(images), "frames")
