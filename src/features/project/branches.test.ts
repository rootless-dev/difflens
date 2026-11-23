import { describe, expect, it } from "vitest";

import {
  countBranches,
  groupBranches,
  isActive,
  isIntegrationBranch,
  matchesFilter,
  remoteName,
} from "@/features/project/branches";
import type { Branch } from "@/lib/api";
import { makeBranch, makeDetail } from "@/test/fixtures";

const now = new Date("2026-11-16T12:00:00Z");

function committed(name: string, authoredAt: string, overrides: Partial<Branch> = {}): Branch {
  return makeBranch(name, { head: { sha: name, subject: name, authoredAt }, ...overrides });
}

const names = (branches: Branch[]) => branches.map((branch) => branch.name);

describe("isActive", () => {
  it("counts a head commit exactly 14 days old as active and one second older as older", () => {
    expect(isActive(committed("a", "2026-11-02T12:00:00Z"), now)).toBe(true);
    expect(isActive(committed("b", "2026-11-02T11:59:59Z"), now)).toBe(false);
  });

  it("treats an invalid date as older", () => {
    expect(isActive(committed("a", "garbage"), now)).toBe(false);
  });
});

describe("isIntegrationBranch", () => {
  const patterns = ["main", "develop", "release/*"];

  it("matches exact names and glob patterns", () => {
    expect(isIntegrationBranch("develop", patterns)).toBe(true);
    expect(isIntegrationBranch("release/2.4", patterns)).toBe(true);
    expect(isIntegrationBranch("release/2.4/hotfix", patterns)).toBe(true);
  });

  it("does not match partial names or regex characters", () => {
    expect(isIntegrationBranch("develop-old", patterns)).toBe(false);
    expect(isIntegrationBranch("feature/release/x", patterns)).toBe(false);
    expect(isIntegrationBranch("mainx", ["main.x"])).toBe(false);
  });
});

describe("matchesFilter", () => {
  const branch = makeBranch("feature/ABC-178-refunds", { card: "ABC-178" });

  it("matches the name or the card, ignoring case and surrounding spaces", () => {
    expect(matchesFilter(branch, "")).toBe(true);
    expect(matchesFilter(branch, "  refunds ")).toBe(true);
    expect(matchesFilter(branch, "abc-178")).toBe(true);
    expect(matchesFilter(branch, "ABC-179")).toBe(false);
  });

  it("matches a card that is not part of the name", () => {
    expect(matchesFilter(makeBranch("spike/cache", { card: "ABC-12" }), "abc-12")).toBe(true);
    expect(matchesFilter(makeBranch("spike/cache", { card: null }), "abc")).toBe(false);
  });
});

describe("groupBranches", () => {
  const detail = makeDetail({
    branches: [
      committed("feature/ABC-1", "2026-11-16T10:00:00Z", { isCurrent: true }),
      committed("feature/ABC-2", "2026-11-15T10:00:00Z"),
      committed("feature/ABC-3", "2026-11-14T10:00:00Z", { location: "remote_only" }),
      committed("develop", "2026-11-14T10:00:00Z"),
      committed("release/2.4", "2026-11-14T10:00:00Z"),
      committed("feature/ABC-4", "2026-10-30T10:00:00Z"),
      committed("spike/token-cache", "2026-10-02T10:00:00Z", { card: null }),
      committed("feature/ABC-5", "2026-10-01T10:00:00Z", { location: "remote_only" }),
    ],
  });
  const view = { query: "", includeRemote: false, now };

  it("splits local branches into checked out, active and older, in the order received", () => {
    const groups = groupBranches(detail, view);

    expect(names(groups.current)).toEqual(["feature/ABC-1"]);
    expect(names(groups.active)).toEqual(["feature/ABC-2"]);
    expect(names(groups.older)).toEqual(["feature/ABC-4", "spike/token-cache"]);
    expect(groups.olderTotal).toBe(2);
    expect(groups.hasCurrent).toBe(true);
  });

  it("never lists integration branches, by pattern or by base name", () => {
    const withBase = { ...detail, bases: ["develop", "trunk"] };
    withBase.branches = [...detail.branches, committed("trunk", "2026-11-15T10:00:00Z")];

    const groups = groupBranches(withBase, { ...view, includeRemote: true });
    const all = [...groups.current, ...groups.active, ...groups.older];

    expect(names(all)).not.toContain("develop");
    expect(names(all)).not.toContain("release/2.4");
    expect(names(all)).not.toContain("trunk");
  });

  it("adds remote-only branches only when asked", () => {
    const groups = groupBranches(detail, { ...view, includeRemote: true });

    expect(names(groups.active)).toEqual(["feature/ABC-2", "feature/ABC-3"]);
    expect(names(groups.older)).toEqual(["feature/ABC-4", "spike/token-cache", "feature/ABC-5"]);
    expect(groups.olderTotal).toBe(3);
  });

  it("filters every group, older included, but keeps the unfiltered older count", () => {
    const groups = groupBranches(detail, { ...view, query: "token" });

    expect(groups.current).toEqual([]);
    expect(groups.active).toEqual([]);
    expect(names(groups.older)).toEqual(["spike/token-cache"]);
    expect(groups.olderTotal).toBe(2);
    expect(groups.hasCurrent).toBe(true);
  });

  it("filters by card", () => {
    const groups = groupBranches(detail, { ...view, query: "abc-4" });

    expect(names(groups.older)).toEqual(["feature/ABC-4"]);
    expect(groups.active).toEqual([]);
  });

  it("reports when no branch to review is checked out", () => {
    const groups = groupBranches(makeDetail({ branches: [committed("feature/ABC-2", "2026-11-15T10:00:00Z")] }), view);

    expect(groups.hasCurrent).toBe(false);
    expect(groups.current).toEqual([]);
  });
});

describe("countBranches", () => {
  it("separates local and remote-only branches", () => {
    expect(
      countBranches([
        makeBranch("a"),
        makeBranch("b", { location: "remote_only" }),
        makeBranch("c"),
      ]),
    ).toEqual({ local: 2, remoteOnly: 1 });
  });
});

describe("remoteName", () => {
  it("takes the prefix of the upstream", () => {
    expect(remoteName("origin/feature/ABC-1")).toBe("origin");
    expect(remoteName("upstream/main")).toBe("upstream");
    expect(remoteName("weird")).toBe("the remote");
  });
});
