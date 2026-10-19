#!/usr/bin/env python3
"""A stand-in for `release-plz release` under release-plz.toml's git_only
settings, for action/test/workflows.sh: each workspace package the config
does not exclude (`release = false`) is tagged git_tag_name at HEAD through
the GitHub API (an annotated tag object, then its ref) with --git-token,
unless the checkout already has that tag. Prints the release command's
JSON output. It refuses settings it does not simulate."""
import glob
import json
import os
import subprocess
import sys
import tomllib

args = sys.argv[1:]
if args[:1] != ["release"] or "--git-token" not in args:
    sys.exit(f"release-plz stand-in: only `release --git-token <t>` is simulated, not {args}")
token = args[args.index("--git-token") + 1]
if not token:
    sys.exit("release-plz stand-in: --git-token is empty")
config = tomllib.load(open("release-plz.toml", "rb"))
ws = config.get("workspace", {})
for key, want in (("git_only", True), ("git_release_enable", False)):
    if ws.get(key) != want:
        sys.exit(f"release-plz stand-in: simulates {key} = {want}; release-plz.toml has {ws.get(key)}")
excluded = {p["name"] for p in config.get("package", []) if p.get("release") is False}
version = tomllib.load(open("Cargo.toml", "rb"))["workspace"]["package"]["version"]
repo = os.environ["GITHUB_REPOSITORY"]
env = dict(os.environ, GH_TOKEN=token, GH_REPO=repo)
head = subprocess.run(["git", "rev-parse", "HEAD"], capture_output=True, text=True, check=True).stdout.strip()
releases = []
for manifest in sorted(glob.glob("crates/*/Cargo.toml")):
    name = tomllib.load(open(manifest, "rb"))["package"]["name"]
    if name in excluded:
        continue
    tag = ws.get("git_tag_name", "{{ package }}-v{{ version }}").replace("{{ version }}", version).replace("{{ package }}", name)
    if subprocess.run(["git", "tag", "-l", tag], capture_output=True, text=True).stdout.strip():
        print(f"release-plz: {name} {version} is already tagged {tag}", file=sys.stderr)
        continue
    made = subprocess.run(["gh", "api", f"repos/{repo}/git/tags", "-f", f"tag={tag}", "-f", f"message={tag}", "-f", f"object={head}", "-f", "type=commit"],
                          env=env, capture_output=True, text=True, check=True)
    subprocess.run(["gh", "api", f"repos/{repo}/git/refs", "-f", f"ref=refs/tags/{tag}", "-f", f"sha={json.loads(made.stdout)['sha']}"],
                   env=env, check=True, stdout=subprocess.DEVNULL)
    releases.append({"package_name": name, "prs": [], "tag": tag, "version": version})
print(json.dumps({"releases": releases}))
