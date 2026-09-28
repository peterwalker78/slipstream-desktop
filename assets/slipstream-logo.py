"""Draws the wallpapers' SLIPSTREAM logo: heavy slab letters cut by thin slits, designed straight
on the grid the wallpapers draw in.

The grid is quarter cells: a column is half a cell wide, a row half a cell tall, so two columns and
one row make a square, and a 45° line steps two columns a row. Letters are 11 rows tall: a bar of
3, a slit of 1, a bar of 3, a slit of 1, a bar of 3. The R's leg runs
2 rows below the baseline.

python3 slipstream-logo.py slipstream-logo

writes slipstream-logo.txt and slipstream-logo-stacked.txt (SLIP over STREAM).
"""
import sys

# Which quarters of a cell are lit (top left, top right, bottom left, bottom right) -> character.
QUAD = {
    (0, 0, 0, 0): " ", (1, 1, 1, 1): "█",
    (1, 1, 0, 0): "▀", (0, 0, 1, 1): "▄", (1, 0, 1, 0): "▌", (0, 1, 0, 1): "▐",
    (1, 0, 0, 0): "▘", (0, 1, 0, 0): "▝", (0, 0, 1, 0): "▖", (0, 0, 0, 1): "▗",
    (1, 0, 0, 1): "▚", (0, 1, 1, 0): "▞",
    (1, 1, 1, 0): "▛", (1, 1, 0, 1): "▜", (1, 0, 1, 1): "▙", (0, 1, 1, 1): "▟",
}

H, TAIL = 11, 2
STEM = 6          # a stem, in columns (three cells)
CHAMFER = 2       # rows cut from each chamfered corner (four columns)
GAP = 4           # between letters


def blank(w):
    return [[0] * w for _ in range(H + TAIL)]


def fill(g, x0, y0, x1, y1, v=1):
    for y in range(max(0, y0), min(len(g), y1)):
        for x in range(max(0, x0), min(len(g[0]), x1)):
            g[y][x] = v


def chamfer_top_left(g):
    for r in range(CHAMFER):
        for x in range(2 * (CHAMFER - r)):
            g[r][x] = 0


def chamfer_bottom_left(g, bottom):
    for i in range(CHAMFER):
        r = bottom - 1 - i
        for x in range(2 * (CHAMFER - i)):
            g[r][x] = 0


def letter(ch):
    if ch == "S":
        w = 26
        g = blank(w)
        fill(g, 0, 0, w, H)
        fill(g, 0, 3, w, 4, 0)             # the upper slit, right across: the top bar floats
        fill(g, 0, 7, w - STEM, 8, 0)      # the lower slit, open to the left
        chamfer_top_left(g)
        chamfer_bottom_left(g, H)
    elif ch == "L":
        w = 22
        g = blank(w)
        fill(g, 0, 0, STEM, H)
        fill(g, 0, 8, w, H)
        chamfer_top_left(g)
    elif ch == "I":
        w = STEM
        g = blank(w)
        fill(g, 0, 0, w, H)
        chamfer_top_left(g)
    elif ch == "P":
        w = 26
        g = blank(w)
        fill(g, 0, 0, w, 7)
        fill(g, 0, 7, STEM, H)
        fill(g, 0, 3, w - STEM, 4, 0)      # the bowl, a slit through the stem
        fill(g, STEM, 7, w, 8, 0)
        chamfer_top_left(g)
        # The bowl's lower corner cut, as the tails are.
        for i in range(CHAMFER):
            for x in range(2 * (CHAMFER - i)):
                g[6 - i][w - 1 - x] = 0
    elif ch == "T":
        w = 26
        g = blank(w)
        fill(g, 0, 0, w, 3)
        m = (w - STEM) // 2
        fill(g, m, 3, m + STEM, H)
        chamfer_top_left(g)
    elif ch == "R":
        w = 28
        g = blank(w)
        fill(g, 0, 0, 26, 7)
        fill(g, 0, 7, STEM, H)
        fill(g, 0, 3, 26 - STEM, 4, 0)
        chamfer_top_left(g)
        # The leg: from under the bowl, 45° down and right, past the baseline.
        for i, y in enumerate(range(7, H + TAIL)):
            x0 = STEM + 4 + 2 * i
            fill(g, x0, y, x0 + STEM + 1, y + 1)
    elif ch == "E":
        w = 24
        g = blank(w)
        fill(g, 0, 0, w, H)
        fill(g, 0, 3, w, 4, 0)             # right across
        fill(g, STEM, 7, w, 8, 0)
        fill(g, w - 4, 4, w, 7, 0)         # the middle bar a little shorter
        chamfer_top_left(g)
    elif ch == "A":
        w = 26
        g = blank(w)
        fill(g, 0, 0, w, 7)
        fill(g, 0, 3, w - STEM, 4, 0)      # through the left leg, joined on the right
        fill(g, 0, 7, STEM, H)
        fill(g, w - STEM, 7, w, H)
        chamfer_top_left(g)
    elif ch == "M":
        w = 32
        g = blank(w)
        body = 32
        fill(g, 0, 0, body, 3)
        m = (body - STEM) // 2
        for x0 in (0, m, body - STEM):
            fill(g, x0, 3, x0 + STEM, H)
        # Counters start with a slit's worth of corner, as the others' slits do.
        fill(g, STEM, 3, STEM + 1, 4)
        fill(g, m - 1, 3, m, 4)
        fill(g, m + STEM, 3, m + STEM + 1, 4)
        fill(g, body - STEM - 1, 3, body - STEM, 4)
        chamfer_top_left(g)
    else:
        raise ValueError(ch)
    return g


def word(text):
    parts = [letter(ch) for ch in text]
    rows = H + TAIL
    out = [[] for _ in range(rows)]
    for i, g in enumerate(parts):
        for r in range(rows):
            out[r].extend(g[r])
            if i < len(parts) - 1:
                out[r].extend([0] * GAP)
    width = len(out[0]) + (len(out[0]) % 2)
    for r in out:
        r.extend([0] * (width - len(r)))
    if len(out) % 2:
        out.append([0] * width)
    lines = []
    for r in range(0, len(out), 2):
        line = ""
        for c in range(0, width, 2):
            q = (out[r][c], out[r][c + 1], out[r + 1][c], out[r + 1][c + 1])
            line += QUAD[q]
        lines.append(line.rstrip())
    return lines


if __name__ == "__main__":
    out = sys.argv[1]
    wide = word("SLIPSTREAM")
    open(out + ".txt", "w").write("\n".join(wide) + "\n")
    stacked = word("SLIP") + [""] + word("STREAM")
    open(out + "-stacked.txt", "w").write("\n".join(stacked) + "\n")
    print(max(map(len, wide)), "x", len(wide))
    print("\n".join(wide))
