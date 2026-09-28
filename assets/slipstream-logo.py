"""Draws SLIPSTREAM from a typeface as terminal block art, in quarter-cell blocks.

usage: slipstream-logo.py FONT OUT_PREFIX [--cols 121] [--skew 0] [--wght N] [--wdth N]
Writes OUT_PREFIX.txt (one line), OUT_PREFIX-stacked.txt (SLIP over STREAM) and a PNG preview
of each. Needs Pillow.
"""
import argparse
from PIL import Image, ImageDraw, ImageFont

# Which quarters are lit (top left, top right, bottom left, bottom right) -> character
QUAD = {
    (0, 0, 0, 0): " ", (1, 1, 1, 1): "█",
    (1, 1, 0, 0): "▀", (0, 0, 1, 1): "▄", (1, 0, 1, 0): "▌", (0, 1, 0, 1): "▐",
    (1, 0, 0, 0): "▘", (0, 1, 0, 0): "▝", (0, 0, 1, 0): "▖", (0, 0, 0, 1): "▗",
    (1, 0, 0, 1): "▚", (0, 1, 1, 0): "▞",
    (1, 1, 1, 0): "▛", (1, 1, 0, 1): "▜", (1, 0, 1, 1): "▙", (0, 1, 1, 1): "▟",
}

# Supersampling across each quarter cell.
K = 12


def load(args, size):
    font = ImageFont.truetype(args.font, size)
    axes = {}
    if args.wght:
        axes["wght"] = args.wght
    if args.wdth:
        axes["wdth"] = args.wdth
    if axes:
        names = [a["name"] if isinstance(a["name"], str) else a["name"].decode() for a in font.get_variation_axes()]
        vals = []
        for a, n in zip(font.get_variation_axes(), names):
            key = "wght" if n.lower().startswith("weight") else "wdth" if n.lower().startswith("width") else n
            vals.append(axes.get(key, a["default"]))
        font.set_variation_by_axes(vals)
    return font


def ink(args, word):
    """The word in black on white, cropped to its ink, skewed."""
    size = 400
    font = load(args, size)
    width = int(size * len(word) * 1.4)
    img = Image.new("L", (width, size * 2), 0)
    d = ImageDraw.Draw(img)
    x = 40
    for ch in word:
        d.text((x, size // 2), ch, font=font, fill=255)
        x += font.getlength(ch) + args.track * size
    if args.skew:
        s = args.skew
        img = img.transform(img.size, Image.AFFINE, (1, s, -s * img.size[1] / 2, 0, 1, 0), Image.BICUBIC)
    return img.crop(img.getbbox())


def cells(img, cols):
    """Quadrant cells `cols` wide; a subpixel is half a cell across and half down, and a cell is
    twice as tall as it is wide, so subpixels are twice as tall as wide."""
    sw = cols * 2
    aspect = img.width / img.height
    # physical: width = cols*w, height = rows*2w => rows = cols / (2*aspect)
    rows = max(1, round(cols / (2 * aspect)))
    sh = rows * 2
    big = img.resize((sw * K, sh * 2 * K), Image.LANCZOS)
    small = big.resize((sw, sh), Image.BOX)
    px = small.load()
    lines = []
    for r in range(rows):
        line = ""
        for c in range(cols):
            q = tuple(int(px[c * 2 + dx, r * 2 + dy] >= 128) for dy in (0, 1) for dx in (0, 1))
            line += QUAD[q]
        lines.append(line.rstrip())
    return lines


def preview(lines, path, cw=8):
    ch = cw * 2
    w = max(len(l) for l in lines) * cw
    img = Image.new("RGB", (w + 40, len(lines) * ch + 40), (11, 13, 18))
    d = ImageDraw.Draw(img)
    n = max(len(l) for l in lines)
    for r, line in enumerate(lines):
        for c, chr_ in enumerate(line):
            q = next((k for k, v in QUAD.items() if v == chr_), None)
            if not q:
                continue
            t = c / n
            col = tuple(int(a + (b - a) * t) for a, b in zip((255, 201, 120), (66, 211, 255)))
            for i, on in enumerate(q):
                if on:
                    x0 = 20 + c * cw + (i % 2) * cw // 2
                    y0 = 20 + r * ch + (i // 2) * ch // 2
                    d.rectangle([x0, y0, x0 + cw // 2 - 1, y0 + ch // 2 - 1], fill=col)
    img.save(path)


def main():
    p = argparse.ArgumentParser()
    p.add_argument("font")
    p.add_argument("out")
    p.add_argument("--cols", type=int, default=121)
    p.add_argument("--stacked-cols", type=int, default=0)
    p.add_argument("--skew", type=float, default=0)
    p.add_argument("--wght", type=float, default=0)
    p.add_argument("--wdth", type=float, default=0)
    p.add_argument("--track", type=float, default=0.0)
    p.add_argument("--text", default="SLIPSTREAM")
    args = p.parse_args()
    wide = cells(ink(args, args.text), args.cols)
    open(args.out + ".txt", "w").write("\n".join(wide) + "\n")
    preview(wide, args.out + ".png")
    # Stacked: both words at the wide logo's cell height.
    a, b = args.text[:4], args.text[4:]
    rows = len(wide)
    parts = []
    for word in (a, b):
        img = ink(args, word)
        cols = max(1, round(rows * 2 * img.width / img.height))
        parts.append(cells(img, cols))
    stacked = parts[0] + [""] + parts[1]
    open(args.out + "-stacked.txt", "w").write("\n".join(stacked) + "\n")
    preview(stacked, args.out + "-stacked.png")
    print(args.out, "rows", rows, "stacked widths", [max(map(len, x)) for x in parts])


if __name__ == "__main__":
    main()
