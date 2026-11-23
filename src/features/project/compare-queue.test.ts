import { describe, expect, it, vi } from "vitest";

import { CompareQueue, type CompareTarget } from "@/features/project/compare-queue";
import type { CompareSummary } from "@/lib/api";
import { apiError, deferred, makeSummary } from "@/test/fixtures";

function target(
  branch: string,
  base = "develop",
  headSha = `sha-${branch}`,
  baseSha = `sha-${base}`,
): CompareTarget {
  return { branch, base, baseSha, headSha };
}

/** A compare function whose calls stay pending until resolved by branch name. */
function controlledCompare() {
  const pending = new Map<string, ReturnType<typeof deferred<CompareSummary>>>();
  const compare = vi.fn((t: CompareTarget) => {
    const call = deferred<CompareSummary>();
    pending.set(`${t.branch}@${t.base}`, call);
    return call.promise;
  });
  const resolve = async (branch: string, base = "develop") => {
    pending.get(`${branch}@${base}`)?.resolve(makeSummary(branch, base));
    await flush();
  };
  const reject = async (branch: string, error: unknown, base = "develop") => {
    pending.get(`${branch}@${base}`)?.reject(error);
    await flush();
  };
  return { compare, resolve, reject };
}

const flush = () => new Promise((resolve) => setTimeout(resolve, 0));

describe("CompareQueue", () => {
  it("runs at most four comparisons at a time, in the order wanted", async () => {
    const { compare, resolve } = controlledCompare();
    const queue = new CompareQueue(compare, 4);

    queue.want(["a", "b", "c", "d", "e", "f"].map((name) => target(name)));

    expect(compare.mock.calls.map(([t]) => t.branch)).toEqual(["a", "b", "c", "d"]);
    expect(queue.get(target("e"))).toEqual({ status: "loading" });

    await resolve("b");

    expect(compare.mock.calls.map(([t]) => t.branch)).toEqual(["a", "b", "c", "d", "e"]);
    expect(queue.get(target("b"))).toMatchObject({ status: "ready", data: { branch: "b" } });
  });

  it("does not compare again what it already knows or is running", async () => {
    const { compare, resolve } = controlledCompare();
    const queue = new CompareQueue(compare);

    queue.want([target("a")]);
    queue.want([target("a")]);
    await resolve("a");
    queue.want([target("a"), target("a", "main")]);
    queue.want([target("a")]);

    expect(compare.mock.calls.map(([t]) => `${t.branch}@${t.base}`)).toEqual(["a@develop", "a@main"]);
  });

  it("compares again when the branch head moves", async () => {
    const { compare, resolve } = controlledCompare();
    const queue = new CompareQueue(compare);

    queue.want([target("a", "develop", "old")]);
    await resolve("a");
    queue.want([target("a", "develop", "new")]);

    expect(compare).toHaveBeenCalledTimes(2);
    expect(queue.get(target("a", "develop", "new"))).toEqual({ status: "loading" });
  });

  it("compares again when only the base moves", async () => {
    const { compare, resolve } = controlledCompare();
    const queue = new CompareQueue(compare);

    queue.want([target("a", "develop", "head", "base-old")]);
    await resolve("a");
    queue.want([target("a", "develop", "head", "base-new")]);

    expect(compare).toHaveBeenCalledTimes(2);
    expect(compare.mock.calls[1][0]).toMatchObject({ baseSha: "base-new", headSha: "head" });
    expect(queue.get(target("a", "develop", "head", "base-new"))).toEqual({ status: "loading" });
    expect(queue.get(target("a", "develop", "head", "base-old"))).toMatchObject({ status: "ready" });
  });

  it("drops queued comparisons that are no longer wanted before they start", async () => {
    const { compare, resolve } = controlledCompare();
    const queue = new CompareQueue(compare, 1);

    queue.want([target("a"), target("b"), target("c")]);
    queue.want([target("a"), target("c")]);

    expect(queue.get(target("b"))).toBeUndefined();

    await resolve("a");

    expect(compare.mock.calls.map(([t]) => t.branch)).toEqual(["a", "c"]);
  });

  it("keeps a short and a full message for a failed comparison", async () => {
    const { compare, reject } = controlledCompare();
    const queue = new CompareQueue(compare);

    queue.want([target("a"), target("b")]);
    await reject("a", apiError("no_common_history", "feature/a and develop share no commit."));
    await reject("b", new Error("boom"));

    expect(queue.get(target("a"))).toEqual({
      status: "error",
      short: "No common history",
      message: "feature/a and develop share no commit.",
    });
    expect(queue.get(target("b"))).toEqual({ status: "error", short: "Could not compare", message: "boom" });
  });

  it("notifies subscribers and changes its version on every change", async () => {
    const { compare, resolve } = controlledCompare();
    const queue = new CompareQueue(compare);
    const listener = vi.fn();
    queue.subscribe(listener);

    const before = queue.getVersion();
    queue.want([target("a")]);
    await resolve("a");

    expect(listener).toHaveBeenCalledTimes(2);
    expect(queue.getVersion()).toBe(before + 2);

    queue.want([target("a")]);
    expect(listener).toHaveBeenCalledTimes(2);
  });
});
