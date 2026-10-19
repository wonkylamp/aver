// Path segments. Public API: see README.

/** Joins segments with `/`, resolving `.` and `..`; the result starts with `/`. */
export function joinPath(...segments: string[]): string {
  const out: string[] = [];
  for (const part of segments.join("/").split("/")) {
    if (part === "" || part === ".") continue;
    if (part === "..") out.pop();
    else out.push(part);
  }
  return `/${out.join("/")}`;
}

/** The non-empty segments of a path, still encoded. */
export function segments(path: string): string[] {
  return path.split("/").filter((s) => s !== "");
}
