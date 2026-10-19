#!/usr/bin/env python3
"""A local stand-in for the parts of GitHub the action and release.yml use.

  mock-github.py serve <dir>   Serves the REST calls below and the files
                               under <dir>/www on 127.0.0.1, writing the
                               port to <dir>/port and every request (method,
                               path, token, JSON body) as a line of
                               <dir>/requests.jsonl. <dir>/state.json is read
                               on every request: {"pulls": {"<n>": {"head":
                               {"sha": ...}}}, "reviews": {"<n>": [...]},
                               "check_runs": {"<sha>": [...]}, "git":
                               "<bare repository>", "fail_asset_downloads":
                               bool}.
  mock-github.py gh <args>     The gh shim: `gh api`, `gh workflow run` and
                               the `gh release` subcommands release.yml's
                               scripts run, sent to
                               the server named by MOCK_GITHUB_URL, since gh
                               itself cannot target a plain-HTTP host.

Git refs, tag objects and compare read and write the bare repository
named by "git". A workflow dispatch is only logged.
Releases live in memory; publishing one writes its assets where GitHub's
download URLs would serve them, www/<owner>/<repo>/releases/download/<tag>/
and, for the latest release, www/<owner>/<repo>/releases/latest/download/.
"""
import fnmatch
import http.server
import json
import os
import re
import shutil
import subprocess
import sys
import threading
import urllib.error
import urllib.parse
import urllib.request


def git(repo, *args):
    return subprocess.run(["git", "-C", repo, *args], capture_output=True, text=True)


def serve(root):
    lock = threading.Lock()
    comments = {}  # issue number -> list of comments
    releases = []  # {"id", "tag_name", "draft", "prerelease", "assets": [{"id", "name", "data"}]}
    pulls = []
    ids = iter(range(1000, 10**9))

    def public(release):
        return {k: v for k, v in release.items() if k != "assets"} | {
            "assets": [{"id": a["id"], "name": a["name"], "size": len(a["data"])} for a in release["assets"]]
        }

    def publish_files(owner_repo, release):
        base = os.path.join(root, "www", owner_repo, "releases")
        dirs = [os.path.join(base, "download", release["tag_name"])]
        if not release["prerelease"] and release.get("make_latest", "true") != "false":
            dirs.append(os.path.join(base, "latest", "download"))
        for d in dirs:
            shutil.rmtree(d, ignore_errors=True)
            os.makedirs(d)
            for a in release["assets"]:
                with open(os.path.join(d, a["name"]), "wb") as f:
                    f.write(a["data"])

    class Handler(http.server.BaseHTTPRequestHandler):
        def log_message(self, *args):
            pass

        def reply(self, status, payload):
            data = payload if isinstance(payload, bytes) else json.dumps(payload).encode()
            self.send_response(status)
            self.send_header("Content-Type", "application/octet-stream" if isinstance(payload, bytes) else "application/json")
            self.send_header("Content-Length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)

        def handle_any(self):
            length = int(self.headers.get("Content-Length") or 0)
            raw = self.rfile.read(length) if length else b""
            binary = self.headers.get("Content-Type") == "application/octet-stream"
            body = raw if binary else (json.loads(raw) if raw else None)
            split = urllib.parse.urlsplit(self.path)
            token = (self.headers.get("Authorization") or "").removeprefix("token ")
            with lock:
                with open(os.path.join(root, "requests.jsonl"), "a") as log:
                    logged = {"bytes": len(raw)} if binary else body
                    log.write(json.dumps({"method": self.command, "path": self.path, "token": token, "body": logged}) + "\n")
                with open(os.path.join(root, "state.json")) as f:
                    state = json.load(f)
                self.route(self.command, split.path, urllib.parse.parse_qs(split.query), body, state, token)

        def route(self, method, path, query, body, state, token):
            www = os.path.join(root, "www")
            local = os.path.realpath(os.path.join(www, path.lstrip("/")))
            if method == "GET" and local.startswith(www + "/") and os.path.isfile(local):
                with open(local, "rb") as f:
                    data = f.read()
                self.send_response(200)
                self.send_header("Content-Length", str(len(data)))
                self.end_headers()
                self.wfile.write(data)
                return
            m = re.fullmatch(r"/repos/([^/]+/[^/]+)/(.*)", path)
            owner_repo, rest = (m.group(1), m.group(2)) if m else ("", "")
            repo = state.get("git", "")
            if method == "GET" and (m2 := re.fullmatch(r"pulls/(\d+)", rest)):
                pull = state.get("pulls", {}).get(m2.group(1))
                return self.reply(200, pull) if pull else self.reply(404, {"message": "Not Found"})
            if method == "GET" and (m2 := re.fullmatch(r"pulls/(\d+)/reviews", rest)):
                return self.reply(200, state.get("reviews", {}).get(m2.group(1), []))
            if method == "GET" and rest == "pulls":
                head = query.get("head", [""])[0]
                return self.reply(200, [p for p in pulls if not head or f"{owner_repo.split('/')[0]}:{p['head']['ref']}" == head])
            if method == "POST" and rest == "pulls":
                if git(repo, "rev-parse", "--verify", "-q", f"refs/heads/{body['head']}").returncode != 0:
                    return self.reply(422, {"message": "Validation Failed: head does not exist"})
                pull = {"number": next(ids), "title": body["title"], "body": body.get("body"), "base": {"ref": body["base"]}, "head": {"ref": body["head"]}}
                pull["html_url"] = f"https://github.com/{owner_repo}/pull/{pull['number']}"
                pulls.append(pull)
                return self.reply(201, pull)
            if method == "GET" and (m2 := re.fullmatch(r"issues/(\d+)/comments", rest)):
                return self.reply(200, comments.get(m2.group(1), []))
            if method == "POST" and (m2 := re.fullmatch(r"issues/(\d+)/comments", rest)):
                comment = {"id": next(ids), "body": body["body"], "user": {"login": "github-actions[bot]", "type": "Bot"}}
                comments.setdefault(m2.group(1), []).append(comment)
                return self.reply(201, comment)
            if method == "PATCH" and (m2 := re.fullmatch(r"issues/comments/(\d+)", rest)):
                for comment in (c for cs in comments.values() for c in cs):
                    if comment["id"] == int(m2.group(1)):
                        comment["body"] = body["body"]
                        return self.reply(200, comment)
                return self.reply(404, {"message": "Not Found"})
            if method == "POST" and rest == "check-runs":
                return self.reply(201, {"id": next(ids), **body})
            if method == "GET" and (m2 := re.fullmatch(r"commits/([0-9a-f]{40})/check-runs", rest)):
                runs = state.get("check_runs", {}).get(m2.group(1), [])
                name = query.get("check_name", [None])[0]
                runs = [r for r in runs if name is None or r["name"] == name]
                return self.reply(200, {"total_count": len(runs), "check_runs": runs})
            if method == "GET" and (m2 := re.fullmatch(r"compare/([^.]+)\.\.\.(.+)", rest)):
                base, head = (git(repo, "rev-parse", "--verify", "-q", f"{r}^{{commit}}").stdout.strip() for r in m2.groups())
                if not base or not head:
                    return self.reply(404, {"message": "Not Found"})
                ancestor = lambda a, b: git(repo, "merge-base", "--is-ancestor", a, b).returncode == 0
                status = "identical" if base == head else "behind" if ancestor(head, base) else "ahead" if ancestor(base, head) else "diverged"
                return self.reply(200, {"status": status})
            if method == "GET" and (m2 := re.fullmatch(r"git/ref/(tags/.+)", rest)):
                sha = git(repo, "rev-parse", "--verify", "-q", f"refs/{m2.group(1)}").stdout.strip()
                return self.reply(200, {"ref": f"refs/{m2.group(1)}", "object": {"sha": sha}}) if sha else self.reply(404, {"message": "Not Found"})
            if method == "POST" and rest == "git/tags":
                obj = (f"object {body['object']}\ntype {body['type']}\ntag {body['tag']}\n"
                       f"tagger {body.get('tagger', {}).get('name', 'github-actions[bot]')} <{body.get('tagger', {}).get('email', 'bot@example.invalid')}> 0 +0000\n\n{body['message']}\n")
                made = subprocess.run(["git", "-C", repo, "mktag"], input=obj, capture_output=True, text=True)
                if made.returncode:
                    return self.reply(422, {"message": made.stderr.strip()})
                return self.reply(201, {"sha": made.stdout.strip(), "tag": body["tag"], "object": {"sha": body["object"], "type": body["type"]}})
            if method == "POST" and re.fullmatch(r"actions/workflows/[^/]+/dispatches", rest):
                return self.reply(204, b"")
            if method == "POST" and rest == "git/refs":
                if git(repo, "rev-parse", "--verify", "-q", body["ref"]).returncode == 0:
                    return self.reply(422, {"message": "Reference already exists"})
                git(repo, "update-ref", body["ref"], body["sha"])
                return self.reply(201, {"ref": body["ref"], "object": {"sha": body["sha"]}})
            if method == "PATCH" and (m2 := re.fullmatch(r"git/refs/(tags/.+)", rest)):
                ref = f"refs/{m2.group(1)}"
                if git(repo, "rev-parse", "--verify", "-q", ref).returncode != 0:
                    return self.reply(422, {"message": "Reference does not exist"})
                git(repo, "update-ref", ref, body["sha"])
                return self.reply(200, {"ref": ref, "object": {"sha": body["sha"]}})
            if method == "GET" and rest == "releases":
                # Drafts are visible only to a token with push access.
                return self.reply(200, [public(r) for r in reversed(releases) if token or not r["draft"]])
            if method == "GET" and (m2 := re.fullmatch(r"releases/tags/(.+)", rest)):
                found = [r for r in releases if r["tag_name"] == m2.group(1) and not r["draft"]]
                return self.reply(200, public(found[0])) if found else self.reply(404, {"message": "Not Found"})
            if method == "POST" and rest == "releases":
                if any(r["tag_name"] == body["tag_name"] for r in releases):
                    return self.reply(422, {"message": "Validation Failed: already_exists"})
                release = {"id": next(ids), "tag_name": body["tag_name"], "name": body.get("name"), "draft": bool(body.get("draft")),
                           "prerelease": bool(body.get("prerelease")), "generate_release_notes": bool(body.get("generate_release_notes")), "assets": []}
                releases.append(release)
                return self.reply(201, public(release))
            by_id = {r["id"]: r for r in releases}
            if method == "PATCH" and (m2 := re.fullmatch(r"releases/(\d+)", rest)):
                release = by_id.get(int(m2.group(1)))
                if not release:
                    return self.reply(404, {"message": "Not Found"})
                release.update({k: v for k, v in body.items() if k in ("draft", "prerelease", "make_latest", "name")})
                if not release["draft"]:
                    publish_files(owner_repo, release)
                return self.reply(200, public(release))
            if method == "POST" and (m2 := re.fullmatch(r"releases/(\d+)/assets", rest)):
                release = by_id.get(int(m2.group(1)))
                name = query.get("name", [""])[0]
                if not release or not name:
                    return self.reply(404, {"message": "Not Found"})
                if any(a["name"] == name for a in release["assets"]):
                    return self.reply(422, {"message": "Validation Failed: already_exists"})
                asset = {"id": next(ids), "name": name, "data": body}
                release["assets"].append(asset)
                return self.reply(201, {"id": asset["id"], "name": name, "size": len(body)})
            assets = {a["id"]: (r, a) for r in releases for a in r["assets"]}
            if (m2 := re.fullmatch(r"releases/assets/(\d+)", rest)) and int(m2.group(1)) in assets:
                release, asset = assets[int(m2.group(1))]
                if method == "DELETE":
                    release["assets"].remove(asset)
                    return self.reply(204, b"")
                if method == "GET":
                    if state.get("fail_asset_downloads"):
                        return self.reply(500, {"message": "Server Error"})
                    return self.reply(200, asset["data"])
            self.reply(404, {"message": "Not Found"})

        do_GET = do_POST = do_PATCH = do_DELETE = handle_any

    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    with open(os.path.join(root, "port.tmp"), "w") as f:
        f.write(str(server.server_address[1]))
    os.rename(os.path.join(root, "port.tmp"), os.path.join(root, "port"))
    server.serve_forever()


def set_field(fields, key, value):
    """gh's -f/-F: `output[title]=x` nests into {"output": {"title": x}}."""
    parts = re.findall(r"[^\[\]]+", key)
    for part in parts[:-1]:
        fields = fields.setdefault(part, {})
    fields[parts[-1]] = value


def request(method, path, data=None, binary=False, missing=None):
    """One call to the mock; returns the reply's bytes, or exits as gh does on an HTTP error (with `missing` on a 404)."""
    url = os.environ["MOCK_GITHUB_URL"] + "/" + path.lstrip("/")
    headers = {"Content-Type": "application/octet-stream" if binary else "application/json"}
    if os.environ.get("GH_TOKEN"):
        headers["Authorization"] = "token " + os.environ["GH_TOKEN"]
    req = urllib.request.Request(url, data=data, method=method, headers=headers)
    try:
        with urllib.request.urlopen(req) as resp:
            return resp.read()
    except urllib.error.HTTPError as e:
        if missing and e.code == 404:
            sys.exit(missing)
        message = json.loads(e.read() or b"{}").get("message", e.reason)
        sys.stderr.write(f"gh: {message} (HTTP {e.code})\n")
        sys.exit(1)


def emit(raw, jq):
    if jq is None:
        sys.stdout.buffer.write(raw)
        return
    out = subprocess.run(["jq", "-r", jq], input=raw, capture_output=True)
    sys.stdout.buffer.write(out.stdout)
    if out.returncode:
        sys.stderr.buffer.write(out.stderr)
        sys.exit(out.returncode)


def gh_api(args):
    method, fields, data, path, jq = None, {}, None, None, None
    i = 0
    while i < len(args):
        a = args[i]
        if a in ("-X", "--method"):
            method, i = args[i + 1], i + 2
        elif a == "--paginate":
            i += 1
        elif a in ("-H", "--header"):
            i += 2
        elif a in ("-q", "--jq"):
            jq, i = args[i + 1], i + 2
        elif a in ("-f", "--raw-field", "-F", "--field"):
            key, _, value = args[i + 1].partition("=")
            if a in ("-F", "--field") and value in ("true", "false"):
                value = value == "true"
            set_field(fields, key, value)
            i += 2
        elif a == "--input":
            src = args[i + 1]
            data = sys.stdin.buffer.read() if src == "-" else open(src, "rb").read()
            i += 2
        elif a.startswith("-"):
            sys.exit(f"gh shim: unsupported flag {a}")
        else:
            path, i = a, i + 1
    if path is None:
        sys.exit("gh shim: no path")
    if data is None and fields:
        data = json.dumps(fields).encode()
    method = method or ("POST" if data is not None else "GET")
    emit(request(method, path, data), jq)


def gh_release(args):
    """The `gh release` subcommands packaging/release/*.sh run, as REST calls."""
    repo = os.environ.get("GH_REPO") or sys.exit("gh shim: GH_REPO is not set")
    sub, args = args[0], args[1:]
    flags, positional, i = {}, [], 0
    while i < len(args):
        a = args[i]
        if a.startswith("--") and "=" in a:
            k, v = a[2:].split("=", 1)
            flags[k] = v
            i += 1
        elif a in ("--json", "--jq", "-q", "--title", "--dir", "-D", "--pattern", "-p", "--notes"):
            key = {"-q": "jq", "-D": "dir", "-p": "pattern"}.get(a, a[2:])
            if key == "pattern":
                flags.setdefault(key, []).append(args[i + 1])
            else:
                flags[key] = args[i + 1]
            i += 2
        elif a.startswith("-"):
            flags[a.lstrip("-")] = True
            i += 1
        else:
            positional.append(a)
            i += 1
    tag, files = positional[0], positional[1:]

    def find():
        for r in json.loads(request("GET", f"repos/{repo}/releases")):
            if r["tag_name"] == tag:
                return r
        sys.stderr.write("release not found\n")
        sys.exit(1)

    def upload(release, clobber):
        for f in files:
            name = os.path.basename(f)
            existing = [a for a in release["assets"] if a["name"] == name]
            if existing and not clobber:
                sys.exit(f"asset under the same name already exists: [{name}]")
            for a in existing:
                request("DELETE", f"repos/{repo}/releases/assets/{a['id']}")
            request("POST", f"repos/{repo}/releases/{release['id']}/assets?name={urllib.parse.quote(name)}", open(f, "rb").read(), binary=True)

    if sub == "view":
        r = find()
        view = {"tagName": r["tag_name"], "isDraft": r["draft"], "isPrerelease": r["prerelease"], "assets": [{"name": a["name"]} for a in r["assets"]]}
        fields = flags.get("json", "").split(",")
        emit(json.dumps({k: v for k, v in view.items() if k in fields}).encode(), flags.get("jq"))
    elif sub == "create":
        if flags.get("verify-tag"):
            request("GET", f"repos/{repo}/git/ref/tags/{tag}", missing=f"tag {tag} doesn't exist in the repo {repo}, aborting due to --verify-tag flag")
        body = {"tag_name": tag, "name": flags.get("title", tag), "draft": bool(flags.get("draft")),
                "prerelease": bool(flags.get("prerelease")), "generate_release_notes": bool(flags.get("generate-notes"))}
        release = json.loads(request("POST", f"repos/{repo}/releases", json.dumps(body).encode()))
        upload(release, False)
    elif sub == "upload":
        upload(find(), bool(flags.get("clobber")))
    elif sub == "download":
        r, out = find(), flags.get("dir", ".")
        patterns = flags.get("pattern", ["*"])
        matched = [a for a in r["assets"] if any(fnmatch.fnmatch(a["name"], p) for p in patterns)]
        if not matched:
            sys.exit("no assets match the file pattern")
        os.makedirs(out, exist_ok=True)
        for a in matched:
            with open(os.path.join(out, a["name"]), "wb") as f:
                f.write(request("GET", f"repos/{repo}/releases/assets/{a['id']}"))
    elif sub == "edit":
        body = {}
        if "draft" in flags:
            body["draft"] = flags["draft"] not in ("false", False)
        if flags.get("prerelease"):
            body["prerelease"] = True
        if flags.get("latest"):
            body["make_latest"] = "true"
        request("PATCH", f"repos/{repo}/releases/{find()['id']}", json.dumps(body).encode())
    else:
        sys.exit(f"gh shim: `gh release {sub}` is not simulated")


def gh_workflow_run(args):
    repo = os.environ.get("GH_REPO") or sys.exit("gh shim: GH_REPO is not set")
    if len(args) != 3 or args[1] != "--ref":
        sys.exit("gh shim: only `gh workflow run <workflow> --ref <ref>` is simulated")
    request("POST", f"repos/{repo}/actions/workflows/{args[0]}/dispatches", json.dumps({"ref": args[2]}).encode())
    print(f"Created workflow_dispatch event for {args[0]} at {args[2]}")


def gh(args):
    if args[:1] == ["api"]:
        gh_api(args[1:])
    elif args[:2] == ["workflow", "run"]:
        gh_workflow_run(args[2:])
    elif args[:1] == ["release"] and len(args) > 2:
        gh_release(args[1:])
    else:
        sys.exit(f"gh shim: `gh {' '.join(args)}` is not simulated")


if __name__ == "__main__":
    if sys.argv[1:2] == ["serve"]:
        serve(os.path.realpath(sys.argv[2]))
    elif sys.argv[1:2] == ["gh"]:
        gh(sys.argv[2:])
    else:
        sys.exit(__doc__)
