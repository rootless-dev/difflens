import type { FunctionBrief, FunctionExplanation } from "@/lib/api";

/**
 * A detailed explanation of a changed function, written in English by Claude Code (as every explanation written
 * before short ones existed is); `overrides` changes any of it.
 */
export function makeExplanation(functionId: string, overrides: Partial<FunctionExplanation> = {}): FunctionExplanation {
  return {
    functionId,
    kind: "changed",
    whatChanged: "Adds the `POST /refunds` handler. It decodes the body and calls `Service.Request`.",
    why: "The card asks for refunds of settled payments.",
    whyInferred: false,
    purpose: "Turns a refund request over HTTP into a call to the refund service.",
    language: "en",
    translations: [],
    provider: "claude_cli",
    providerName: "Claude Code",
    model: "opus",
    createdAt: "2026-11-19T10:00:00Z",
    headSha: "e7f3b10c9d8e7f6a",
    outdated: false,
    ...overrides,
  };
}

/** A short explanation of a changed function, written in English by Claude Code; `overrides` changes any of it. */
export function makeBrief(functionId: string, overrides: Partial<FunctionBrief> = {}): FunctionBrief {
  return {
    functionId,
    kind: "changed",
    text: "New `POST /refunds` handler that turns a refund request into a call to the refund service.",
    language: "en",
    translations: [],
    provider: "claude_cli",
    providerName: "Claude Code",
    model: "opus",
    createdAt: "2026-11-21T10:00:00Z",
    headSha: "e7f3b10c9d8e7f6a",
    outdated: false,
    ...overrides,
  };
}
