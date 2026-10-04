#!/usr/bin/env python3
"""Remove a crate's root `pub use` re-exports and repoint every consumer
to the canonical path. Usage:
  purge_pub_use.py <crate_dir> <crate_extern_name> [<shim_file_rel>]
Parses top-level `pub use crate::module::{..}` / `pub use module::{..}`
lines in the given file (default src/lib.rs), builds item -> full-path
maps, rewrites all .rs files under frontend/ apps/ backend/ protocol/,
and deletes the pub use lines. Re-exports of FOREIGN crates are left in
place (reported) — they need a direct dep decision.
"""
import os, re, sys

crate_dir, extern_name = sys.argv[1], sys.argv[2]
shim = sys.argv[3] if len(sys.argv) > 3 else 'src/lib.rs'
lib = os.path.join(crate_dir, shim)
src = open(lib).read()

# ---- parse pub use lines (possibly multiline) at top level
pub_use_re = re.compile(r'^pub use ([^;]+);\s*$', re.M | re.S)
# normalize multiline statements first: join lines of each pub use
stmts = []
spans = []
i = 0
lines = src.split('\n')
n = 0
while n < len(lines):
    if lines[n].startswith('pub use '):
        start = n
        buf = lines[n]
        while ';' not in buf:
            n += 1
            buf += ' ' + lines[n].strip()
        stmts.append(buf)
        spans.append((start, n))
    n += 1

item_map = {}      # item-name-as-consumed -> full path inside crate (module::item) or None if foreign
foreign = []
kept = []

def add(path, name, alias):
    # path like crate::module::sub, name the item
    consumed = alias or name
    if path.startswith('crate::'):
        inner = path[len('crate::'):]
        item_map[consumed] = (inner, name)
    elif path.split('::')[0] in ('self',):
        pass
    else:
        # relative module of same crate (pub use module::X) or foreign crate
        head = path.split('::')[0]
        if os.path.exists(os.path.join(crate_dir, 'src', head + '.rs')) or \
           os.path.exists(os.path.join(crate_dir, 'src', head)):
            item_map[consumed] = (path, name)
        else:
            foreign.append((path, name, alias))

for stmt in stmts:
    body = stmt[len('pub use '):].rstrip(';').strip()
    m = re.match(r'^([\w:]+)::\{(.*)\}$', body, re.S)
    if m:
        prefix, inner = m.group(1), m.group(2)
        for part in [p.strip() for p in inner.split(',') if p.strip()]:
            if '::' in part:
                # nested path inside braces
                sub = part
                alias = None
                if ' as ' in sub:
                    sub, alias = [x.strip() for x in sub.split(' as ')]
                name = sub.split('::')[-1]
                add(prefix + '::' + '::'.join(sub.split('::')[:-1]) + '::' + name if '::' in sub else prefix + '::' + sub, name, alias)
            else:
                alias = None
                name = part
                if ' as ' in part:
                    name, alias = [x.strip() for x in part.split(' as ')]
                add(prefix + '::' + name, name, alias)
    else:
        sub = body
        alias = None
        if ' as ' in sub:
            sub, alias = [x.strip() for x in sub.split(' as ')]
        name = sub.split('::')[-1]
        add(sub, name, alias)

if not item_map:
    print('nothing to do; foreign:', foreign)
    sys.exit(0)

# ---- delete the pub use lines that were fully internal
drop = set()
for stmt, (a, b) in zip(stmts, spans):
    body = stmt[len('pub use '):].rstrip(';').strip()
    head = body.split('::')[0].replace('crate', '').strip(':') or body.split('::')[0]
    first = body.split('::')[0]
    internal = first == 'crate' or os.path.exists(os.path.join(crate_dir, 'src', first + '.rs')) or os.path.exists(os.path.join(crate_dir, 'src', first))
    if internal:
        for k in range(a, b + 1):
            drop.add(k)
new_lines = [l for i, l in enumerate(lines) if i not in drop]
open(lib, 'w').write('\n'.join(new_lines))

# ---- rewrite consumers
roots = ['frontend', 'apps', 'backend', 'protocol']
ext = extern_name.replace('-', '_')

def rewrite(text, crate_prefix):
    # crate_prefix: 'text' for external consumers, 'crate' inside the crate
    changed = False
    # 1) qualified paths: prefix::Item -> prefix::module::Item
    for consumed, (inner, name) in item_map.items():
        pat = re.compile(r'(?<![:\w])' + crate_prefix + r'::' + consumed + r'\b')
        repl = crate_prefix + '::' + inner if consumed == name else crate_prefix + '::' + inner + ' as ' + consumed
        # alias only valid in use statements; for qualified paths use real name
        qual = crate_prefix + '::' + (inner if consumed == name else inner)
        new = pat.sub(qual, text)
        if new != text:
            changed = True
            text = new
    # 2) use statements with brace groups: use prefix::{A, B::c, D};
    use_re = re.compile(r'use ' + crate_prefix + r'::\{([^;{}]*(?:\{[^{}]*\}[^;{}]*)*)\};', re.S)
    def fix_group(m):
        inner = m.group(1)
        parts = []
        depth = 0
        cur = ''
        for ch in inner:
            if ch == '{': depth += 1
            if ch == '}': depth -= 1
            if ch == ',' and depth == 0:
                parts.append(cur); cur = ''
            else:
                cur += ch
        if cur.strip(): parts.append(cur)
        out = []
        for p in parts:
            q = p.strip()
            leaf = q.split('::')[0].split(' as ')[0].strip()
            if '::' not in q and leaf in item_map:
                inner_path, name = item_map[leaf]
                alias = '' if leaf == name else ' as ' + leaf
                if ' as ' in q:
                    name_only, al = [x.strip() for x in q.split(' as ')]
                    out.append(inner_path + ' as ' + al)
                else:
                    out.append(inner_path + alias)
            else:
                out.append(q)
        return 'use ' + crate_prefix + '::{' + ', '.join(out) + '};'
    new = use_re.sub(fix_group, text)
    if new != text:
        changed = True
        text = new
    return text, changed

count = 0
for root in roots:
    for r, dirs, files in os.walk(root):
        if 'target' in r.split(os.sep) or 'vendor' in r.split(os.sep):
            continue
        for f in files:
            if not f.endswith('.rs'):
                continue
            p = os.path.join(r, f)
            t = open(p).read()
            inside = os.path.abspath(p).startswith(os.path.abspath(crate_dir) + os.sep)
            prefix = 'crate' if inside else ext
            t2, ch = rewrite(t, prefix)
            if inside:
                # inside the crate, also fix bare `use crate::{...}` handled above; nothing else
                pass
            if ch:
                open(p, 'w').write(t2)
                count += 1
print('map:', {k: v[0] for k, v in item_map.items()})
print('files changed:', count)
if foreign:
    print('FOREIGN re-exports left in place:', foreign)
