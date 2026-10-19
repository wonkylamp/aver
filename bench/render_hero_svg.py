#!/usr/bin/env python3
"""Render terminal SVGs from captured output. Standard library only, deterministic.

  render_hero_svg.py [capture.json] [out.svg]
      The split-screen hero from bench/hero/capture.json (default) into
      assets/hero-comparison.svg (default): a plain `git worktree add` + `npm ci`
      per agent on the left, `aver workspace new --count 4` on the right, replayed
      from the median rep's real output and timings.

  render_hero_svg.py --single <txt> <out.svg> [--title <title>]
      One static terminal pane from a text file, colored from its ANSI SGR codes,
      or by verdict keywords when it has none.
"""
import json
import os
import re
import sys
from dataclasses import dataclass, field
from xml.sax.saxutils import escape

FONT = "ui-monospace,SFMono-Regular,Menlo,Consolas,'Liberation Mono',monospace"
FS = 13          # font size, px
LH = 18          # line height, px
CW = 7.85        # advance of one monospace character at FS, px (Menlo/SF Mono)
PAD = 16
BAR = 30         # window title bar height
PLAY = 4.0       # playback seconds per real second
HOLD = 3.5       # playback seconds the final frame holds before the loop
FPS = 10         # timer frames per playback second

COLORS = {
    "bg": "#0d1117", "pane": "#161b22", "border": "#30363d", "bar": "#21262d",
    "fg": "#e6edf3", "dim": "#8b949e", "prompt": "#58a6ff", "green": "#3fb950",
    "red": "#f85149", "yellow": "#d29922", "magenta": "#bc8cff", "cyan": "#39c5cf",
}
ANSI = {30: "dim", 31: "red", 32: "green", 33: "yellow", 34: "prompt", 35: "magenta",
        36: "cyan", 37: "fg", 90: "dim", 91: "red", 92: "green", 93: "yellow",
        94: "prompt", 95: "magenta", 96: "cyan", 97: "fg"}


@dataclass
class Span:
    text: str
    color: str = "fg"
    bold: bool = False


@dataclass
class Line:
    spans: list
    at: float = 0.0  # real seconds after the pane's start when the line appears


@dataclass
class Pane:
    title: str
    lines: list = field(default_factory=list)
    end: float = 0.0          # real seconds when the pane's work is done
    done_label: str = "DONE"


def keyword_color(text):
    t = text.lstrip()
    if t.startswith(("DONE: PASS", "Accepted ", "✔")):
        return "green"
    if t.startswith(("NOT DONE: FAIL", "exit 1", "✖")) or " FAIL_OWN" in t:
        return "red"
    if t.startswith(("NOT DONE", "NEEDS HUMAN REVIEW", "﹣")) or "# SKIP" in t:
        return "yellow"
    if t.startswith(("aver:", "ℹ", "Preparing", "HEAD is now", "added ")):
        return "dim"
    return "fg"


def parse_ansi(text):
    """Split one line with SGR codes into spans; a line without any is colored by keywords."""
    if "\x1b[" not in text:
        return [Span(text, keyword_color(text))]
    spans, color, bold = [], "fg", False
    for part in re.split(r"(\x1b\[[0-9;]*m)", text):
        m = re.fullmatch(r"\x1b\[([0-9;]*)m", part)
        if not m:
            if part:
                spans.append(Span(part, color, bold))
            continue
        for code in [int(c) for c in (m.group(1) or "0").split(";") if c]:
            if code == 0:
                color, bold = "fg", False
            elif code == 1:
                bold = True
            elif code in ANSI:
                color = ANSI[code]
    return spans


def wrap(spans, cols, words=False):
    """Hard-wrap a line's spans at cols characters, as a terminal does; with
    words, at the last space that fits."""
    rows, row, n = [], [], 0
    for s in spans:
        text = s.text
        while text:
            room = cols - n
            cut = room
            if words and len(text) > room:
                space = text.rfind(" ", 0, room + 1)
                if space > 0:
                    cut = space + 1
            chunk, text = text[:cut], text[cut:]
            if words and n + len(chunk) < cols and text:
                n = cols
            row.append(Span(chunk, s.color, s.bold))
            n += len(chunk)
            if n >= cols and text:
                rows.append(row)
                row, n = [], 0
    rows.append(row)
    return rows


def prompt_line(cwd, cmd):
    return [Span(cwd, "prompt", True), Span(" $ ", "dim"), Span(cmd, "fg", True)]


def output_lines(text):
    return [l for l in text.rstrip("\n").split("\n") if l.strip()]


def git_pane(cap, rep):
    steps = {s["name"]: s for s in cap["runs"][rep] if s["side"] == "git"}
    pane, t = Pane("git worktree + npm ci, 4 agents"), 0.0
    for k in range(1, 5):
        s = steps[f"worktree-add-{k}"]
        pane.lines.append(Line(prompt_line("repo", s["cmd"]), t))
        t += s["wall_s"]
        pane.lines += [Line([Span(l, keyword_color(l))], t) for l in output_lines(s["output"])]
    for k in range(1, 5):
        s = steps[f"npm-ci-{k}"]
        pane.lines.append(Line(prompt_line(f"wt{k}", s["cmd"]), t))
        t += s["wall_s"]
        pane.lines += [Line([Span(l, keyword_color(l))], t) for l in output_lines(s["output"])]
    pane.end = t
    return pane


def aver_pane(cap, rep):
    s = next(x for x in cap["runs"][rep] if x["name"] == "workspace-new-4")
    pane = Pane("aver, 4 agents")
    pane.lines.append(Line(prompt_line("repo", short_cmd(s["cmd"])), 0.0))
    out = output_lines(s["output"])
    dir_ms = [float(m) / 1000 for m in re.findall(r" in (\d+(?:\.\d+)?) ms \(", s["output"])]
    # The import and task come first; each directory line lands after its own measured time.
    t = max(s["wall_s"] - sum(dir_ms), 0.0)
    i = 0
    for l in out:
        if l.startswith("aver: ws_"):
            t += dir_ms[i]
            i += 1
        pane.lines.append(Line([Span(l, keyword_color(l))], min(t, s["wall_s"])))
    pane.end = s["wall_s"]
    return pane


def short_cmd(cmd):
    return re.sub(r"^\S*/aver ", "aver ", cmd)


class Anim:
    """Opacity keyframes shared by every element that appears or vanishes at the same moments."""

    def __init__(self, period):
        self.period = period
        self.names = {}

    def cls(self, on, off=None):
        key = (round(on, 2), None if off is None else round(off, 2))
        if key not in self.names:
            self.names[key] = f"k{len(self.names)}"
        return self.names[key]

    def css(self):
        out = []
        for (on, off), name in self.names.items():
            a = max(min(on / self.period * 100, 100), 0)
            frames = f"0%{{opacity:0}}{a:.2f}%{{opacity:1}}"
            if off is not None:
                frames += f"{min(off / self.period * 100, 100):.2f}%{{opacity:0}}"
            frames += f"100%{{opacity:{0 if off is not None else 1}}}"
            out.append(f"@keyframes {name}{{{frames}}}"
                       f".{name}{{animation:{name} {self.period:.2f}s step-end infinite}}")
        return "\n".join(out)


def tspans(row):
    out = []
    for s in row:
        if not s.text:
            continue
        weight = ' font-weight="700"' if s.bold else ""
        out.append(f'<tspan fill="{COLORS[s.color]}"{weight}>{escape(s.text)}</tspan>')
    return "".join(out)


def window(x, y, w, h, title):
    return (f'<rect x="{x}" y="{y}" width="{w}" height="{h}" rx="8" fill="{COLORS["pane"]}" stroke="{COLORS["border"]}"/>'
            f'<path d="M{x} {y + BAR}h{w}" stroke="{COLORS["border"]}"/>'
            + "".join(f'<circle cx="{x + 16 + i * 18}" cy="{y + BAR / 2}" r="5.5" fill="{c}"/>'
                      for i, c in enumerate(("#ff5f57", "#febc2e", "#28c840")))
            + f'<text x="{x + w / 2}" y="{y + BAR / 2 + 4.5}" text-anchor="middle" fill="{COLORS["dim"]}">{escape(title)}</text>')


def render_pane(pane, x, y, w, h, anim, total_end):
    cols = int((w - 2 * PAD) / CW)
    parts = [window(x, y, w, h, pane.title)]
    row_y = y + BAR + PAD + FS
    for line in pane.lines:
        on = line.at * PLAY
        for row in wrap(line.spans, cols):
            parts.append(f'<text x="{x + PAD}" y="{row_y:.1f}" class="{anim.cls(on)}">{tspans(row)}</text>')
            row_y += LH
    # Elapsed timer: one frame per 1/FPS playback second, the final value held to the loop's end.
    tx, ty = x + w - PAD, y + h - PAD
    steps = int(pane.end * PLAY * FPS)
    for i in range(steps):
        on, off = i / FPS, (i + 1) / FPS
        parts.append(f'<text x="{tx}" y="{ty}" text-anchor="end" fill="{COLORS["dim"]}" class="{anim.cls(on, off)}" opacity="0">'
                     f'{i / FPS / PLAY:.2f}s</text>')
    end = pane.end * PLAY
    parts.append(f'<text x="{tx}" y="{ty}" text-anchor="end" class="{anim.cls(end)}" font-weight="700">'
                 f'<tspan fill="{COLORS["fg"]}">{pane.end:.2f}s </tspan><tspan fill="{COLORS["green"]}">{pane.done_label}</tspan></text>')
    if pane.end * PLAY < total_end:
        return parts
    parts.append(f'<text x="{x + PAD}" y="{ty}" fill="{COLORS["yellow"]}" class="{anim.cls(0, end)}" opacity="0">installing…</text>')
    return parts


def hero(cap_path, out_path):
    cap = json.load(open(cap_path))
    left = git_pane(cap, cap["median_rep"]["git"])
    right = aver_pane(cap, cap["median_rep"]["aver"])
    longest = max(left.end, right.end) * PLAY
    anim = Anim(longest + HOLD)
    pw = 720
    rows = max(sum(len(wrap(l.spans, int((pw - 2 * PAD) / CW))) for l in p.lines) for p in (left, right))
    ph = BAR + 2 * PAD + rows * LH + LH + 8
    W, H = 2 * pw + 3 * PAD, ph + 2 * PAD + 2 * LH
    m = cap["medians"]
    body = [f'<rect width="{W}" height="{H}" rx="10" fill="{COLORS["bg"]}"/>']
    body += render_pane(left, PAD, PAD, pw, ph, anim, longest)
    body += render_pane(right, 2 * PAD + pw, PAD, pw, ph, anim, longest)
    mach = cap["machine"]
    foot = (f'4 workspaces on a {mach["cpu"]}, {mach["filesystem"].upper()}: git worktree add + npm ci {m["git_total_x4_s"]:.2f}s, '
            f'aver workspace new {m["aver_workspace_new_x4_s"]:.2f}s (medians of {cap["reps"]} runs). '
            f'Replayed at 1/{PLAY:g} speed; timers show real time.')
    body.append(f'<text x="{PAD}" y="{H - PAD}" fill="{COLORS["dim"]}" font-size="12">{escape(foot)}</text>')
    write_svg(out_path, W, H, body, anim.css(), "git worktree vs aver workspace new")


def write_svg(out_path, W, H, body, css, label):
    style = (f"text{{font-family:{FONT};font-size:{FS}px;white-space:pre}}\n{css}\n"
             "@media (prefers-reduced-motion:reduce){[class^=k]{animation:none}}")
    svg = (f'<svg xmlns="http://www.w3.org/2000/svg" width="{W:.0f}" height="{H:.0f}" viewBox="0 0 {W:.0f} {H:.0f}" '
           f'role="img" aria-label="{escape(label)}" xml:space="preserve">\n<style>{style}</style>\n' + "\n".join(body) + "\n</svg>\n")
    os.makedirs(os.path.dirname(os.path.abspath(out_path)), exist_ok=True)
    with open(out_path, "w", encoding="utf-8") as f:
        f.write(svg)
    print(f"{out_path}: {len(svg.encode()) / 1024:.1f} KB", file=sys.stderr)


def single(txt_path, out_path, title):
    text = open(txt_path, encoding="utf-8").read().rstrip("\n")
    lines = []
    for raw in text.split("\n"):
        m = re.match(r"^(\S*) ?\$ (.*)$", raw)
        if m and "\x1b" not in raw:
            lines.append(prompt_line(m.group(1) or "~", m.group(2)))
        elif re.match(r"^[+-](?![+-])", raw):
            lines.append([Span(raw, "green" if raw[0] == "+" else "red")])
        else:
            lines.append(parse_ansi(raw))
    cols = min(max((sum(len(s.text) for s in l) for l in lines), default=40), 110)
    w = cols * CW + 2 * PAD
    rows = [r for l in lines for r in wrap(l, cols, words=True)]
    h = BAR + 2 * PAD + len(rows) * LH
    body = [window(0, 0, w, h, title)]
    y = BAR + PAD + FS
    for r in rows:
        body.append(f'<text x="{PAD}" y="{y:.1f}">{tspans(r)}</text>')
        y += LH
    write_svg(out_path, w, h, body, "", title)


def main(argv):
    if argv and argv[0] == "--single":
        if len(argv) < 3:
            sys.exit(__doc__)
        title = argv[argv.index("--title") + 1] if "--title" in argv else os.path.basename(argv[1])
        single(argv[1], argv[2], title)
        return
    root = os.path.dirname(os.path.abspath(__file__))
    cap = argv[0] if argv else os.path.join(root, "hero", "capture.json")
    out = argv[1] if len(argv) > 1 else os.path.join(root, "..", "assets", "hero-comparison.svg")
    hero(cap, os.path.normpath(out))


if __name__ == "__main__":
    main(sys.argv[1:])
