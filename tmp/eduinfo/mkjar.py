import json, glob, time
from pathlib import Path
src = Path('/Users/mi7qi/project/JIsjtu/agent_backend/.sso')
f = list(src.glob('*/cookies.json'))[0]
data = json.loads(f.read_text())
lines = ['# Netscape HTTP Cookie File']
now = int(time.time())
for c in data:
    dom = c.get('domain','')
    if not (dom.lstrip('.') == 'sjtu.edu.cn' or dom.lstrip('.').endswith('.sjtu.edu.cn')):
        continue
    exp = c.get('expires', -1)
    if exp is None or exp < 0:
        exp = now + 86400
    if exp <= now:
        print('skip expired', c['name'], dom)
        continue
    include_sub = 'TRUE' if dom.startswith('.') else 'FALSE'
    secure = 'TRUE' if c.get('secure') else 'FALSE'
    lines.append('\t'.join([dom, include_sub, c.get('path','/'), secure, str(int(exp)), c['name'], c['value']]))
Path('jar.txt').write_text('\n'.join(lines) + '\n')
print('cookies written:', len(lines)-1)
for l in lines[1:]:
    p = l.split('\t')
    print(' ', p[5], p[0], 'exp', p[4])
