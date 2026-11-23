/** Folder name git would create for `url`: "git@host:acme/ledger.git" gives "ledger". */
export function repositoryName(url: string): string {
  const trimmed = url.trim().replace(/[/\\]+$/, "").replace(/\.git$/i, "");
  const lastSeparator = Math.max(
    trimmed.lastIndexOf("/"),
    trimmed.lastIndexOf("\\"),
    trimmed.lastIndexOf(":"),
  );
  return lastSeparator === -1 ? "" : trimmed.slice(lastSeparator + 1);
}

export function joinPath(parent: string, name: string): string {
  if (!name) {
    return parent;
  }
  const separator = parent.includes("\\") && !parent.includes("/") ? "\\" : "/";
  return parent.endsWith(separator) ? parent + name : parent + separator + name;
}
