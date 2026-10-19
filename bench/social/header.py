#!/opt/homebrew/bin/python3
"""Render the 1200x675 RGBA overlay for one split-screen clip: the caption bar,
the two pane titles and the divider between the panes.

usage: header.py <out.png> <caption> <left title> <right title>

This ffmpeg has no drawtext (built without freetype), so the text is drawn
here with Pillow and laid over the stacked panes with ffmpeg's overlay.
"""
import sys
from PIL import Image, ImageDraw, ImageFont

W, H, BAR = 1200, 675, 100
BG = (22, 24, 29, 255)
FG = (240, 240, 240, 255)
DIM = (150, 155, 165, 255)
AVER = (110, 200, 140, 255)
HELV = "/System/Library/Fonts/Helvetica.ttc"

out, caption, left, right = sys.argv[1:5]
img = Image.new("RGBA", (W, H), (0, 0, 0, 0))
d = ImageDraw.Draw(img)
d.rectangle([0, 0, W, BAR], fill=BG)
cap = ImageFont.truetype(HELV, 34, index=1)
lab = ImageFont.truetype(HELV, 22, index=1)
d.text((W / 2, 36), caption, font=cap, fill=FG, anchor="mm")
d.text((W / 4, 80), left, font=lab, fill=DIM, anchor="mm")
d.text((3 * W / 4, 80), right, font=lab, fill=AVER, anchor="mm")
d.line([(0, BAR - 1), (W, BAR - 1)], fill=(60, 64, 72, 255), width=2)
d.line([(W / 2 - 1, 64), (W / 2 - 1, H)], fill=(60, 64, 72, 255), width=2)
img.save(out)
