"""Write the benchmark block in README.md from a run of the profile harness.

Usage: FL_RS_CORPUS="<FL Studio>/Data" taskset -c 2 \\
           cargo run --release -p engine --example profile \\
           | python3 -I tools/bench.py
       python3 -I tools/bench.py --check

The harness prints a line per project. This reads them on stdin, passes
them through, and writes between the bench markers in README.md: the
commit it ran at, how many projects have a call over budget once warm,
and the slowest projects, warm and cold. Lines from several runs, one
project each, may be joined. CI has no FL Studio, so it cannot run the
harness: it runs --check, which only reads the block and stops with an
error where it is missing or holds no table.
"""
import pathlib, re, subprocess, sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
START, END = "<!-- bench:start -->", "<!-- bench:end -->"
SHOWN = 10
LINE = re.compile(
    r"\s*([\d.]+)x\s+p99\s+[\d.]+ ms \(\s*(\d+)%\)\s+worst\s+[\d.]+ ms \(\s*(\d+)%\)"
    r".*?over\s+(\d+)\s+first.*?cold\s+([\d.]+)x worst\s+[\d.]+ ms \(\s*(\d+)%\)"
    r"\s+[0-9a-f]{16}\s+(.*)"
)
SUMMARY = re.compile(r"projects at (\d+) frames \(([\d.]+) ms a call\), (\d+) warm-up rounds")


def block(projects, frames, budget, warmup, commit):
    projects.sort(key=lambda p: float(p["speed"]))
    over = sum(1 for p in projects if p["over"] > 0)
    lines = [
        f"The profile harness at `{commit}`, {len(projects)} projects in calls of"
        f" {frames} frames ({budget} ms each), each on one core. Cold is the first"
        f" round of a project, every call timed; warm is the round after {warmup}"
        f" more. Warm, {over} projects have a call that runs over. The {SHOWN} slowest:",
        "",
        "| Project | Warm | p99 call | Worst call | Calls over | Cold | Cold worst call |",
        "|---|---:|---:|---:|---:|---:|---:|",
    ]
    for p in projects[:SHOWN]:
        lines.append(
            f"| {p['name']} | {p['speed']}x | {p['p99']}% | {p['worst']}% | {p['over']}"
            f" | {p['cold']}x | {p['cold_worst']}% |"
        )
    lines.append("")
    lines.append(
        "A call's time is a share of the time it plays. `tools/bench.py` writes this block"
        " from a run of the harness."
    )
    return "\n".join(lines)


def replace(readme, text):
    start, end = readme.index(START) + len(START), readme.index(END)
    return readme[:start] + "\n" + text + "\n" + readme[end:]


def main():
    path = ROOT / "README.md"
    readme = path.read_text(encoding="utf-8")
    assert START in readme and END in readme, "README.md: no bench markers"
    if sys.argv[1:] == ["--check"]:
        inside = readme[readme.index(START) : readme.index(END)]
        assert "| Project |" in inside, "README.md: the bench block holds no table"
        return
    projects, frames, budget, warmup = [], "512", "10.67", "2"
    for line in sys.stdin:
        sys.stdout.write(line)
        if found := LINE.match(line):
            speed, p99, worst, over, cold, cold_worst, name = found.groups()
            projects.append(
                {
                    "speed": speed,
                    "p99": p99,
                    "worst": worst,
                    "over": int(over),
                    "cold": cold,
                    "cold_worst": cold_worst,
                    "name": pathlib.PurePath(name.strip()).stem,
                }
            )
        elif found := SUMMARY.search(line):
            frames, budget, warmup = found.groups()
    assert projects, "stdin: no project lines from the profile harness"
    commit = subprocess.run(
        ["git", "rev-parse", "--short", "HEAD"], cwd=ROOT, capture_output=True, text=True
    ).stdout.strip()
    block_text = block(projects, frames, budget, warmup, commit)
    path.write_text(replace(readme, block_text), encoding="utf-8")


main()
