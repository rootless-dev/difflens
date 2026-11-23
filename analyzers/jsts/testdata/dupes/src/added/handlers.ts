// Added by the change: one validation written out three times.
export function createUser(input: Record<string, unknown>) {
  const found: string[] = [];
  if (typeof input.name !== "string" || input.name.trim() === "") {
    found.push("name is required");
  }
  if (typeof input.email !== "string" || !input.email.includes("@")) {
    found.push("email is invalid");
  }
  if (found.length > 0) {
    return { ok: false, errors: found };
  }
  return { ok: true, errors: found };
}

export const USER_LIMIT = 100;

export function updateUser(patch: Record<string, unknown>) {
  const problems: string[] = [];
  if (typeof patch.name !== "string" || patch.name.trim() === "") {
    problems.push("a name is needed");
  }
  if (typeof patch.email !== "string" || !patch.email.includes("@")) {
    problems.push("the email is wrong");
  }
  if (problems.length > 0) {
    return { ok: false, errors: problems };
  }
  return { ok: true, errors: problems };
}

function audit(message: string) {
  console.log(`audit: ${message}`);
}

export function inviteUser(invite: Record<string, unknown>) {
  const issues: string[] = [];
  if (typeof invite.name !== "string" || invite.name.trim() === "") {
    issues.push("who is invited?");
  }
  if (typeof invite.email !== "string" || !invite.email.includes("@")) {
    issues.push("where to send it?");
  }
  if (issues.length > 0) {
    return { ok: false, errors: issues };
  }
  audit("invited");
  return { ok: true, errors: issues };
}
