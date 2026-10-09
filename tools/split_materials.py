"""Split a version 1 material file (MOOSEMATERIAL 1: a `materials` section, a row each) into
version 2 files, one a material: NAME.mmat beside it, the comment block above each row (and
one at the end of the row) at the top of its file. The old file is removed.

Usage (from repo root): python3 tools/split_materials.py assets/materials/standard.mmat
"""
import os, re, sys

path = sys.argv[1]
lines = open(path).read().split("\n")
header = next(l.strip() for l in lines if l.strip() and not l.strip().startswith("#"))
assert header == "MOOSEMATERIAL 1", f"{path} isn't a version 1 material file"
out_dir = os.path.dirname(path)
in_section, block, after_row, written = False, [], False, []
for raw in lines:
    text = raw.strip()
    if not in_section:
        in_section = text.startswith("materials ")
        continue
    if text.startswith("#"):
        # Column headings aren't a material's.
        if not re.match(r"#\s*id\s+name", text):
            # A comment after a row starts the next rows' block.
            if after_row:
                block, after_row = [], False
            block.append(text)
        continue
    if not text:
        continue
    row, _, comment = raw.partition("#")
    tokens = row.split()
    _, name, shader, *options = tokens
    # The block above a run of rows is each row's.
    comments = block + (["# " + comment.strip()] if comment.strip() else [])
    after_row = True
    settings = [("shader", shader)]
    translucent = False
    for option in options:
        if option == "translucent":
            translucent = True
            continue
        key, value = option.split("=", 1)
        settings.append((key, value))
    body = ["MOOSEMATERIAL 2", *comments]
    body += [f"{key:<18} {value}" for key, value in settings]
    if translucent:
        body.append("translucent")
    target = os.path.join(out_dir, f"{name}.mmat")
    assert not os.path.exists(target), f"{target} exists"
    open(target, "w").write("\n".join(body) + "\n")
    written.append(name)
os.remove(path)
print(f"wrote {len(written)} materials: {', '.join(written)}")
