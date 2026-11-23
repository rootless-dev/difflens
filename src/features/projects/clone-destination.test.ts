import { describe, expect, it } from "vitest";

import { joinPath, repositoryName } from "@/features/projects/clone-destination";

describe("repositoryName", () => {
  it.each([
    ["git@bitbucket.org:acme/ledger-worker.git", "ledger-worker"],
    ["https://github.com/acme/notifier", "notifier"],
    ["https://github.com/acme/notifier.git/", "notifier"],
    ["ssh://git@host:7999/team/payments-api.git", "payments-api"],
    ["git@host:solo.git", "solo"],
    ["  /srv/git/tools.git  ", "tools"],
    ["", ""],
    ["not-a-url", ""],
  ])("%s gives %s", (url, expected) => {
    expect(repositoryName(url)).toBe(expected);
  });
});

describe("joinPath", () => {
  it.each([
    ["/work", "ledger", "/work/ledger"],
    ["/work/", "ledger", "/work/ledger"],
    ["C:\\work", "ledger", "C:\\work\\ledger"],
    ["/work", "", "/work"],
  ])("%s + %s gives %s", (parent, name, expected) => {
    expect(joinPath(parent, name)).toBe(expected);
  });
});
