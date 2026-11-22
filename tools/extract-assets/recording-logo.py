#!/usr/bin/env python3
"""Extract Steam sp/15786 Nm (Ge), without evaluating the Steam bundle.
Usage: recording-logo.py tools/cdp/build/pretty/sp.js OUTPUT_DIR
Geometry and presentation attributes are copied verbatim; only currentColor,
40px output size and Steam's direction=right CSS mirror are resolved.
"""
import hashlib
import json
from pathlib import Path
import re
import sys
from xml.sax.saxutils import quoteattr

source = Path(sys.argv[1]).read_text()
module = source.split('      15786: (je, te, t) => {', 1)[1]
assert 'Nm: () => Ge' in module[:4000]
component = module.split('        function Ge(wt) {', 1)[1].split('        function ut(wt)', 1)[0]
assert 'viewBox: "0 0 36 36"' in component
assert 'Ht == "right" ? "-1" : "1"' in component
nodes = []
for tag, props in re.findall(r'\(0, e.jsx\)\("(rect|path)", \{(.*?)\}\)', component, re.S):
    attrs = []
    for key, value in re.findall(r'(\w+): ("[^"\n]*")', props):
        key = {'fillRule': 'fill-rule', 'clipRule': 'clip-rule'}.get(key, key)
        value = json.loads(value)
        attrs.append(f'{key}={quoteattr(value)}')
    nodes.append(f'  <{tag} {" ".join(attrs)}/>')
assert len(nodes) == 6 and sum('<rect ' in n for n in nodes) == 5
out = Path(sys.argv[2])
out.mkdir(parents=True, exist_ok=True)
for direction in ('left', 'right'):
    mirror = ' transform="translate(36 0) scale(-1 1)"' if direction == 'right' else ''
    svg = '<svg xmlns="http://www.w3.org/2000/svg" width="40" height="40" viewBox="0 0 36 36" fill="none" color="#d2b76a">\n'
    svg += f'<g{mirror}>\n' + '\n'.join(nodes) + '\n</g>\n</svg>\n'
    path = out / f'recording-{direction}.svg'
    path.write_text(svg)
    print(hashlib.sha256(path.read_bytes()).hexdigest(), path)
