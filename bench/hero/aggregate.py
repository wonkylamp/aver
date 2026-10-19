"""Fold capture.sh's step and disk records into capture.json (stdout)."""
import hashlib, json, os, re, statistics, subprocess, sys, time

root, aver, image, docker_ok = sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4] == "1"
steps = [json.loads(l) for l in open(os.path.join(root, "steps.jsonl"))]
disk = [json.loads(l) for l in open(os.path.join(root, "disk.jsonl"))]
reps = sorted({s["rep"] for s in steps})

def sh(cmd):
    try:
        return subprocess.run(cmd, shell=True, capture_output=True, text=True).stdout.strip()
    except Exception:
        return ""

def med(xs):
    return round(statistics.median(xs), 4) if xs else None

by = {}
for s in steps:
    by.setdefault((s["side"], s["name"]), []).append(s)

summary = {}
for (side, name), rs in by.items():
    summary[f"{side}/{name}"] = {
        "cmd": rs[0]["cmd"],
        "exit": [r["exit"] for r in rs],
        "wall_s": [r["wall_s"] for r in rs],
        "median_s": med([r["wall_s"] for r in rs]),
    }

def per_rep_sum(side, prefix):
    out = []
    for rep in reps:
        xs = [s["wall_s"] for s in steps if s["side"] == side and s["rep"] == rep and s["name"].startswith(prefix)]
        if xs:
            out.append(round(sum(xs), 4))
    return out

ws_dir_ms = []
for s in steps:
    if s["name"] == "workspace-new-4":
        ws_dir_ms.append([float(m) for m in re.findall(r" in (\d+(?:\.\d+)?) ms \(", s["output"])])

derived = {
    "git_worktree_add_x4_s": per_rep_sum("git", "worktree-add-"),
    "git_npm_ci_x4_s": per_rep_sum("git", "npm-ci-"),
    "aver_workspace_new_x4_s": [s["wall_s"] for s in steps if s["name"] == "workspace-new-4"],
    "aver_workspace_new_per_dir_ms": ws_dir_ms,
    "docker_container_x4_s": per_rep_sum("docker", "container-"),
}
derived["git_total_x4_s"] = [round(a + b, 4) for a, b in zip(derived["git_worktree_add_x4_s"], derived["git_npm_ci_x4_s"])]
medians = {k: med(v) for k, v in derived.items() if v and not isinstance(v[0], list)}
medians["aver_workspace_new_per_dir_ms"] = med([x for xs in ws_dir_ms for x in xs])

disk_out = {}
for d in disk:
    disk_out.setdefault(d["name"], []).append(d["before"] - d["after"])
disk_summary = {k: {"delta_bytes": v, "median_mb": round(statistics.median(v) / 1e6, 2)} for k, v in disk_out.items()}

runs = {str(rep): [{"side": s["side"], "name": s["name"], "cwd": s["cwd"], "cmd": s["cmd"],
                    "exit": s["exit"], "wall_s": s["wall_s"], "output": s["output"]}
                   for s in steps if s["rep"] == rep] for rep in reps}

def median_rep(xs):
    m = statistics.median_low(xs)
    return str(reps[xs.index(m)])

st = os.stat(aver)
machine = {
    "model": sh("sysctl -n hw.model"),
    "cpu": sh("sysctl -n machdep.cpu.brand_string"),
    "ncpu": sh("sysctl -n hw.ncpu"),
    "memory_gb": round(int(sh("sysctl -n hw.memsize") or 0) / 2**30, 1),
    "os": sh("sw_vers -productName") + " " + sh("sw_vers -productVersion") + " (" + sh("sw_vers -buildVersion") + ")",
    "filesystem": sh("diskutil info / | awk -F: '/Type \\(Bundle\\)/{gsub(/ /,\"\",$2);print $2}'"),
    "node": sh("node --version"), "npm": sh("npm --version"), "git": sh("git --version"),
    "docker": sh("docker version --format '{{.Server.Version}}'") if docker_ok else None,
}
out = {
    "captured_at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
    "aver": {"path": aver, "version": sh(f"{aver} --version"),
             "mtime": time.strftime("%Y-%m-%dT%H:%M:%S%z", time.localtime(st.st_mtime)),
             "sha256": hashlib.sha256(open(aver, "rb").read()).hexdigest()},
    "machine": machine,
    "reps": len(reps),
    "docker": {"measured": docker_ok, "image": image if docker_ok else None,
               "note": "npm ci inside each container fetches from registry.npmjs.org" if docker_ok else "not measured"},
    "notes": [
        "Disk deltas are free-space deltas on / (statvfs after sync), not du; other processes on the machine add noise.",
        "git side: npm ci --offline from a warm local npm cache, sequential over 4 worktrees.",
        "aver side: the timed `aver workspace new --count 4` is the second invocation after the first imported the commit.",
        "runs holds every rep's verbatim terminal text; median_rep names the rep whose total is the (low) median, which the SVG replays.",
        "Commands ran outside any agent environment (CLAUDECODE and AVER_* unset).",
    ],
    "medians": medians,
    "derived": derived,
    "steps": summary,
    "disk": disk_summary,
    "median_rep": {"git": median_rep(derived["git_total_x4_s"]),
                   "aver": median_rep(derived["aver_workspace_new_x4_s"])},
    "runs": runs,
}
json.dump(out, sys.stdout, indent=1)
