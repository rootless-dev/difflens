// Two copies that were already there before the change: not the change's doing.
export function parseQuery(search: string): Map<string, string> {
  const params = new Map<string, string>();
  for (const part of search.replace(/^\?/, "").split("&")) {
    if (part === "") continue;
    const [key, value = ""] = part.split("=");
    params.set(decodeURIComponent(key), decodeURIComponent(value));
  }
  return params;
}

export function parseFragment(hash: string): Map<string, string> {
  const fields = new Map<string, string>();
  for (const piece of hash.replace(/^#/, "").split(";")) {
    if (piece === "") continue;
    const [name, text = ""] = piece.split(":");
    fields.set(decodeURIComponent(name), decodeURIComponent(text));
  }
  return fields;
}
