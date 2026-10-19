#!/usr/bin/env python3
"""Runs a composite action's steps from its action.yml, as a GitHub runner
would, on this machine.

  run-action.py <action dir> <result.json> [input=value ...]

Each step's `if:`, `env:` and `with:` expressions are evaluated against the
inputs, the earlier steps' outputs and the github and runner contexts
(GITHUB_EVENT_NAME, GITHUB_EVENT_PATH, GITHUB_REF, GITHUB_REPOSITORY,
RUNNER_TEMP, RUNNER_OS, RUNNER_ARCH, ACTION_REPOSITORY, ACTION_REF and
GH_TOKEN from the environment). `shell: bash` steps run as on GitHub;
`uses:` steps are simulated for actions/cache restore and save (under
$E2E_CACHE_DIR) and actions/upload-artifact (under $E2E_ARTIFACTS_DIR), and
any other `uses:` is an error, so the harness breaks when action.yml grows a
step it cannot run. Writes {steps, outputs, conclusion} to result.json and
exits 1 when the job failed.
"""
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile

import yaml


class Expr:
    """GitHub's expression syntax, as far as action.yml uses it."""

    TOKEN = re.compile(r"\s*(?:(?P<str>'(?:[^']|'')*')|(?P<num>\d+)|(?P<op>==|!=|&&|\|\||[!(),])|(?P<id>[A-Za-z_][A-Za-z0-9_.\-*]*))")
    STATUS = ("always", "success", "failure", "cancelled")

    def __init__(self, ctx, failed):
        self.ctx, self.failed = ctx, failed

    def eval(self, text):
        self.toks, pos = [], 0
        text = text.strip()
        while pos < len(text):
            m = self.TOKEN.match(text, pos)
            if not m or m.end() == pos:
                raise SyntaxError(f"cannot parse expression at {text[pos:]!r}")
            kind = m.lastgroup
            self.toks.append((kind, m.group(kind)))
            pos = m.end()
        self.i = 0
        value = self.or_()
        if self.i != len(self.toks):
            raise SyntaxError(f"trailing tokens in {text!r}")
        return value

    def peek(self):
        return self.toks[self.i] if self.i < len(self.toks) else (None, None)

    def take(self, value=None):
        tok = self.peek()
        if value is not None and tok[1] != value:
            raise SyntaxError(f"expected {value}, got {tok[1]}")
        self.i += 1
        return tok

    def or_(self):
        left = self.and_()
        while self.peek()[1] == "||":
            self.take()
            right = self.and_()
            left = left if truthy(left) else right
        return left

    def and_(self):
        left = self.cmp()
        while self.peek()[1] == "&&":
            self.take()
            right = self.cmp()
            left = right if truthy(left) else left
        return left

    def cmp(self):
        left = self.unary()
        while self.peek()[1] in ("==", "!="):
            op = self.take()[1]
            right = self.unary()
            same = loose(left) == loose(right)
            left = same if op == "==" else not same
        return left

    def unary(self):
        if self.peek()[1] == "!":
            self.take()
            return not truthy(self.unary())
        return self.primary()

    def primary(self):
        kind, value = self.take()
        if value == "(":
            inner = self.or_()
            self.take(")")
            return inner
        if kind == "str":
            return value[1:-1].replace("''", "'")
        if kind == "num":
            return int(value)
        if kind != "id":
            raise SyntaxError(f"unexpected {value}")
        if value in ("true", "false"):
            return value == "true"
        if value == "null":
            return None
        if self.peek()[1] == "(":
            self.take()
            args = []
            while self.peek()[1] != ")":
                args.append(self.or_())
                if self.peek()[1] == ",":
                    self.take()
            self.take(")")
            return self.call(value, args)
        # `a.*.b` is an object filter: b of each of a's values, as a list.
        nodes, spread = [self.ctx], False
        for part in value.split("."):
            if part == "*":
                nodes, spread = [v for n in nodes if isinstance(n, dict) for v in n.values()], True
            else:
                nodes = [n.get(part) if isinstance(n, dict) else None for n in nodes]
        return nodes if spread else nodes[0]

    def call(self, name, args):
        if name == "always":
            return True
        if name == "success":
            return not self.failed
        if name == "failure":
            return self.failed
        if name == "cancelled":
            return False
        if name == "format":
            return re.sub(r"\{(\d+)\}", lambda m: text(args[1 + int(m.group(1))]), args[0])
        raise SyntaxError(f"function {name}() is not simulated")


def truthy(v):
    return v not in (None, False, "", 0)


def loose(v):
    return v.lower() if isinstance(v, str) else v


def text(v):
    if v is None:
        return ""
    if isinstance(v, bool):
        return "true" if v else "false"
    return v if isinstance(v, str) else json.dumps(v)


def interpolate(value, ev):
    return re.sub(r"\$\{\{(.*?)\}\}", lambda m: text(ev.eval(m.group(1))), str(value))


def condition(raw, ev):
    if raw is None:
        return not ev.failed
    expr = str(raw)
    m = re.fullmatch(r"\s*\$\{\{(.*)\}\}\s*", expr, re.S)
    if m:
        expr = m.group(1)
    if not any(re.search(rf"\b{f}\(", expr) for f in Expr.STATUS):
        expr = f"success() && ({expr})"
    return truthy(ev.eval(expr))


def read_outputs(path):
    outputs, lines = {}, open(path).read().splitlines()
    i = 0
    while i < len(lines):
        line = lines[i]
        m = re.fullmatch(r"([^=<]+)<<(.+)", line)
        if m:
            end = lines.index(m.group(2), i + 1)
            outputs[m.group(1)] = "\n".join(lines[i + 1 : end])
            i = end + 1
            continue
        if "=" in line:
            k, v = line.split("=", 1)
            outputs[k] = v
        i += 1
    return outputs


def paths(raw):
    return [os.path.expanduser(p.strip()) for p in str(raw).splitlines() if p.strip()]


def copy(src, dst):
    if os.path.isdir(src):
        shutil.copytree(src, dst, symlinks=True, dirs_exist_ok=True)
    elif os.path.exists(src):
        os.makedirs(os.path.dirname(dst), exist_ok=True)
        shutil.copy2(src, dst)


def simulate(uses, w):
    """The `uses:` steps action.yml has, by what they do to the filesystem."""
    action = uses.split("@")[0]
    cache = os.environ.get("E2E_CACHE_DIR", "")
    if action == "actions/cache/restore":
        keys = [w["key"]] + [k for k in str(w.get("restore-keys", "")).splitlines() if k.strip()]
        for n, key in enumerate(keys):
            hits = sorted(k for k in os.listdir(cache) if (k == key if n == 0 else k.startswith(key.strip())))
            if hits:
                for j, p in enumerate(paths(w["path"])):
                    copy(os.path.join(cache, hits[-1], str(j)), p)
                print(f"Cache restored from key: {hits[-1]}")
                return {"cache-hit": "true" if n == 0 else "false", "cache-matched-key": hits[-1]}
        print(f"Cache not found for input keys: {', '.join(keys)}")
        return {"cache-hit": "false"}
    if action == "actions/cache/save":
        dst = os.path.join(cache, w["key"])
        if os.path.exists(dst):
            print(f"::warning::Cache with key {w['key']} already exists, not saving")
            return {}
        for j, p in enumerate(paths(w["path"])):
            copy(p, os.path.join(dst, str(j)))
        print(f"Cache saved with key: {w['key']}")
        return {}
    if action == "actions/upload-artifact":
        dst = os.path.join(os.environ["E2E_ARTIFACTS_DIR"], w["name"])
        for p in paths(w["path"]):
            copy(p, dst)
        return {}
    raise SystemExit(f"run-action.py: the step uses {uses}, which the harness does not simulate")


def main():
    action_dir, result_path = sys.argv[1], sys.argv[2]
    given = dict(a.split("=", 1) for a in sys.argv[3:])
    spec = yaml.safe_load(open(os.path.join(action_dir, "action.yml")))
    unknown = set(given) - set(spec["inputs"])
    if unknown:
        raise SystemExit(f"run-action.py: action.yml has no input {sorted(unknown)}")
    env = dict(os.environ)
    event_path = env.get("GITHUB_EVENT_PATH")
    event = json.load(open(event_path)) if event_path else {}
    ctx = {
        "github": {
            "event_name": env.get("GITHUB_EVENT_NAME", ""),
            "event": event,
            "ref": env.get("GITHUB_REF", ""),
            "repository": env.get("GITHUB_REPOSITORY", ""),
            "token": env.get("GH_TOKEN", ""),
            "action_repository": env.get("ACTION_REPOSITORY", ""),
            "action_ref": env.get("ACTION_REF", ""),
        },
        "runner": {"temp": env["RUNNER_TEMP"], "os": env.get("RUNNER_OS", ""), "arch": env.get("RUNNER_ARCH", "")},
        "steps": {},
        "inputs": {},
    }
    ev = Expr(ctx, failed=False)
    for name, decl in spec["inputs"].items():
        ctx["inputs"][name] = given.get(name, interpolate(decl.get("default", ""), ev))

    env.pop("GH_TOKEN", None)
    env["GITHUB_ACTION_PATH"] = action_dir
    env.setdefault("GITHUB_ACTIONS", "true")
    env.setdefault("CI", "true")
    scratch = tempfile.mkdtemp(dir=env["RUNNER_TEMP"])
    env["GITHUB_PATH"] = os.path.join(scratch, "path")
    open(env["GITHUB_PATH"], "w").close()
    log = []
    for n, step in enumerate(spec["runs"]["steps"]):
        name = step.get("name", f"step {n}")
        record = {"name": name, "id": step.get("id"), "outcome": "skipped", "outputs": {}}
        log.append(record)
        if not condition(step.get("if"), ev):
            print(f"::group::{name} (skipped)\n::endgroup::", flush=True)
            if step.get("id"):
                ctx["steps"][step["id"]] = {"outputs": {}, "outcome": "skipped"}
            continue
        print(f"::group::{name}", flush=True)
        if "uses" in step:
            with_ = {k: interpolate(v, ev) for k, v in (step.get("with") or {}).items()}
            outputs, ok = simulate(step["uses"], with_), True
        else:
            if step.get("shell") != "bash":
                raise SystemExit(f"run-action.py: step {name!r} has shell {step.get('shell')!r}")
            step_env = dict(env)
            step_env.update({k: interpolate(v, ev) for k, v in (step.get("env") or {}).items()})
            extra = [p for p in open(env["GITHUB_PATH"]).read().splitlines() if p]
            step_env["PATH"] = os.pathsep.join(list(reversed(extra)) + [env["PATH"]])
            out_file = os.path.join(scratch, f"output.{n}")
            open(out_file, "w").close()
            step_env["GITHUB_OUTPUT"] = out_file
            script = os.path.join(scratch, f"step.{n}.sh")
            with open(script, "w") as f:
                f.write(interpolate(step["run"], ev))
            code = subprocess.call(
                ["bash", "--noprofile", "--norc", "-eo", "pipefail", script],
                env=step_env,
                cwd=env.get("GITHUB_WORKSPACE") or os.getcwd(),
            )
            sys.stdout.flush()
            outputs, ok = read_outputs(out_file), code == 0
            if not ok:
                print(f"::error::Process completed with exit code {code}.", flush=True)
                record["exit"] = code
        print("::endgroup::", flush=True)
        record["outcome"] = "success" if ok else "failure"
        record["outputs"] = outputs
        if step.get("id"):
            ctx["steps"][step["id"]] = {"outputs": outputs, "outcome": record["outcome"]}
        ev.failed = ev.failed or not ok
    outputs = {k: text(ev.eval(re.fullmatch(r"\s*\$\{\{(.*)\}\}\s*", v["value"], re.S).group(1))) for k, v in spec.get("outputs", {}).items()}
    conclusion = "failure" if ev.failed else "success"
    json.dump({"steps": log, "outputs": outputs, "conclusion": conclusion}, open(result_path, "w"), indent=2)
    print(f"Job {conclusion}")
    sys.exit(1 if ev.failed else 0)


if __name__ == "__main__":
    main()
