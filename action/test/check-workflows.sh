#!/usr/bin/env bash
# Parses every workflow and action YAML file and fails if one does not parse
# or has a `uses:` that is not pinned to a full commit SHA (local ./ paths are
# allowed).
set -euo pipefail

root=$(cd "$(dirname "$0")/../.." && pwd)
files=()
for f in "$root"/.github/workflows/*.yml "$root"/.github/workflows/*.yaml "$root"/.github/actions/*/action.yml \
  "$root"/action/action.yml "$root"/action/examples/*.yml "$root"/action/examples/*.yaml; do
  [ -f "$f" ] && files+=("$f")
done
[ "${#files[@]}" -gt 0 ] || {
  echo "no YAML files found under $root" >&2
  exit 1
}

# Prints "<file>\t<uses>" for every uses: key, or exits non-zero on a parse
# error.
list_uses_python() {
  python3 -c '
import sys, yaml
def walk(node, path):
    if isinstance(node, dict):
        for k, v in node.items():
            if k == "uses" and isinstance(v, str):
                print(path + "\t" + v)
            walk(v, path)
    elif isinstance(node, list):
        for v in node:
            walk(v, path)
for path in sys.argv[1:]:
    with open(path) as f:
        walk(yaml.safe_load(f), path)
' "$@"
}

list_uses_ruby() {
  ruby -ryaml -e '
def walk(node, path)
  case node
  when Hash
    node.each do |k, v|
      puts "#{path}\t#{v}" if k == "uses" && v.is_a?(String)
      walk(v, path)
    end
  when Array
    node.each { |v| walk(v, path) }
  end
end
ARGV.each { |path| walk(YAML.safe_load(File.read(path), aliases: true), path) }
' "$@"
}

if python3 -c 'import yaml' 2>/dev/null; then
  uses=$(list_uses_python "${files[@]}")
elif command -v ruby >/dev/null 2>&1; then
  uses=$(list_uses_ruby "${files[@]}")
else
  echo "neither python3 with PyYAML nor ruby is available to parse YAML" >&2
  exit 1
fi

bad=0
while IFS=$'\t' read -r file ref; do
  [ -n "$file" ] || continue
  case "$ref" in
    ./*) continue ;;
  esac
  if ! printf '%s\n' "$ref" | grep -Eq '^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+(/[A-Za-z0-9_./-]+)?@[0-9a-f]{40}$'; then
    echo "${file#"$root"/}: uses '$ref' is not pinned to a full commit SHA" >&2
    bad=1
  fi
done <<<"$uses"

for f in "${files[@]}"; do
  while IFS= read -r line; do
    echo "${f#"$root"/}: '$line' lacks a '# vX.Y.Z' comment naming the pinned version" >&2
    bad=1
  done < <(grep -E '^[[:space:]-]*uses:[[:space:]]*[^.[:space:]][^[:space:]]*@[0-9a-f]{40}' "$f" | grep -Ev '#[[:space:]]*v[0-9]+\.[0-9]+\.[0-9]+' || true)
done

[ "$bad" = 0 ] || exit 1
echo "${#files[@]} YAML files parse; every uses: is pinned to a commit SHA"
