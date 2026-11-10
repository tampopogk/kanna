import re
import sys
from pathlib import Path
from PIL import Image

mock_png, shots, out, prefix = sys.argv[1], Path(sys.argv[2]), Path(sys.argv[3]), sys.argv[4]
keys = re.findall(r'key: "([a-z-]+)"', Path(__file__).parent.parent.joinpath("KannaMobile/Scenes.swift").read_text())
mock = Image.open(mock_png).convert("RGB")
w = 250
h = w * 868 // 414
per, across = 10, 5
for page in range(0, len(keys), per):
    part = keys[page:page + per]
    rows = (len(part) + across - 1) // across
    sheet = Image.new("RGB", (across * (2 * w + 26), rows * (h + 12)), (150, 150, 150))
    for n, key in enumerate(part):
        i = page + n
        tile = mock.crop((i % 8 * 414, i // 8 * 868, i % 8 * 414 + 414, i // 8 * 868 + 868)).resize((w, h), Image.LANCZOS)
        app = Image.open(shots / f"{key}.png").convert("RGB")
        app = app.resize((w, w * app.height // app.width), Image.LANCZOS)
        x, y = (n % across) * (2 * w + 26), (n // across) * (h + 12)
        sheet.paste(tile, (x, y))
        sheet.paste(app, (x + w + 4, y))
    sheet.save(out / f"{prefix}-{page // per + 1}.png")
