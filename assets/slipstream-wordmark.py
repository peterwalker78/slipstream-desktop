"""Draws the SLIPSTREAM wordmark as SVG from the same letters as the wallpapers' logo, for pages
that show it outside the desktop.

python3 slipstream-wordmark.py readme/wordmark

writes readme/wordmark-dark.svg (the logo's own amber, mint and cyan, for dark pages) and
readme/wordmark-light.svg (deeper shades of them, for light ones).
"""
import importlib.util
import os
import sys

here = os.path.dirname(os.path.abspath(__file__))
spec = importlib.util.spec_from_file_location("logo", os.path.join(here, "slipstream-logo.py"))
logo = importlib.util.module_from_spec(spec)
spec.loader.exec_module(logo)

# A grid column is half a cell wide and a row half a cell tall, so a row is two columns high.
COLUMN, ROW = 1, 2
SHADES = {
    "dark": ("#ffb547", "#3cf0bf", "#33ccff"),
    "light": ("#c77800", "#0e9e78", "#0a7fb8"),
}


def grid(text):
    parts = [logo.letter(ch) for ch in text]
    rows = logo.H + logo.TAIL
    out = [[] for _ in range(rows)]
    for i, g in enumerate(parts):
        for r in range(rows):
            out[r].extend(g[r])
            if i < len(parts) - 1:
                out[r].extend([0] * logo.GAP)
    return out


def svg(g, shades):
    width, height = len(g[0]) * COLUMN, len(g) * ROW
    runs = []
    for r, row in enumerate(g):
        c = 0
        while c < len(row):
            if row[c]:
                start = c
                while c < len(row) and row[c]:
                    c += 1
                runs.append(f"M{start * COLUMN} {r * ROW}h{(c - start) * COLUMN}v{ROW}h-{(c - start) * COLUMN}z")
            else:
                c += 1
    amber, mint, cyan = shades
    return (
        f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {width} {height}" role="img" '
        f'aria-label="Slipstream">'
        f'<defs><linearGradient id="g" x1="0" x2="1" y1="0" y2="0">'
        f'<stop offset="0" stop-color="{amber}"/><stop offset=".5" stop-color="{mint}"/>'
        f'<stop offset="1" stop-color="{cyan}"/></linearGradient></defs>'
        f'<path fill="url(#g)" d="{"".join(runs)}"/></svg>\n'
    )


if __name__ == "__main__":
    out = sys.argv[1]
    g = grid("SLIPSTREAM")
    for name, shades in SHADES.items():
        with open(f"{out}-{name}.svg", "w") as f:
            f.write(svg(g, shades))
    print(len(g[0]), "x", len(g), "grid")
