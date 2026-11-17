import os
import shutil
import sys

from PIL import Image, ImageDraw, ImageFilter, ImageFont

ROOT = os.path.dirname(os.path.abspath(__file__))
OUT = os.path.join(ROOT, "images")
FONTS = os.path.join(os.environ.get("WINDIR", "C:\\Windows"), "Fonts")
shots = sys.argv[1]

def font(name, size):
    return ImageFont.truetype(os.path.join(FONTS, name), size)

TEXT = {
    "de": {
        "tagline": "Der Archivierer, der mitdenkt.",
        "points": [
            "Bis zu einem Drittel kleiner als 7-Zip",
            "Öffnet ZIP, 7z, RAR und TAR",
            "Passwort, Aufteilen, Reparieren",
            "Direkt im Rechtsklick-Menü",
        ],
        "chart": ["Original", "ZIP", "7z (Ultra)", "Quarz"],
        "unit": "MB",
    },
    "en": {
        "tagline": "The archiver that thinks ahead.",
        "points": [
            "Up to a third smaller than 7-Zip",
            "Opens ZIP, 7z, RAR and TAR",
            "Passwords, splitting, repair",
            "Right in the right-click menu",
        ],
        "chart": ["Original", "ZIP", "7z (Ultra)", "Quarz"],
        "unit": "MB",
    },
}

def rounded(image, radius):
    mask = Image.new("L", image.size, 0)
    ImageDraw.Draw(mask).rounded_rectangle((0, 0, image.size[0] - 1, image.size[1] - 1), radius=radius, fill=255)
    out = Image.new("RGBA", image.size)
    out.paste(image, (0, 0), mask)
    return out

def banner(lang):
    W, H = 1280, 600
    top, bottom = (14, 70, 160), (6, 30, 82)
    img = Image.new("RGB", (1, H))
    for y in range(H):
        t = y / (H - 1)
        img.putpixel((0, y), tuple(round(a + (b - a) * t) for a, b in zip(top, bottom)))
    img = img.resize((W, H)).convert("RGBA")
    d = ImageDraw.Draw(img)

    shot = Image.open(os.path.join(shots, lang, "01-start.png")).convert("RGB")
    shot = shot.resize((640, round(640 * shot.height / shot.width)), Image.LANCZOS)
    shot = rounded(shot, 14)
    x, y = W - shot.width - 40, (H - shot.height) // 2 + 10
    shadow = Image.new("RGBA", (W, H), (0, 0, 0, 0))
    ImageDraw.Draw(shadow).rounded_rectangle((x + 6, y + 14, x + shot.width + 6, y + shot.height + 14), 16, fill=(0, 0, 0, 120))
    img = Image.alpha_composite(img, shadow.filter(ImageFilter.GaussianBlur(18)))
    img.alpha_composite(shot, (x, y))
    d = ImageDraw.Draw(img)

    icon = Image.open(os.path.join(ROOT, "..", "assets", "quarz256.png")).convert("RGBA").resize((88, 88), Image.LANCZOS)
    img.alpha_composite(icon, (64, 92))
    d.text((168, 94), "Quarz", font=font("seguisb.ttf", 66), fill="white")
    t = TEXT[lang]
    d.text((66, 206), t["tagline"], font=font("segoeui.ttf", 28), fill=(205, 226, 255))
    for i, point in enumerate(t["points"]):
        cy = 290 + i * 54
        d.ellipse((68, cy + 7, 88, cy + 27), fill=(76, 194, 255))
        d.line((73, cy + 17, 77, cy + 21, 84, cy + 12), fill=(6, 30, 82), width=3)
        d.text((104, cy), point, font=font("segoeui.ttf", 25), fill="white")
    img.convert("RGB").save(os.path.join(OUT, f"banner-{lang}.png"), optimize=True)

def chart(lang, dark):
    values = [699.5, 641.0, 534.3, 402.5]
    names = TEXT[lang]["chart"]
    ink, secondary = ("#f0efec", "#c3c2b7") if dark else ("#0b0b0b", "#52514e")
    accent = "#3987e5" if dark else "#2a78d6"
    gray = "#5e5d59" if dark else "#c3c2b7"
    axis = "#383835" if dark else "#c3c2b7"
    W, left, right, row, bar = 760, 132, 150, 46, 22
    H = row * len(values) + 16
    scale = (W - left - right) / max(values)
    fmt = (lambda v: f"{v:.1f}".replace(".", ",")) if lang == "de" else (lambda v: f"{v:.1f}")
    parts = [
        f'<svg xmlns="http://www.w3.org/2000/svg" width="{W}" height="{H}" viewBox="0 0 {W} {H}" '
        f'font-family="Segoe UI, system-ui, -apple-system, sans-serif" role="img" '
        f'aria-label="{", ".join(f"{n}: {fmt(v)} MB" for n, v in zip(names, values))}">',
        f'<line x1="{left}" y1="4" x2="{left}" y2="{H - 8}" stroke="{axis}" stroke-width="1"/>',
    ]
    for i, (name, value) in enumerate(zip(names, values)):
        y = 8 + i * row + (row - bar) / 2
        w = value * scale
        color = accent if name == "Quarz" else gray
        weight = 600 if name == "Quarz" else 400
        parts.append(
            f'<path d="M{left},{y} h{w - 4} a4,4 0 0 1 4,4 v{bar - 8} a4,4 0 0 1 -4,4 h{-(w - 4)} z" fill="{color}"/>'
        )
        parts.append(f'<text x="{left - 14}" y="{y + bar / 2 + 5}" text-anchor="end" font-size="15" font-weight="{weight}" fill="{ink}">{name}</text>')
        saved = "" if i == 0 else f'  <tspan fill="{secondary}" font-weight="400">&#8722;{100 - 100 * value / values[0]:.0f} %</tspan>'
        parts.append(
            f'<text x="{left + w + 10}" y="{y + bar / 2 + 5}" font-size="15" font-weight="{weight}" fill="{ink}">'
            f'{fmt(value)} {TEXT[lang]["unit"]}{saved}</text>'
        )
    parts.append("</svg>")
    name = f"chart-{lang}-{'dark' if dark else 'light'}.svg"
    with open(os.path.join(OUT, name), "w", encoding="utf-8") as f:
        f.write("\n".join(parts))

os.makedirs(OUT, exist_ok=True)
for lang in ("de", "en"):
    target = os.path.join(OUT, lang)
    os.makedirs(target, exist_ok=True)
    for shot in os.listdir(os.path.join(shots, lang)):
        if shot.endswith(".png"):
            Image.open(os.path.join(shots, lang, shot)).save(os.path.join(target, shot), optimize=True)
    banner(lang)
    chart(lang, False)
    chart(lang, True)
shutil.copy(os.path.join(ROOT, "..", "assets", "quarz256.png"), os.path.join(OUT, "icon.png"))
print("ok", sorted(os.listdir(OUT)))
