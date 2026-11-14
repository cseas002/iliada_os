#!/usr/bin/env python3
# Regenerates the Iliada logo set from one shared helmet geometry.
# Requires: python3, cairosvg, pillow.  Run: python3 nix/branding/tools/generate.py
import os
HERE = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
import math, os
OUT = HERE
os.makedirs(OUT+"/png", exist_ok=True)

def f(v): 
    s = f"{v:.2f}".rstrip('0').rstrip('.')
    return "0" if s in ("-0","") else s
def P(*pts): return " ".join(f"{f(x)} {f(y)}" for x,y in pts)

# ---------------- shared geometry (256 box) ----------------
CX, CY, R = 130, 128, 64          # dome circle
GAP = 6                            # crest seat gap
RI = R + GAP                       # crest inner radius
def pol(r, deg, cx=CX, cy=CY):
    a = math.radians(deg); return (cx + r*math.cos(a), cy - r*math.sin(a))

# front of dome meets brow at angle a_front
a_front = 186   # just below horizontal on the left
FB = pol(R, a_front)               # front-brow point on dome
BK = pol(R, -4)                    # back point where dome becomes back plate

HELMET = (
  # start at nose-guard tip, go up the front edge to the dome
  f"M{P((64,178))} "
  f"C{P((63,164),(FB[0]+0.105*18, FB[1]+0.995*18),FB)} "
  # dome arc over the top to the back (clockwise in screen space)
  f"A{R} {R} 0 1 1 {P(BK)} "
  # back plate down, flaring into the neck guard
  f"C{P((193,156),(190,170),(193,184))} "
  f"C{P((196,198),(206,208),(222,213))} "
  # underside of the neck guard into the neck arch
  f"C{P((206,222),(182,222),(170,212))} "
  f"C{P((163,206),(161,196),(161,186))} "
  f"C{P((161,168),(151,157),(138,157))} "
  f"C{P((124,157),(117,168),(116,182))} "
  # cheek guard back edge down to the chin point
  f"C{P((114,206),(102,226),(84,240))} "
  # cheek guard front edge up to the slit
  f"C{P((89,214),(92,188),(92,162))} "
  # eye: lower lid out to the almond tip
  f"C{P((92,155),(96,151),(103,150))} "
  f"C{P((117,148),(129,141),(136,128))} "
  # eye: upper lid back to the front, turning down into the slit
  f"C{P((120,122),(98,121),(88,127))} "
  f"C{P((83,130),(81,135),(81,142))} "
  # nose guard: slit's front edge down to the tip
  f"L{P((80,170))} Z"
)

# Crest: seated on the dome with a constant gap, thin at the brow,
# full over the crown, sweeping down the back into a tail.
c_front = 158
CI0 = pol(RI, c_front)            # inner front
CO0 = pol(RI+11, c_front+5)       # outer front tip (oblique cut)
CIB = pol(RI, -4)                 # inner back where it leaves the circle
CREST = (
  f"M{P(CO0)} "
  f"C{P((70,58),(100,34),(136,32))} "
  f"C{P((192,30),(232,72),(232,128))} "
  f"C{P((232,154),(226,176),(212,196))} "
  # tail tip, then inner edge back up, parallel to the back plate
  f"C{P((206,186),(199,172),(199,158))} "
  f"C{P((199,148),(200,140),CIB)} "
  f"A{RI} {RI} 0 0 0 {P(CI0)} Z"
)

# subtle bronze sheen on the dome (inside, follows the dome)
HI0 = pol(R-9, 168); HI1 = pol(R-9, 98)
SHEEN = f"M{P(pol(R-8,170))} A{R-8} {R-8} 0 0 1 {P(pol(R-8,100))} A{R-2} {R-2} 0 0 0 {P(pol(R-8,170, CX+4, CY+4))} Z"

DEFS = """
    <linearGradient id="il-bronze" x1="58" y1="58" x2="188" y2="230" gradientUnits="userSpaceOnUse">
      <stop offset="0" stop-color="#F6D38E"/>
      <stop offset="0.45" stop-color="#DDA34E"/>
      <stop offset="1" stop-color="#A5671F"/>
    </linearGradient>
    <linearGradient id="il-crest" x1="48" y1="34" x2="218" y2="184" gradientUnits="userSpaceOnUse">
      <stop offset="0" stop-color="#EE5442"/>
      <stop offset="0.55" stop-color="#CF3228"/>
      <stop offset="1" stop-color="#9E1F18"/>
    </linearGradient>"""

import re as _re
def shift(d, dx, dy):
    out=[]
    for cmd, args in _re.findall(r'([MCLAZ])([^MCLAZ]*)', d):
        nums=[float(v) for v in args.replace(',',' ').split()]
        if cmd=='A':
            for i in range(0,len(nums),7):
                nums[i+5]+=dx; nums[i+6]+=dy
        else:
            for i in range(0,len(nums),2):
                nums[i]+=dx; nums[i+1]+=dy
        out.append(cmd+" ".join(f(v) for v in nums))
    return " ".join(out)
DX, DY = -12, -6
CREST_D, HELMET_D, SHEEN_D = shift(CREST,DX,DY), shift(HELMET,DX,DY), shift(SHEEN,DX,DY)

def mark_group(mono=False):
    if mono:
        return f'<path d="{CREST_D}"/>\n    <path d="{HELMET_D}"/>'
    return (f'<path fill="url(#il-crest)" d="{CREST_D}"/>\n'
            f'    <path fill="url(#il-bronze)" d="{HELMET_D}"/>\n'
            f'    <path fill="#FFF1CC" opacity="0.35" d="{SHEEN_D}"/>')

def svg(vb, body, defs="", title="Iliada", w=None, h=None):
    W,H = vb[2], vb[3]
    return (f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="{" ".join(map(f,vb))}" width="{f(w or W)}" height="{f(h or H)}">\n'
            f'  <title>{title}</title>\n' + (f'  <defs>{defs}\n  </defs>\n' if defs else "") + body + '\n</svg>\n')

def write(name, s):
    open(f"{OUT}/{name}","w").write(s)

# 1. main mark
write("iliada.svg", svg((0,0,256,256), f'  <g>\n    {mark_group()}\n  </g>', DEFS))

# 3. mono
write("iliada-mono.svg", svg((0,0,256,256), f'  <g fill="currentColor">\n    {mark_group(True)}\n  </g>', title="Iliada (mono)"))

# 2. icon: squircle (superellipse n=5) as smooth cubic Beziers
def squircle(cx, cy, r, n=5, seg=48):
    pts=[]
    for i in range(seg):
        t = 2*math.pi*i/seg
        c, s = math.cos(t), math.sin(t)
        pts.append((cx + r*math.copysign(abs(c)**(2/n), c), cy + r*math.copysign(abs(s)**(2/n), s)))
    d = f"M{P(pts[0])} "
    for i in range(seg):
        p0, p1, p2, p3 = pts[i-1], pts[i], pts[(i+1)%seg], pts[(i+2)%seg]
        c1 = (p1[0]+(p2[0]-p0[0])/6, p1[1]+(p2[1]-p0[1])/6)
        c2 = (p2[0]-(p3[0]-p1[0])/6, p2[1]-(p3[1]-p1[1])/6)
        d += f"C{P(c1,c2,p2)} "
    return d + "Z"
SQ = squircle(128,128,120)
ICON_DEFS = DEFS + """
    <radialGradient id="il-navy" cx="128" cy="78" r="210" gradientUnits="userSpaceOnUse">
      <stop offset="0" stop-color="#22396A"/>
      <stop offset="0.6" stop-color="#14254A"/>
      <stop offset="1" stop-color="#0B1630"/>
    </radialGradient>
    <linearGradient id="il-rim" x1="0" y1="8" x2="0" y2="248" gradientUnits="userSpaceOnUse">
      <stop offset="0" stop-color="#FFFFFF" stop-opacity="0.14"/>
      <stop offset="0.5" stop-color="#FFFFFF" stop-opacity="0"/>
      <stop offset="1" stop-color="#000000" stop-opacity="0.18"/>
    </linearGradient>"""
S = 0.72; T = 128 - 128*S
icon_body = (f'  <path fill="url(#il-navy)" d="{SQ}"/>\n'
  f'  <path fill="none" stroke="url(#il-rim)" stroke-width="1.5" d="{squircle(128,128,119.25)}"/>\n'
  f'  <g transform="translate({f(T)} {f(T+4)}) scale({S})" fill="#040A1A" opacity="0.28">\n    {mark_group(True)}\n  </g>\n'
  f'  <g transform="translate({f(T)} {f(T+1)}) scale({S})">\n    {mark_group()}\n  </g>')
write("iliada-icon.svg", svg((0,0,256,256), icon_body, ICON_DEFS, "Iliada icon"))

# 4. wordmark "Iliada": geometric letters as filled outlines
SW = 19          # stem width
XH = 104         # x-height
CAP = 146        # cap / ascender height
def rect(x, y, w, h):     # clockwise
    return f"M{P((x,y),(x+w,y),(x+w,y+h),(x,y+h))} Z "
def ring(cx, cy, rx, ry, t):   # outer cw, inner ccw
    o = f"M{P((cx,cy-ry))} A{f(rx)} {f(ry)} 0 1 1 {P((cx,cy+ry))} A{f(rx)} {f(ry)} 0 1 1 {P((cx,cy-ry))} Z "
    i = f"M{P((cx,cy-ry+t))} A{f(rx-t)} {f(ry-t)} 0 1 0 {P((cx,cy+ry-t))} A{f(rx-t)} {f(ry-t)} 0 1 0 {P((cx,cy-ry+t))} Z "
    return o + i
def dot(cx, cy, r):
    return f"M{P((cx,cy-r))} A{r} {r} 0 1 1 {P((cx,cy+r))} A{r} {r} 0 1 1 {P((cx,cy-r))} Z "
def wordmark(x0, base):
    d = ""; x = x0
    # I with slab serifs
    sw = 64
    d += rect(x, base-CAP, sw, SW-2) + rect(x, base-SW+2, sw, SW-2) + rect(x+(sw-SW)/2, base-CAP, SW, CAP)
    x += sw + 30
    # l
    d += rect(x, base-CAP-6, SW, CAP+6); x += SW + 30
    # i
    d += rect(x, base-XH, SW, XH) + dot(x+SW/2, base-CAP+8, 12.5); x += SW + 24
    # a (single storey)
    rx = 52; ry = XH/2 + 1.5
    d += ring(x+rx, base-ry, rx, ry, SW) + rect(x+2*rx-SW, base-XH-1.5, SW, XH+1.5); x += 2*rx + 21
    # d
    d += ring(x+rx, base-ry, rx, ry, SW) + rect(x+2*rx-SW, base-CAP-6, SW, CAP+6); x += 2*rx + 21
    # a
    d += ring(x+rx, base-ry, rx, ry, SW) + rect(x+2*rx-SW, base-XH-1.5, SW, XH+1.5); x += 2*rx
    return d.strip(), x

def lockup(dark):
    ink = "#F4EFE6" if dark else "#18213A"
    ms = 1.0
    WS = 0.8
    wm, xend = wordmark(0, 0)
    tx = 272; base = 128 + CAP*WS/2 + 2
    W = tx + xend*WS + 36
    body = (f'  <g>\n    {mark_group()}\n  </g>\n'
            f'  <path fill="{ink}" transform="translate({f(tx)} {f(base)}) scale({WS})" d="{wm}"/>')
    return svg((0,0,W,256), body, DEFS, "Iliada"), W
s, LW = lockup(False); write("iliada-lockup.svg", s)
s, _ = lockup(True); write("iliada-lockup-dark.svg", s)

# ---- PNG exports ----
import cairosvg, os
B=OUT+'/'
for sz in (16,32,64,128,256,512,1024):
    cairosvg.svg2png(url=B+'iliada-icon.svg', write_to=B+f'png/iliada-icon-{sz}.png', output_width=sz, output_height=sz)
cairosvg.svg2png(url=B+'iliada.svg', write_to=B+'png/iliada-512.png', output_width=512, output_height=512)
cairosvg.svg2png(url=B+'iliada-mono.svg', write_to=B+'png/iliada-mono-512.png', output_width=512, output_height=512)
for n in ('iliada-lockup','iliada-lockup-dark'):
    cairosvg.svg2png(url=B+n+'.svg', write_to=B+f'png/{n}-1600.png', output_width=1600)

# ---- fastfetch ASCII (logo.txt) ----
import cairosvg, io, re
from PIL import Image
src=open(OUT+'/iliada-mono.svg').read()
paths=re.findall(r'<path d="([^"]*)"/>', src)
X0,Y0,W,H=38,24,184,212   # mark bbox (256 units)
COLS=32; ROWS=19
def layer(d):
    s=f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="{X0} {Y0} {W} {H}"><path d="{d}"/></svg>'
    png=cairosvg.svg2png(bytestring=s.encode(), output_width=COLS*8, output_height=ROWS*16)
    im=Image.open(io.BytesIO(png)).getchannel('A').resize((COLS,ROWS*2),Image.BOX)
    return im.load()
crest,helm=layer(paths[0]),layer(paths[1])
T=110
lines=[]
for r in range(ROWS):
    line=""; cur=None
    for c in range(COLS):
        top=[crest[c,2*r]>T, helm[c,2*r]>T]; bot=[crest[c,2*r+1]>T, helm[c,2*r+1]>T]
        t=any(top); b=any(bot)
        ch = "█" if t and b else "▀" if t else "▄" if b else " "
        if ch!=" ":
            col = "1" if (top[0] or bot[0]) and not (top[1] or bot[1]) else "2"
            if col!=cur: line+=f"${col}"; cur=col
        line+=ch
    lines.append(line.rstrip())
while lines and not lines[-1].strip(): lines.pop()
open(OUT+'/logo.txt','w').write("\n".join(lines)+"\n")
