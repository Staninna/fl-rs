"""Write the progress block in README.md from docs/plugins.md and docs/parity.md.

Usage: python3 -I tools/progress.py
       cargo test --release -p engine --test corpus -- --nocapture 2>&1 \\
           | python3 -I tools/progress.py --render

The block sits between the progress markers in README.md: the table and
the lists of plugins built. CI runs this on every push to main and commits
the result, so change the docs, not the block.

--render reads a corpus render's output on stdin, passes it through, and
writes its counts (projects with sound and in full, effect slots hosted,
generator channels playing) and the commit it ran at into docs/plugins.md
before it writes the block.
"""
import pathlib, re, subprocess, sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
START, END = "<!-- progress:start -->", "<!-- progress:end -->"
SOUND = r"Corpus render at `\w+`: (\d+) of (\d+) shipped projects have sound"


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
    rows, names = [], {"Generator": [], "Effect": []}
    exact = documented = 0
    for line in plugins.splitlines():
        cells = [c.strip() for c in line.strip().strip("|").split("|")]
        if len(cells) == 6 and cells[0] in ("Generator", "Effect"):
            kind, name, status = cells[0], cells[1], cells[5]
            built = status.startswith("implemented")
            rows.append((kind, int(cells[4]), built))
            if built:
                rough = re.match(r"implemented[^,(]*, approximate", status)
                names[kind].append(f"{name} (approximate)" if rough else name)
                exact += "approximat" not in status
                doc = re.search(r"docs/plugins/[\w.-]+\.md", status)
                documented += bool(doc and (ROOT / doc[0]).exists())
    built = sum(b for _, _, b in rows)

    table = [
        ("Stock generators built, of all FL ships", [b for kind, _, b in rows if kind == "Generator"]),
        ("Stock effects built, of all FL ships", [b for kind, _, b in rows if kind == "Effect"]),
    ]
    lines = ["| | done | of | |", "|---|---:|---:|---|"]
    for label, of_kind in table:
        lines.append(row(label, sum(of_kind), len(of_kind)))
    lines.append(row("Built plugins with nothing approximated", exact, built))
    lines.append(row("Built plugins with their own doc", documented, built))
    uses = sum(used for _, used, _ in rows)
    built_uses = sum(u for _, u, b in rows if b)
    lines.append(row("Plugin uses in shipped projects that are built", built_uses, uses))
    sound = number(SOUND, plugins, "docs/plugins.md")
    lines.append(row("Shipped projects with any sound", *sound))
    full = number(r"(\d+) of (\d+) play in full", plugins, "docs/plugins.md")
    lines.append(row("Shipped projects that play in full", *full))
    slots = number(r"(\d+) of (\d+) effect slots", plugins, "docs/plugins.md")
    lines.append(row("Effect slots hosted where sound reaches", *slots))
    channels = number(r"(\d+) of (\d+) generator channels", plugins, "docs/plugins.md")
    lines.append(row("Generator channels with notes that play", *channels))
    trip = number(r"Container round trip: (\d+)/(\d+) projects", parity, "docs/parity.md")
    lines.append(row("Shipped projects saved back byte for byte", *trip))
    events = number(r"Events with a reading: (\d+)/(\d+)", parity, "docs/parity.md")
    lines.append(row("Event types in shipped projects with a reading", *events))
    confidence = re.search(r"loader\+pyflp (\d+),.*conflict (\d+), pyflp (\d+),", parity)
    assert confidence, "docs/parity.md: no confidence counts"
    agreed, conflict, pyflp = int(confidence[1]), confidence[2], confidence[3]
    lines.append(row("Event readings FL's loader and PyFLP agree on", agreed, events[1]))
    dictionary = (ROOT / "docs/event-dictionary.md").read_text(encoding="utf-8")
    known = len(re.findall(r"(?m)^\| *\d+ ", dictionary))
    lines += [
        "",
        "The plugin rows count against everything FL ships and everything built. The",
        "rest count what FL's 165 shipped projects contain, so 100% there is not 100%",
        "of FL:",
        "",
        "- Nothing approximated: the plugin's row in `docs/plugins.md` names no",
        "  approximation. It is read from the binary, not yet checked against FL's output.",
        "- Plugin uses count once per project, and include third-party VSTs and",
        "  visualisers that will never be built, so this row cannot reach 100%.",
        "- A project has sound when anything comes out of it, even with half its plugins",
        "  passed through. It plays in full when it has sound, every plugin is hosted,",
        "  nothing is approximated, every sample is found and decoded, and every",
        "  automation target and clip stretch is applied. Neither means it sounds like FL.",
        "- The project, slot and channel counts come from the last corpus render, which",
        "  needs FL's files, so CI cannot rerun it; `tools/progress.py --render` records",
        "  them in `docs/plugins.md`.",
        "- Saved back byte for byte: unknown bytes are carried through, not understood.",
        f"- Event types: the shipped projects use {events[1]} of the {known} in",
        "  `docs/event-dictionary.md`, which also lists what FL's loader and PyFLP know.",
        f"  {conflict} readings conflict with PyFLP and {pyflp} rest on PyFLP alone",
        "  (`docs/parity.md`).",
        "",
        "Built so far, most used in the shipped projects first. `docs/plugins.md` says",
        "what is exact and what is approximated in each.",
        "",
        f"- Generators: {', '.join(names['Generator'])}.",
        f"- Effects: {', '.join(names['Effect'])}.",
        "",
        "`tools/progress.py` writes this block from the docs.",
    ]
    return "\n".join([START, *lines, END])


# A project's line in the corpus render: what it left out or approximated.
PROJECT = re.compile(
    r"^.+: notes \d+ voices \d+ clips \d+ stretched (\d+) unresolved (\d+) unsupported (\d+) "
    r"skipped \d+ automation \d+ unsupported targets (\d+) hosted slots \d+ unhosted (\d+) "
    r"generators \d+ unhosted (\d+) approximated generators (\d+) approximated (\d+) "
    r".* peak ([\d.]+) "
)


def record_render():
    """Pass a corpus render's output through and keep its counts, and the
    commit it ran at, in docs/plugins.md."""
    summary = effects = generators = None
    full = seen = 0
    for line in sys.stdin:
        print(line, end="", flush=True)
        if project := PROJECT.match(line):
            *gaps, peak = project.groups()
            seen += 1
            full += float(peak) > 0 and all(int(gap) == 0 for gap in gaps)
        summary = re.search(r"corpus render: (\d+) projects, (\d+) with sound", line) or summary
        effects = re.search(r"corpus effects: (\d+) slots hosted and (\d+) passed", line) or effects
        generators = re.search(r"corpus generators: (\d+) channels hosted and (\d+) silent", line) or generators
    assert summary and effects and generators, "no corpus render summary on stdin; did it run?"
    commit = subprocess.run(
        ["git", "-C", str(ROOT), "rev-parse", "--short", "HEAD"],
        capture_output=True, text=True, check=True,
    ).stdout.strip()
    projects, sound = int(summary[1]), int(summary[2])
    assert seen == projects, f"read {seen} project lines of {projects}; did their format change?"
    hosted, passed = int(effects[1]), int(effects[2])
    playing, silent = int(generators[1]), int(generators[2])
    line = (
        f"Corpus render at `{commit}`: {sound} of {projects} shipped projects have sound with "
        f"the stock plugins built so far, and {full} of {projects} play in full. "
        f"{hosted} of {hosted + passed} effect slots on inserts that sound reaches are hosted, "
        f"and {playing} of {playing + silent} generator channels with notes play."
    )
    path = ROOT / "docs/plugins.md"
    text, count = re.subn(r"Corpus render at `\w+`:.*", line, path.read_text(encoding="utf-8"))
    assert count == 1, "docs/plugins.md: no corpus render line to update"
    path.write_text(text, encoding="utf-8")


def row(label, done, total):
    return f"| {label} | {done} | {total} | `{bar(done, total)}` {100 * done // total}% |"


def main():
    if sys.argv[1:] == ["--render"]:
        record_render()
    readme = ROOT / "README.md"
    text = readme.read_text(encoding="utf-8")
    head, rest = text.split(START, 1)
    _, tail = rest.split(END, 1)
    readme.write_text(head + block() + tail, encoding="utf-8")


main()
