"""Run one shell command, time it and append a JSON record to a JSONL file.

usage: step.py <jsonl> <rep> <side> <name> <cwd> <command>
       step.py free            prints free bytes on / after sync
"""
import json, os, subprocess, sys, time

def free():
    os.sync()
    s = os.statvfs("/")
    return s.f_bavail * s.f_frsize

if sys.argv[1] == "free":
    print(free())
    sys.exit(0)

out, rep, side, name, cwd, cmd = sys.argv[1:7]
t0 = time.perf_counter()
p = subprocess.run(["/bin/sh", "-c", cmd], cwd=cwd, stdout=subprocess.PIPE,
                   stderr=subprocess.STDOUT)
wall = time.perf_counter() - t0
text = p.stdout.decode("utf-8", "replace")
with open(out, "a") as f:
    f.write(json.dumps({"rep": int(rep), "side": side, "name": name, "cwd": cwd,
                        "cmd": cmd, "exit": p.returncode, "wall_s": round(wall, 4),
                        "output": text}) + "\n")
sys.stdout.write(text)
print(f"[{side}/{name}] exit={p.returncode} {wall:.3f}s", file=sys.stderr)
