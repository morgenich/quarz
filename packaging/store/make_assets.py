import os

from PIL import Image

here = os.path.dirname(os.path.abspath(__file__))
icon = Image.open(os.path.join(here, "..", "..", "assets", "quarz256.png")).convert("RGBA")
out = os.path.join(here, "Assets")
os.makedirs(out, exist_ok=True)


def square(name, size, padding=0.0):
    canvas = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    inner = round(size * (1 - 2 * padding))
    canvas.alpha_composite(icon.resize((inner, inner), Image.LANCZOS), ((size - inner) // 2, (size - inner) // 2))
    canvas.save(os.path.join(out, name))


square("Square44x44Logo.png", 44)
square("Square44x44Logo.targetsize-44_altform-unplated.png", 44)
square("Square150x150Logo.png", 150, 0.16)
square("StoreLogo.png", 50)
wide = Image.new("RGBA", (310, 150), (0, 0, 0, 0))
wide.alpha_composite(icon.resize((100, 100), Image.LANCZOS), (105, 25))
wide.save(os.path.join(out, "Wide310x150Logo.png"))
print(sorted(os.listdir(out)))
