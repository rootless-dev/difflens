import { describe, expect, it } from "vitest";

import { shouldAutoFetch } from "@/features/project/use-project-fetch";
import { makeProject } from "@/test/fixtures";

const now = new Date("2026-11-16T12:00:00Z");

describe("shouldAutoFetch", () => {
  it("fetches a project that was never fetched", () => {
    expect(shouldAutoFetch(makeProject({ lastFetchedAt: null }), now)).toBe(true);
  });

  it("fetches when the last fetch is older than 10 minutes", () => {
    expect(shouldAutoFetch(makeProject({ lastFetchedAt: "2026-11-16T11:49:59Z" }), now)).toBe(true);
  });

  it("does not fetch when the last fetch is 10 minutes old or less", () => {
    expect(shouldAutoFetch(makeProject({ lastFetchedAt: "2026-11-16T11:50:00Z" }), now)).toBe(false);
    expect(shouldAutoFetch(makeProject({ lastFetchedAt: "2026-11-16T11:58:00Z" }), now)).toBe(false);
  });

  it("never fetches an unavailable project", () => {
    expect(shouldAutoFetch(makeProject({ available: false, lastFetchedAt: null }), now)).toBe(false);
  });

  it("never fetches a repository without a remote", () => {
    expect(shouldAutoFetch(makeProject({ hasRemote: false, lastFetchedAt: null }), now)).toBe(false);
    expect(shouldAutoFetch(makeProject({ hasRemote: false, lastFetchedAt: "garbage" }), now)).toBe(false);
  });

  it("fetches when the stored timestamp cannot be read", () => {
    expect(shouldAutoFetch(makeProject({ lastFetchedAt: "garbage" }), now)).toBe(true);
  });
});
