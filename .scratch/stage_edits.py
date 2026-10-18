"""Stage chosen edits to a file shared by several agents.

Usage: python3 -I stage_edits.py <spec.json>
spec: {"path/in/repo": [[old, new], ...], ...}. A pair whose new is
{"row": "prefix"} takes the live file's line starting with that prefix.
Each old must occur once in the HEAD version. Writes a patch of HEAD plus
the edits and applies it to the index only.
"""
import json, pathlib, subprocess, sys, tempfile

spec = json.loads(pathlib.Path(sys.argv[1]).read_text())
patch = []
with tempfile.TemporaryDirectory() as tmp:
    for rel, pairs in spec.items():
        head = subprocess.run(["git", "show", f"HEAD:{rel}"], capture_output=True, text=True, check=True).stdout
        live = pathlib.Path(rel).read_text()
        text = head
        for old, new in pairs:
            if isinstance(new, dict):
                new = next(l for l in live.splitlines() if l.startswith(new["row"]))
            assert text.count(old) == 1, (rel, old, text.count(old))
            text = text.replace(old, new)
        a = pathlib.Path(tmp, "a", rel); b = pathlib.Path(tmp, "b", rel)
        a.parent.mkdir(parents=True, exist_ok=True); b.parent.mkdir(parents=True, exist_ok=True)
        a.write_text(head); b.write_text(text)
        d = subprocess.run(["diff", "-u", f"a/{rel}", f"b/{rel}"], cwd=tmp, capture_output=True, text=True).stdout
        patch.append(d)
    p = pathlib.Path(tmp, "x.patch")
    p.write_text("".join(patch))
    subprocess.run(["git", "apply", "--cached", "-p1", str(p)], check=True)
