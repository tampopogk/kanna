import sys
from pathlib import Path

source, theme, out = sys.argv[1:4]
css = "<style>.intro,.controls,figcaption,section.part>h2,section.part>p,.macs,#decisions{display:none!important}main{padding:0;max-width:none}#stage{display:grid;grid-template-columns:repeat(8,414px)}section.part,.wall{display:contents}section.part{margin:0}figure{height:868px}.phone{box-shadow:none}</style></head>"
html = Path(source).read_text().replace("</head>", css)
if theme == "light":
    html = html.replace('<html lang="en">', '<html lang="en" data-theme="light">')
Path(out).write_text(html)
