"""Top-down SVG preview of a .mmp level: sectors, portals, entities.

Usage: python3 tools/preview_level.py assets/levels/two_rooms.mmp assets/levels/two_rooms_preview.svg
PNG on macOS: qlmanage -t -s 960 -o <dir> <file>.svg
"""
import math, shlex, sys
L = [shlex.split(l.split('#',1)[0]) for l in open(sys.argv[1])]
L = [t for t in L if t]
secs = {}; i = 1
while i < len(L):
    t = L[i]
    if t[0] == 'name': i += 1; continue
    n = int(t[-1])   # 'values <name> <count>' carries an extra token
    secs[t[0] if t[0] != 'values' else 'values:' + t[1]] = [r[1:] for r in L[i+1:i+1+n]]; i += 1 + n
V = [tuple(map(float, r)) for r in secs['vertices']]
S = 30
# page x = -z (room_a left, room_b right), page y = x (+X down the page)
P = lambda x, z: f'{(10 - z) * S:.1f},{(x + 6) * S:.1f}'
o = [f'<svg xmlns="http://www.w3.org/2000/svg" width="{32*S}" height="{32*S}"><rect width="100%" height="100%" fill="#fff"/>']
colors = {'0': '#f3dcc4', '1': '#dfe8df', '2': '#d4def0'}
for r in secs['surfaces']:
    pts = [V[int(x.split(':')[0])] for x in r[4:]]
    if all(abs(p[1]) < 1e-9 for p in pts):
        o.append('<polygon points="%s" fill="%s" stroke="#888"/>' % (' '.join(P(p[0], p[2]) for p in pts), colors[r[0]]))
for r in secs['surfaces']:
    if int(r[1]) >= 0:
        pts = [V[int(x)] for x in r[4:]]
        a, b = P(min(p[0] for p in pts), pts[0][2]).split(','), P(max(p[0] for p in pts), pts[0][2]).split(',')
        o.append(f'<line x1="{a[0]}" y1="{a[1]}" x2="{b[0]}" y2="{b[1]}" stroke="#d00" stroke-width="5" stroke-dasharray="6,4"/>')
for r in secs['entities']:
    x, y, z = map(float, r[3:6]); yaw = math.radians(float(r[7]))
    if r[2] == '-':
        fx, fz = -math.sin(yaw), -math.cos(yaw)
        a, b = P(x, z).split(','), P(x + fx, z + fz).split(',')
        o.append(f'<circle cx="{a[0]}" cy="{a[1]}" r="7" fill="#2a2"/><line x1="{a[0]}" y1="{a[1]}" x2="{b[0]}" y2="{b[1]}" stroke="#2a2" stroke-width="3"/>')
        o.append(f'<text x="{float(a[0])-30}" y="{float(a[1])+24}" font-size="13" font-family="Helvetica">spawn</text>')
        continue
    c = [(-.5,-.5),(.5,-.5),(.5,.5),(-.5,.5)]
    pts = [(x + u*math.cos(yaw) + w*math.sin(yaw), z - u*math.sin(yaw) + w*math.cos(yaw)) for u, w in c]
    fill = '#8a5a2b' if y == 0 else '#d09050'
    o.append('<polygon points="%s" fill="%s" fill-opacity="0.8" stroke="#000"/>' % (' '.join(P(p, q) for p, q in pts), fill))
    a = P(x, z).split(',')
    o.append(f'<text x="{float(a[0])-30}" y="{float(a[1])+(-24 if y > 0 else 34)}" font-size="13" font-family="Helvetica">{r[10]}</text>')
for label, z in (('room_a', 7.5), ('hallway', -3), ('room_b', -12.5)):
    a = P(-3.3, z).split(',')
    o.append(f'<text x="{a[0]}" y="{a[1]}" font-size="17" font-family="Helvetica" fill="#555">{label}</text>')
o.append(f'<text x="20" y="{15*S}" font-size="14" font-family="Helvetica">top-down view: -Z to the right, +X down; red dashes = portals; light crate = stacked</text></svg>')
open(sys.argv[2], 'w').write('\n'.join(o))
