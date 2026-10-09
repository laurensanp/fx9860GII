"""Draws the calculator skin for the native window (modelled on the real
fx-9860GII "USB POWER GRAPHIC 2") and writes it, plus the key/button
positions, into fxemu/res/skin/. Needs Pillow and the DejaVu fonts.

    python3 make_skin.py <output dir> <font dir>
"""
import math
import os
import sys

from PIL import Image, ImageDraw, ImageFont

OUT, FONTS = sys.argv[1], sys.argv[2]
os.makedirs(OUT, exist_ok=True)

W, H = 460, 1022
LCD_X, LCD_Y, LCD_SCALE = 38, 92, 3            # LCD is 128x64, drawn at 3x
STATUS_Y, STATUS_H = 990, 20

# --- colours (sampled by eye from the real calculator) ---
BG = (17, 22, 28)
SILVER = (214, 215, 212)
SILVER_EDGE = (176, 178, 176)
PANEL = (40, 41, 43)
BEZEL = (24, 24, 25)
LCD_BG = (183, 191, 168)
TEXT_DARK = (58, 61, 66)
ORANGE = (232, 160, 40)
RED = (210, 52, 52)
WHITE = (245, 245, 243)

KEY_STYLES = {
    # name: (face, text colour)
    "fkey": ((232, 232, 229), (40, 40, 40)),
    "shift": ((240, 172, 40), (255, 255, 255)),
    "alpha": ((222, 58, 58), (255, 255, 255)),
    "dark": ((30, 30, 31), (245, 245, 245)),
    "menu": ((158, 161, 164), (255, 255, 255)),
    "light": ((240, 238, 231), (40, 40, 40)),
    "grey": ((196, 199, 203), (40, 40, 40)),
    "exe": ((132, 160, 236), (24, 40, 110)),
}


def font(name, size):
    return ImageFont.truetype(os.path.join(FONTS, name), size)


F_SMALL = font("DejaVuSans.ttf", 10)
F_SMALL_B = font("DejaVuSans-Bold.ttf", 10)
F_KEY = font("DejaVuSans-Bold.ttf", 13)
F_KEY_BIG = font("DejaVuSans.ttf", 20)
F_FKEY = font("DejaVuSans-Bold.ttf", 14)
F_TOOL = font("DejaVuSans.ttf", 11)
F_STATUS = font("DejaVuSans.ttf", 11)

img = Image.new("RGB", (W, H), BG)
d = ImageDraw.Draw(img)


def centered(draw, box, text, f, fill):
    x0, y0, x1, y1 = box
    l, t, r, b = draw.textbbox((0, 0), text, font=f)
    draw.text(((x0 + x1) / 2 - (r - l) / 2 - l, (y0 + y1) / 2 - (b - t) / 2 - t), text, font=f, fill=fill)


def shade(c, k):
    return tuple(max(0, min(255, int(v * k))) for v in c)


# --- body ---
d.rounded_rectangle([4, 4, W - 5, H - 5], radius=34, fill=SILVER_EDGE)
d.rounded_rectangle([7, 6, W - 8, H - 9], radius=32, fill=SILVER)

# Brand: CASIO (bold, spaced) and fx-9860GII (slanted serif)
x = 30
for ch in "CASIO":
    d.text((x, 18), ch, font=font("DejaVuSans-Bold.ttf", 30), fill=TEXT_DARK)
    x += d.textlength(ch, font=font("DejaVuSans-Bold.ttf", 30)) + 1
model = Image.new("RGBA", (220, 40), (0, 0, 0, 0))
ImageDraw.Draw(model).text((14, 4), "fx-9860GII", font=font("DejaVuSerif.ttf", 24), fill=TEXT_DARK + (255,))
model = model.transform(model.size, Image.AFFINE, (1, 0.25, -6, 0, 1, 0), resample=Image.BICUBIC)
img.paste(model, (W - 220 - 16, 18), model)

# --- dark upper panel with the screen ---
d.rounded_rectangle([16, 62, W - 17, 528], radius=30, fill=PANEL)
d.rounded_rectangle([26, 72, W - 27, 300], radius=22, fill=BEZEL)
d.rectangle([LCD_X - 4, LCD_Y - 4, LCD_X + 128 * LCD_SCALE + 3, LCD_Y + 64 * LCD_SCALE + 3], fill=(12, 12, 12))
d.rectangle([LCD_X, LCD_Y, LCD_X + 128 * LCD_SCALE - 1, LCD_Y + 64 * LCD_SCALE - 1], fill=LCD_BG)

# "USB POWER GRAPHIC 2"
usb_f = font("DejaVuSans-Bold.ttf", 15)
rest_f = font("DejaVuSans.ttf", 12)
two_f = font("DejaVuSans-Bold.ttf", 15)
usb = Image.new("RGBA", (50, 24), (0, 0, 0, 0))
ImageDraw.Draw(usb).text((4, 2), "USB", font=usb_f, fill=WHITE + (255,))
usb = usb.transform(usb.size, Image.AFFINE, (1, 0.22, -4, 0, 1, 0), resample=Image.BICUBIC)
total = 42 + d.textlength(" POWER GRAPHIC ", font=rest_f) + d.textlength("2", font=two_f)
x0 = (W - total) / 2
img.paste(usb, (int(x0), 305), usb)
d.text((x0 + 40, 309), " POWER GRAPHIC ", font=rest_f, fill=WHITE)
d.text((x0 + 40 + d.textlength(" POWER GRAPHIC ", font=rest_f), 305), "2", font=two_f, fill=WHITE)


def key(box, style, label, f=None, text=None):
    """Draw a key with a little shadow and a lighter top edge."""
    face, fg = KEY_STYLES[style]
    x0, y0, x1, y1 = box
    r = min(10, (y1 - y0) // 2)
    d.rounded_rectangle([x0, y0 + 3, x1, y1 + 3], radius=r, fill=shade(face, 0.55) if style != "dark" else (8, 8, 8))
    d.rounded_rectangle([x0, y0, x1, y1], radius=r, fill=face)
    d.rounded_rectangle([x0 + 2, y0 + 1, x1 - 2, y0 + (y1 - y0) // 2], radius=r - 2, fill=shade(face, 1.06))
    centered(d, box, label, f or F_KEY, text or fg)


keys = []   # (code, name, x0, y0, x1, y1)


def add(code, name, box):
    keys.append((code, name) + tuple(int(v) for v in box))


# --- F-keys with their SHIFT functions ---
flabels = ["Trace", "Zoom", "V-Window", "Sketch", "G-Solv", "G↔T"]
n, left, right, gap = 6, 30, W - 30, 12
kw = (right - left - gap * (n - 1)) / n
for i in range(n):
    x0 = left + i * (kw + gap)
    box = (x0, 352, x0 + kw, 384)
    centered(d, (x0 - 8, 334, x0 + kw + 8, 348), flabels[i], F_SMALL_B, ORANGE)
    key(box, "fkey", f"F{i + 1}", F_FKEY)
    add(0x91 + i, f"F{i + 1}", box)

# --- SHIFT/OPTN/VARS/MENU and ALPHA/x²/^/EXIT, with labels above ---
col_w, col_gap, x_start = 62, 10, 30
rows = [
    (424, [(0x81, "SHIFT", "SHIFT", "shift", ""), (0x82, "OPTN", "OPTN", "dark", "☀ LIGHT"),
           (0x83, "VARS", "VARS", "dark", "PRGM"), (0x84, "MENU", "MENU", "menu", "SET UP")]),
    (484, [(0x71, "ALPHA", "ALPHA", "alpha", "A-LOCK"), (0x72, "SQUARE", "x²", "dark", "√      r"),
           (0x73, "POWER", "^", "dark", "ˣ√      θ"), (0x74, "EXIT", "EXIT", "dark", "QUIT")]),
]
for y, row in rows:
    for i, (code, name, label, style, top) in enumerate(row):
        x0 = x_start + i * (col_w + col_gap)
        box = (x0, y, x0 + col_w, y + 30)
        if top:
            centered(d, (x0 - 6, y - 17, x0 + col_w + 6, y - 3), top, F_SMALL_B, ORANGE)
        key(box, style, label, font("DejaVuSans-Bold.ttf", 12 if len(label) > 3 else 15))
        add(code, name, box)

# --- the round REPLAY pad (arrow keys) ---
cx, cy, R = 370, 455, 60
d.ellipse([cx - R - 3, cy - R - 1, cx + R + 3, cy + R + 5], fill=(10, 10, 10))
d.ellipse([cx - R, cy - R, cx + R, cy + R], fill=(205, 208, 211))
d.ellipse([cx - R + 6, cy - R + 4, cx + R - 6, cy + R - 10], fill=(216, 219, 222))
centered(d, (cx - 40, cy - 10, cx + 40, cy + 10), "REPLAY", font("DejaVuSans.ttf", 11), (120, 124, 130))
for ang, (code, name) in zip((90, 270, 180, 0), [(0x86, "UP"), (0x75, "DOWN"), (0x85, "LEFT"), (0x76, "RIGHT")]):
    a = math.radians(ang)
    tx, ty = cx + math.cos(a) * (R - 13), cy - math.sin(a) * (R - 13)
    pts = []
    for da in (0, 140, 220):
        b = a + math.radians(da)
        pts.append((tx + math.cos(b) * 6, ty - math.sin(b) * 6))
    d.polygon(pts, fill=(130, 134, 140))
    # clickable zone: the outer part of the pad in that direction
    zx, zy = cx + math.cos(a) * (R - 22), cy - math.sin(a) * (R - 22)
    add(code, name, (zx - 22, zy - 22, zx + 22, zy + 22))

# --- lower keyboard: labels above each key (SHIFT orange left, ALPHA red right) ---
LOWER = [
    [(0x61, "XOT", "X,θ,T", "dark", "∠", "A"), (0x62, "LOG", "log", "dark", "10ˣ", "B"),
     (0x63, "LN", "ln", "dark", "eˣ", "C"), (0x64, "SIN", "sin", "dark", "sin⁻¹", "D"),
     (0x65, "COS", "cos", "dark", "cos⁻¹", "E"), (0x66, "TAN", "tan", "dark", "tan⁻¹", "F")],
    [(0x51, "FRAC", "a b/c", "dark", "", "G"), (0x52, "FD", "F↔D", "dark", "", "H"),
     (0x53, "LEFTP", "(", "dark", "∛", "I"), (0x54, "RIGHTP", ")", "dark", "x⁻¹", "J"),
     (0x55, "COMMA", ",", "dark", "", "K"), (0x56, "ARROW", "→", "dark", "", "L")],
    [(0x41, "7", "7", "light", "CAPTURE", "M"), (0x42, "8", "8", "light", "CLIP", "N"),
     (0x43, "9", "9", "light", "PASTE", "O"), (0x44, "DEL", "DEL", "grey", "INS", "UNDO"),
     (0x07, "ACON", "AC/ON", "grey", "", "OFF")],
    [(0x31, "4", "4", "light", "CATALOG", "P"), (0x32, "5", "5", "light", "", "Q"),
     (0x33, "6", "6", "light", "", "R"), (0x34, "MUL", "×", "light", "{", "S"),
     (0x35, "DIV", "÷", "light", "}", "T")],
    [(0x21, "1", "1", "light", "List", "U"), (0x22, "2", "2", "light", "Mat", "V"),
     (0x23, "3", "3", "light", "", "W"), (0x24, "ADD", "+", "light", "[", "X"),
     (0x25, "SUB", "−", "light", "]", "Y")],
    [(0x11, "0", "0", "light", "i", "Z"), (0x12, "DOT", "·", "light", "=", "SPACE"),
     (0x13, "EXP", "EXP", "light", "π", "\""), (0x14, "NEG", "(−)", "light", "Ans", ""),
     (0x15, "EXE", "EXE", "exe", "↵", "")],
]
y = 556
for row in LOWER:
    n = len(row)
    gap = 12 if n == 6 else 14
    left, right = 26, W - 26
    kw = (right - left - gap * (n - 1)) / n
    kh = 34 if n == 6 else 42
    for i, (code, name, label, style, sh, al) in enumerate(row):
        x0 = left + i * (kw + gap)
        box = (x0, y + 16, x0 + kw, y + 16 + kh)
        if sh:
            d.text((x0 + 1, y), sh, font=F_SMALL_B, fill=(214, 136, 24))
        if al:
            l, t, r, b = d.textbbox((0, 0), al, font=F_SMALL_B)
            d.text((x0 + kw - (r - l) - 1, y), al, font=F_SMALL_B, fill=RED)
        big = style in ("light", "exe") and len(label) <= 3 and label not in ("EXP", "(−)", "EXE", "DEL")
        f = F_KEY_BIG if big else font("DejaVuSans-Bold.ttf", 12 if len(label) > 3 else 15)
        key(box, style, label, f)
        add(code, name, box)
    y += 16 + kh + 12

# --- toolbar (emulator functions) ---
tools = []
labels = [("turbo", "Turbo"), ("shot", "Screenshot"), ("restart", "Restart")]
widths = [d.textlength(t, font=F_TOOL) + 22 for _, t in labels]
x = (W - (sum(widths) + 8 * (len(labels) - 1))) / 2
TOOL_Y0, TOOL_Y1 = 962, 984


def draw_tool(draw, box, label, on):
    bg, fg = ((240, 172, 40), (255, 255, 255)) if on else ((190, 192, 194), (60, 62, 66))
    draw.rounded_rectangle(box, radius=8, fill=bg)
    centered(draw, box, label, F_TOOL, fg)


for (name, label), w in zip(labels, widths):
    box = (int(x), TOOL_Y0, int(x + w), TOOL_Y1)
    draw_tool(d, box, label, False)
    tools.append((name, box))
    x += w + 8

img.save(os.path.join(OUT, "skin.png"))
open(os.path.join(OUT, "skin.rgb"), "wb").write(img.tobytes())


# --- sprites ---
def sprite(name, im):
    open(os.path.join(OUT, name + ".rgb"), "wb").write(im.tobytes())


tb = [b for n, b in tools if n == "turbo"][0]
ton = img.crop(tb)
draw_tool(ImageDraw.Draw(ton), (0, 0, ton.width - 1, ton.height - 1), "Turbo", True)
sprite("turbo_on", ton)

MESSAGES = [
    ("off", "Switched off – press AC/ON (or Home) to switch on", (190, 110, 20)),
    ("shot", "Screenshot saved to your Pictures folder", (40, 120, 50)),
    ("turbo", "Turbo: running as fast as possible", (190, 110, 20)),
    ("help", "Enter = EXE   Backspace = DEL   Esc = EXIT   M = MENU   Home = AC/ON", (110, 113, 118)),
]
for name, text, col in MESSAGES:
    m = img.crop((20, STATUS_Y, W - 20, STATUS_Y + STATUS_H))
    md = ImageDraw.Draw(m)
    md.rectangle([0, 0, m.width, m.height], fill=SILVER)
    f = F_STATUS
    while md.textbbox((0, 0), text, font=f)[2] > m.width and f.size > 8:
        f = font("DejaVuSans.ttf", f.size - 1)
    centered(md, (0, 0, m.width, m.height), text, f, col)
    sprite("msg_" + name, m)

# --- layout for the Rust code ---
with open(os.path.join(OUT, "layout.rs"), "w", encoding="utf-8") as o:
    o.write("// Generated by res/make_skin.py -- do not edit.\n")
    o.write(f"pub const W: usize = {W};\npub const H: usize = {H};\n")
    o.write(f"pub const LCD_X: usize = {LCD_X};\npub const LCD_Y: usize = {LCD_Y};\npub const LCD_SCALE: usize = {LCD_SCALE};\n")
    o.write(f"pub const LCD_ON: u32 = 0x2A2E28;\npub const LCD_OFF: u32 = 0x{LCD_BG[0]:02X}{LCD_BG[1]:02X}{LCD_BG[2]:02X};\n")
    o.write(f"pub const STATUS_X: usize = 20;\npub const STATUS_Y: usize = {STATUS_Y};\npub const STATUS_W: usize = {W - 40};\npub const STATUS_H: usize = {STATUS_H};\n")
    o.write("/// (key code, name, x0, y0, x1, y1)\npub const KEYS: &[(u8, &str, usize, usize, usize, usize)] = &[\n")
    for code, name, x0, y0, x1, y1 in keys:
        o.write(f"    (0x{code:02X}, \"{name}\", {x0}, {y0}, {x1}, {y1}),\n")
    o.write("];\n/// (name, x0, y0, x1, y1)\npub const TOOLS: &[(&str, usize, usize, usize, usize)] = &[\n")
    for name, (x0, y0, x1, y1) in tools:
        o.write(f"    (\"{name}\", {x0}, {y0}, {x1}, {y1}),\n")
    o.write("];\n")
print("skin written:", W, "x", H, len(keys), "keys")
