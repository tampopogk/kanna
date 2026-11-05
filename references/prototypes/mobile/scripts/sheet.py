import sys
from pathlib import Path
from PIL import Image

src, dest, columns, width = Path(sys.argv[1]), sys.argv[2], int(sys.argv[3]), int(sys.argv[4])
names = sys.argv[5:] or sorted(p.stem for p in src.glob("*.png"))
images = [Image.open(src / f"{n}.png").convert("RGB") for n in names]
scaled = [i.resize((width, width * i.height // i.width), Image.LANCZOS) for i in images]
height = scaled[0].height
rows = (len(scaled) + columns - 1) // columns
sheet = Image.new("RGB", (columns * (width + 10), rows * (height + 10)), (120, 120, 120))
for n, image in enumerate(scaled):
    sheet.paste(image, ((n % columns) * (width + 10), (n // columns) * (height + 10)))
sheet.save(dest)
