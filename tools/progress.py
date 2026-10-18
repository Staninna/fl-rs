"""Write the progress block in README.md from docs/plugins.md and docs/parity.md.

Usage: python3 -I tools/progress.py

The block sits between the progress markers in README.md. CI runs this on
every push to main and commits the result, so change the docs, not the block.
"""
import pathlib, re

ROOT = pathlib.Path(__file__).resolve().parent.parent
START, END = "<!-- progress:start -->", "<!-- progress:end -->"


def bar(done, total, width=20):
    filled = round(width * done / total)
    return "█" * filled + "░" * (width - filled)


def number(pattern, text, source):
    found = re.search(pattern, text)
    assert found, f"{source}: no line matches {pattern!r}"
    return int(found[1]), int(found[2])


def block():
    plugins = (ROOT / "docs/plugins.md").read_text(encoding="utf-8")
    parity = (ROOT / "docs/parity.md").read_text(encoding="utf-8")

    # kind | plugin | DLL KB | render test | used by projects | status
    rows = []
    for line in plugins.splitlines():
        cells = [c.strip() for c in line.strip().strip("|").split("|")]
        if len(cells) == 6 and cells[0] in ("Generator", "Effect"):
            rows.append((cells[0], int(cells[4]), cells[5].startswith("implemented")))

    table = [
        ("Generators built", [built for kind, _, built in rows if kind == "Generator"]),
        ("Effects built", [built for kind, _, built in rows if kind == "Effect"]),
    ]
    lines = ["| | done | of | |", "|---|---:|---:|---|"]
    for label, built in table:
        lines.append(row(label, sum(built), len(built)))
    uses = sum(used for _, used, _ in rows)
    lines.append(row("Plugin uses in shipped projects, built", sum(u for _, u, b in rows if b), uses))
    sound = number(r"(\d+) of (\d+) shipped projects have sound", plugins, "docs/plugins.md")
    lines.append(row("Shipped projects with sound", *sound))
    trip = number(r"Container round trip: (\d+)/(\d+) projects", parity, "docs/parity.md")
    lines.append(row("Shipped projects that round-trip byte for byte", *trip))
    events = number(r"Events with a reading: (\d+)/(\d+)", parity, "docs/parity.md")
    lines.append(row("`.flp` events understood", *events))
    lines += [
        "",
        "A plugin used by 20 projects counts 20 times in the plugin uses. The count of",
        "projects with sound comes from the last corpus render, which needs FL's files;",
        "`docs/plugins.md` keeps it. `tools/progress.py` writes this table from the docs.",
    ]
    return "\n".join([START, *lines, END])


def row(label, done, total):
    return f"| {label} | {done} | {total} | `{bar(done, total)}` {100 * done // total}% |"


def main():
    readme = ROOT / "README.md"
    text = readme.read_text(encoding="utf-8")
    head, rest = text.split(START, 1)
    _, tail = rest.split(END, 1)
    readme.write_text(head + block() + tail, encoding="utf-8")


main()
