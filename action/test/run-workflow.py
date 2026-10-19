#!/usr/bin/env python3
"""Runs a workflow's jobs as GitHub would for one event, on this machine.

  run-workflow.py <workflow.yml> <result.json>

The event is GITHUB_EVENT_NAME, GITHUB_EVENT_PATH (its payload), GITHUB_REF
and GITHUB_SHA; GITHUB_REPOSITORY names the repository. The workflow's `on:`
filters decide whether it runs at all. Jobs run in `needs` order, one at a
time, each matrix combination in turn, with the github, needs, matrix,
steps, env, runner, vars and secrets contexts (WORKFLOW_SECRETS and
WORKFLOW_VARS, JSON objects). A secret in a job's `if:` is an error, as it
is on GitHub. Each job gets a fresh RUNNER_TEMP directory and checkout, and
github.token is `token-<job id>`, so a mock can tell which job sent a
request. Steps are `run:` steps (bash) and these `uses:`:

  actions/checkout             clones WORKFLOW_REPO at `ref` or GITHUB_SHA
  actions/upload-artifact      copies into $E2E_ARTIFACTS_DIR/<name>
  actions/download-artifact    copies back, merged with merge-multiple
  actions/attest-build-provenance
                               appends its subjects (name, sha256) to
                               $E2E_ARTIFACTS_DIR/../attestations.jsonl;
                               the job needs id-token and attestations write
  actions/setup-node           nothing (the host's Node)
  a key of WORKFLOW_ACTIONS    {"owner/repo/path": dir}: that composite
                               action, run by run-action.py

A job whose `uses:` is ./<workflow> runs that workflow (workflow_call) with
its `with:` as inputs and its secrets inherited or mapped; its jobs are
<job>/<called job> and may not ask for more permissions than the caller.
WORKFLOW_STUB_JOBS ({job: result}) skips running the jobs it names, and
WORKFLOW_CHECK_RUNS (a mock state file) gets each job's check run, named
as GitHub names it.

Any other `uses:` is an error. Writes {triggered, jobs: {<id>: {result,
outputs, permissions, runs: [{matrix, runs-on, conclusion, steps}]}}} to
result.json; exits 1 when a job failed.
"""
import glob
import hashlib
import importlib.util
import itertools
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import time

import yaml

HERE = os.path.dirname(os.path.abspath(__file__))
spec = importlib.util.spec_from_file_location("run_action", os.path.join(HERE, "run-action.py"))
ra = importlib.util.module_from_spec(spec)
spec.loader.exec_module(ra)


class Secrets(dict):
    """A secret the repository does not have reads as '', as on GitHub."""

    def get(self, key, default=None):
        return super().get(key, "")


class Expr(ra.Expr):
    def call(self, name, args):
        if name == "startsWith":
            return ra.text(args[0]).lower().startswith(ra.text(args[1]).lower())
        if name == "endsWith":
            return ra.text(args[0]).lower().endswith(ra.text(args[1]).lower())
        if name == "contains":
            hay = args[0]
            return args[1] in hay if isinstance(hay, list) else ra.text(args[1]).lower() in ra.text(hay).lower()
        if name == "join":
            return (ra.text(args[1]) if len(args) > 1 else ",").join(ra.text(v) for v in args[0]) if isinstance(args[0], list) else ra.text(args[0])
        if name == "fromJSON":
            return json.loads(args[0])
        if name == "toJSON":
            return json.dumps(args[0])
        return super().call(name, args)


def pattern(glob_):
    """GitHub's filter pattern syntax: * ** + ? [..]."""
    out, i = "", 0
    while i < len(glob_):
        c = glob_[i]
        if glob_.startswith("**", i):
            out, i = out + ".*", i + 2
            continue
        if c == "*":
            out += "[^/]*"
        elif c in "+?":
            out += c
        elif c == "[":
            j = glob_.index("]", i)
            out, i = out + glob_[i : j + 1], j + 1
            continue
        else:
            out += re.escape(c)
        i += 1
    return re.compile(out)


def matches(name, patterns):
    hit = False
    for p in patterns:
        negate = p.startswith("!")
        if pattern(p[1:] if negate else p).fullmatch(name):
            hit = not negate
    return hit


def triggered(on, event, ref, payload):
    if isinstance(on, str):
        on = {on: None}
    elif isinstance(on, list):
        on = {e: None for e in on}
    if event not in on:
        return False
    filt = on[event] or {}
    if event == "push":
        if ref.startswith("refs/tags/"):
            return "tags" in filt and matches(ref[len("refs/tags/") :], filt["tags"]) if ("tags" in filt or "branches" in filt) else True
        if ref.startswith("refs/heads/"):
            return "branches" in filt and matches(ref[len("refs/heads/") :], filt["branches"]) if ("tags" in filt or "branches" in filt) else True
    if "types" in filt:
        return payload.get("action") in filt["types"]
    if event in ("pull_request", "pull_request_target") and payload.get("action") not in (None, "opened", "synchronize", "reopened"):
        return False
    return True


def combinations(strategy):
    matrix = (strategy or {}).get("matrix")
    if not matrix:
        return [{}]
    axes = {k: v for k, v in matrix.items() if k not in ("include", "exclude")}
    combos = [dict(zip(axes, values)) for values in itertools.product(*axes.values())] if axes else []
    return combos + list(matrix.get("include", []))


def runner_of(runs_on):
    name = str(runs_on)
    return {"os": "macOS" if name.startswith("macos") else "Linux", "arch": "ARM64" if "arm" in name else "X64"}


def sha256(path):
    with open(path, "rb") as f:
        return hashlib.sha256(f.read()).hexdigest()


def simulate(uses, w, job, workspace, ctx):
    action = uses.split("@")[0]
    artifacts = os.environ["E2E_ARTIFACTS_DIR"]
    if action == "actions/checkout":
        shutil.rmtree(workspace, ignore_errors=True)
        subprocess.run(["git", "clone", "-q", os.environ["WORKFLOW_REPO"], workspace], check=True)
        subprocess.run(["git", "-C", workspace, "-c", "advice.detachedHead=false", "checkout", "-q", w.get("ref") or ctx["github"]["sha"]], check=True)
        return {}, True
    if action == "actions/upload-artifact":
        dst = os.path.join(artifacts, w["name"])
        if os.path.exists(dst):
            print(f"::error::an artifact named {w['name']} already exists")
            return {}, False
        found = [p for raw in ra.paths(w["path"]) for p in glob.glob(os.path.join(workspace, raw))]
        if not found:
            print(f"::error::no files were found with the provided path: {w['path']}")
            return {}, w.get("if-no-files-found", "warn") != "error"
        for p in found:
            ra.copy(p, dst if os.path.isdir(p) else os.path.join(dst, os.path.basename(p)))
        return {}, True
    if action == "actions/download-artifact":
        dst = os.path.join(workspace, w.get("path", "."))
        for name in sorted(os.listdir(artifacts)):
            ra.copy(os.path.join(artifacts, name), dst if ra.truthy(w.get("merge-multiple")) and w.get("merge-multiple") != "false" else os.path.join(dst, name))
        return {}, True
    if action == "actions/attest-build-provenance":
        perms = job.get("permissions") or {}
        if perms.get("id-token") != "write" or perms.get("attestations") != "write":
            print("::error::attest-build-provenance needs id-token: write and attestations: write")
            return {}, False
        if "subject-checksums" in w:
            subjects = []
            for line in open(os.path.join(workspace, w["subject-checksums"])).read().splitlines():
                digest, name = line.split(None, 1)
                subjects.append({"name": name.lstrip("*"), "sha256": digest})
        else:
            subjects = [{"name": os.path.basename(p), "sha256": sha256(p)} for raw in ra.paths(w["subject-path"]) for p in sorted(glob.glob(os.path.join(workspace, raw)))]
        if not subjects:
            print("::error::no subjects to attest")
            return {}, False
        with open(os.path.join(artifacts, "..", "attestations.jsonl"), "a") as f:
            f.write(json.dumps({"job": ctx["job"], "subjects": subjects}) + "\n")
        return {"bundle-path": "", "attestation-id": "1"}, True
    if action == "actions/setup-node":
        return {}, True
    local = json.loads(os.environ.get("WORKFLOW_ACTIONS", "{}")).get(action)
    if local:
        result = os.path.join(ctx["runner"]["temp"], "action-result.json")
        env = dict(os.environ, **ctx["env"], GITHUB_WORKSPACE=workspace, RUNNER_TEMP=ctx["runner"]["temp"], RUNNER_OS=ctx["runner"]["os"], RUNNER_ARCH=ctx["runner"]["arch"],
                   ACTION_REPOSITORY="/".join(action.split("/")[:2]), ACTION_REF=uses.split("@", 1)[1])
        env.setdefault("GH_TOKEN", ctx["github"]["token"])
        code = subprocess.call([sys.executable, os.path.join(HERE, "run-action.py"), local, result, *(f"{k}={v}" for k, v in w.items())], env=env, cwd=workspace)
        outputs = json.load(open(result))["outputs"] if os.path.exists(result) else {}
        return outputs, code == 0
    raise SystemExit(f"run-workflow.py: the step uses {uses}, which the harness does not simulate")


def run_job(job_id, job, matrix, ctx_base, wf_env, root):
    temp = tempfile.mkdtemp(prefix=f"{job_id}.", dir=root)
    workspace = os.path.join(temp, "workspace")
    os.makedirs(workspace)
    ctx = dict(ctx_base, matrix=matrix, steps={}, job=job_id)
    ev = Expr(ctx, failed=False)
    runs_on = ra.interpolate(job.get("runs-on", ""), ev)
    ctx["runner"] = dict(runner_of(runs_on), temp=temp)
    ctx["env"] = {k: ra.interpolate(v, ev) for k, v in {**wf_env, **(job.get("env") or {})}.items()}
    base = {k: v for k, v in os.environ.items() if k not in ("GH_TOKEN", "GITHUB_TOKEN")}
    gh = ctx["github"]
    base.update(GITHUB_WORKSPACE=workspace, GITHUB_REF=gh["ref"], GITHUB_REF_NAME=gh["ref_name"], GITHUB_SHA=gh["sha"], GITHUB_REPOSITORY=gh["repository"],
                GITHUB_EVENT_NAME=gh["event_name"], GITHUB_EVENT_PATH=os.environ.get("GITHUB_EVENT_PATH", ""), GITHUB_JOB=job_id,
                RUNNER_TEMP=temp, RUNNER_OS=ctx["runner"]["os"], RUNNER_ARCH=ctx["runner"]["arch"], CI="true", GITHUB_ACTIONS="true")
    files = {k: os.path.join(temp, k.lower()) for k in ("GITHUB_PATH", "GITHUB_ENV", "GITHUB_STEP_SUMMARY")}
    for path in files.values():
        open(path, "w").close()
    base.update(files)
    log = []
    for n, step in enumerate(job.get("steps", [])):
        name = step.get("name") or step.get("uses") or (step.get("run") or "").splitlines()[0]
        record = {"name": name, "id": step.get("id"), "outcome": "skipped", "outputs": {}}
        log.append(record)
        job_env = ctx["env"]
        if not ra.condition(step.get("if"), ev):
            print(f"::group::{job_id} / {name} (skipped)\n::endgroup::", flush=True)
            if step.get("id"):
                ctx["steps"][step["id"]] = {"outputs": {}, "outcome": "skipped", "conclusion": "skipped"}
            continue
        ctx["env"] = dict(job_env, **{k: ra.interpolate(v, ev) for k, v in (step.get("env") or {}).items()})
        print(f"::group::{job_id} / {name}", flush=True)
        if "uses" in step:
            with_ = {k: ra.interpolate(v, ev) for k, v in (step.get("with") or {}).items()}
            outputs, ok = simulate(step["uses"], with_, job, workspace, ctx)
        else:
            env = dict(base)
            for line in open(files["GITHUB_ENV"]).read().splitlines():
                if "=" in line:
                    k, v = line.split("=", 1)
                    env[k] = v
            env.update(ctx["env"])
            extra = [p for p in open(files["GITHUB_PATH"]).read().splitlines() if p]
            env["PATH"] = os.pathsep.join(list(reversed(extra)) + [base["PATH"]])
            out_file = os.path.join(temp, f"output.{n}")
            open(out_file, "w").close()
            env["GITHUB_OUTPUT"] = out_file
            script = os.path.join(temp, f"step.{n}.sh")
            with open(script, "w") as f:
                f.write(ra.interpolate(step["run"], ev))
            shell = ["bash", "--noprofile", "--norc", "-eo", "pipefail"] if step.get("shell") == "bash" else ["bash", "-e"]
            if step.get("shell") not in (None, "bash"):
                raise SystemExit(f"run-workflow.py: step {name!r} has shell {step['shell']!r}")
            cwd = os.path.join(workspace, ra.interpolate(step.get("working-directory", "."), ev))
            code = subprocess.call(shell + [script], env=env, cwd=cwd)
            sys.stdout.flush()
            outputs, ok = ra.read_outputs(out_file), code == 0
            if not ok:
                print(f"::error::Process completed with exit code {code}.", flush=True)
                record["exit"] = code
        print("::endgroup::", flush=True)
        ctx["env"] = job_env
        record["outcome"] = "success" if ok else "failure"
        record["outputs"] = outputs
        conclusion = "success" if ok or step.get("continue-on-error") else "failure"
        if step.get("id"):
            ctx["steps"][step["id"]] = {"outputs": outputs, "outcome": record["outcome"], "conclusion": conclusion}
        ev.failed = ev.failed or conclusion == "failure"
    outputs = {k: ra.interpolate(v, ev) for k, v in (job.get("outputs") or {}).items()}
    return {"matrix": matrix, "runs-on": runs_on, "conclusion": "failure" if ev.failed else "success", "steps": log}, outputs


LEVELS = {"none": 0, "read": 1, "write": 2}


def exceeds(inner, outer):
    """The scopes a called workflow's job asks for beyond its caller's."""
    if not isinstance(outer, dict) or not isinstance(inner, dict):
        return []
    return [k for k, v in inner.items() if LEVELS.get(v, 0) > LEVELS.get(outer.get(k, "none"), 0)]


def record_check_run(name, sha, conclusion):
    """GitHub gives every job a check run on the commit; WORKFLOW_CHECK_RUNS
    is the mock's state file, whose check_runs list them."""
    path = os.environ.get("WORKFLOW_CHECK_RUNS")
    if not path:
        return
    state = json.load(open(path))
    state.setdefault("check_runs", {}).setdefault(sha, []).append(
        {"name": name, "head_sha": sha, "status": "completed", "conclusion": conclusion, "app": {"slug": "github-actions"}})
    tmp = path + ".run-workflow"
    json.dump(state, open(tmp, "w"))
    os.replace(tmp, path)


def run_jobs(wf, wf_path, github, secrets, inputs, done, prefix="", caller_perms=None):
    """Runs wf's jobs into done, keyed <prefix><job id>. A job whose `uses:`
    names a workflow of this repository runs that workflow's jobs under
    <job id>/, with its `with:` as inputs. WORKFLOW_STUB_JOBS ({job id:
    result}) stands in for jobs too heavy to run here."""
    stubs = json.loads(os.environ.get("WORKFLOW_STUB_JOBS", "{}"))
    jobs, mine = wf["jobs"], {}
    root = tempfile.mkdtemp(prefix="workflow.", dir=os.environ.get("RUNNER_TEMP") or None)
    while len(mine) < len(jobs):
        ready = [j for j in jobs if j not in mine and all(n in mine for n in as_list(jobs[j].get("needs")))]
        if not ready:
            raise SystemExit("run-workflow.py: the jobs' needs form a cycle")
        job_id = ready[0]
        key = prefix + job_id
        job = jobs[job_id]
        needs = {n: mine[n] for n in as_list(job.get("needs"))}
        raw_if = job.get("if")
        if raw_if is not None and "secrets." in str(raw_if):
            raise SystemExit(f"run-workflow.py: job {key}'s if: reads secrets, which GitHub rejects")
        perms = job.get("permissions", wf.get("permissions", caller_perms))
        if caller_perms is not None and exceeds(perms, caller_perms):
            raise SystemExit(f"run-workflow.py: called job {key} asks for {', '.join(exceeds(perms, caller_perms))} beyond its caller's permissions")
        job_github = dict(github, token=f"token-{key}")
        ctx = {"github": job_github, "needs": {n: {"result": d["result"], "outputs": d["outputs"]} for n, d in needs.items()},
               "vars": json.loads(os.environ.get("WORKFLOW_VARS", "{}")), "inputs": inputs}
        ev = Expr(dict(ctx, env={}), failed=any(d["result"] != "success" for d in needs.values()))
        check = prefix.replace("/", " / ") + (job.get("name") or job_id)
        record = {"result": "skipped", "outputs": {}, "permissions": perms, "runs": []}
        mine[job_id] = done[key] = record
        if key in stubs:
            record["result"] = stubs[key]
            print(f"== {key}: {stubs[key]} (stubbed)", flush=True)
            record_check_run(check, github["sha"], stubs[key])
            continue
        if not ra.condition(raw_if, ev):
            print(f"== {key}: skipped", flush=True)
            record_check_run(check, github["sha"], "skipped")
            continue
        ctx["secrets"] = Secrets(secrets, GITHUB_TOKEN=job_github["token"])
        started = time.monotonic()
        if "uses" in job:
            uses = job["uses"]
            if not uses.startswith("./"):
                raise SystemExit(f"run-workflow.py: job {key} calls {uses}, which the harness does not simulate")
            called_path = os.path.join(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(wf_path)))), uses[2:])
            called = yaml.safe_load(open(called_path))
            spec_ = (called.get("on", called.get(True)) or {})
            spec_ = spec_.get("workflow_call") if isinstance(spec_, dict) and "workflow_call" in spec_ else None
            if spec_ is None and "workflow_call" not in str(called.get("on", called.get(True))):
                raise SystemExit(f"run-workflow.py: {uses} has no workflow_call trigger")
            with_ = {k: ra.interpolate(v, ev) for k, v in (job.get("with") or {}).items()}
            declared = (spec_ or {}).get("inputs") or {}
            unknown = set(with_) - set(declared)
            if unknown:
                raise SystemExit(f"run-workflow.py: job {key} passes inputs {sorted(unknown)} that {uses} does not declare")
            called_inputs = {k: with_.get(k, d.get("default", "")) for k, d in declared.items()}
            missing = [k for k, d in declared.items() if d.get("required") and k not in with_]
            if missing:
                raise SystemExit(f"run-workflow.py: job {key} does not pass required inputs {missing}")
            passed_secrets = secrets if job.get("secrets") == "inherit" else {k: ra.interpolate(v, Expr(dict(ctx, env={}), failed=False)) for k, v in (job.get("secrets") or {}).items()}
            print(f"== {key}: calls {uses}", flush=True)
            inner = {}
            run_jobs(called, called_path, github, passed_secrets, called_inputs, inner, prefix=key + "/", caller_perms=perms or {})
            done.update(inner)
            record["result"] = "failure" if any(d["result"] == "failure" for d in inner.values()) else "success"
        else:
            for matrix in combinations(job.get("strategy")):
                print(f"== {key} {json.dumps(matrix) if matrix else ''}", flush=True)
                run, outputs = run_job(key, job, matrix, ctx, wf.get("env") or {}, root)
                record["runs"].append(run)
                record["outputs"].update(outputs)
            record["result"] = "failure" if any(r["conclusion"] == "failure" for r in record["runs"]) else "success"
            record_check_run(check, github["sha"], record["result"])
        print(f"== {key}: {record['result']} ({time.monotonic() - started:.1f}s)", flush=True)


def main():
    wf_path, result_path = sys.argv[1], sys.argv[2]
    wf = yaml.safe_load(open(wf_path))
    on = wf.get("on", wf.get(True))
    event = os.environ["GITHUB_EVENT_NAME"]
    ref = os.environ.get("GITHUB_REF", "")
    payload = json.load(open(os.environ["GITHUB_EVENT_PATH"])) if os.environ.get("GITHUB_EVENT_PATH") else {}
    result = {"triggered": triggered(on, event, ref, payload), "jobs": {}}
    if not result["triggered"]:
        json.dump(result, open(result_path, "w"), indent=2)
        print(f"{os.path.basename(wf_path)} does not run on {event} {ref}")
        return
    github = {
        "event_name": event,
        "event": payload,
        "ref": ref,
        "ref_name": re.sub(r"^refs/(heads|tags)/", "", ref),
        "sha": os.environ["GITHUB_SHA"],
        "repository": os.environ["GITHUB_REPOSITORY"],
        "repository_owner": os.environ["GITHUB_REPOSITORY"].split("/")[0],
        "server_url": "https://github.com",
        "run_id": "1",
    }
    done = {}
    run_jobs(wf, wf_path, github, dict(json.loads(os.environ.get("WORKFLOW_SECRETS", "{}"))), {}, done)
    result["jobs"] = done
    json.dump(result, open(result_path, "w"), indent=2)
    failed = [j for j, d in done.items() if d["result"] == "failure"]
    print(f"Workflow {'failure: ' + ', '.join(failed) if failed else 'success'}")
    sys.exit(1 if failed else 0)


def as_list(v):
    return [] if v is None else [v] if isinstance(v, str) else list(v)


if __name__ == "__main__":
    main()
