import { describe, expect, it } from "vitest";

import {
  agentPrompt,
  callsText,
  formatTokens,
  locationNote,
  planShape,
  recheckNote,
  resolutionByline,
  skippedText,
  whereText,
  writtenBy,
} from "@/features/review/insights/insights-text";
import { i18n } from "@/i18n";
import type { ResolutionKind } from "@/lib/api";
import { makeAnalysis, makeFinding, makePlan } from "@/test/insights-fixtures";

describe("insights text", () => {
  it("rounds token estimates", () => {
    expect(formatTokens(820)).toBe("820");
    expect(formatTokens(48_200)).toBe("48k");
    expect(formatTokens(999_499)).toBe("999k");
    expect(formatTokens(1_240_000)).toBe("1.2M");
    expect(formatTokens(2_000_000)).toBe("2M");
  });

  it("gives the cost shape of an analysis and what it leaves out", () => {
    expect(planShape(makePlan())).toBe("5 files in 1 call, about 48k tokens in");
    const split = makePlan({
      files: 1,
      calls: [
        { paths: ["a.go"], tokens: 90_000, truncated: [] },
        { paths: ["b.go"], tokens: 60_000, truncated: [] },
      ],
      tokens: 150_000,
      skipped: [{ path: "x.png", reason: "binary" }],
    });
    expect(planShape(split)).toBe("1 file in 2 calls, about 150k tokens in");
    expect(skippedText(split)).toBe("1 file is listed to the model but not sent: 1 binary.");
    expect(skippedText(makePlan({ skipped: [] }))).toBeNull();
  });

  it("names the calls being asked", () => {
    expect(callsText([1], 4)).toBe("Call 2 of 4");
    expect(callsText([2, 0], 4)).toBe("Calls 1 and 3 of 4");
  });

  it("says who read the change, of which head", () => {
    expect(writtenBy(makeAnalysis())).toBe("Read by Claude Code, model claude-opus-5-5 at e7f3b10");
    expect(writtenBy(makeAnalysis({ headSha: null }))).toBe("Read by Claude Code, model claude-opus-5-5 on the working tree");
  });

  it("places a finding where its lines are now", () => {
    const moved = makeFinding("f", { location: { kind: "found", line: 60, endLine: 64, moved: true } });
    expect(whereText(moved)).toBe("internal/refund/service.go:60–64");
    expect(locationNote(moved)).toBeNull();
    const single = makeFinding("f", { line: 7, endLine: 7, location: { kind: "not_found", problem: "outside_file" } });
    expect(whereText(single)).toBe("internal/refund/service.go:7");
    expect(locationNote(single)).toBe("Location not found: internal/refund/service.go has no line 7.");
  });

  it("asks for a recheck only of a classified finding whose code changed", () => {
    const at = "2026-11-12T09:00:00Z";
    expect(recheckNote(makeFinding("f", { recheck: false, verdict: "confirmed", verdictAt: at }))).toBeNull();
    expect(recheckNote(makeFinding("f", { recheck: true, verdict: "dismissed", verdictAt: at }))).toBe(
      "You dismissed this on 12 Nov. The code around it changed since then, so check it again.",
    );
    expect(
      recheckNote(
        makeFinding("f", {
          recheck: true,
          verdict: "confirmed",
          verdictAt: at,
          location: { kind: "not_found", problem: "code_changed" },
        }),
      ),
    ).toBe("You confirmed this on 12 Nov. The code it points at can no longer be found, so check it again.");
  });

  it("says who found a finding fixed and when, in the language shown", async () => {
    const now = new Date("2026-11-20T12:00:00Z");
    const fixed = (kind: ResolutionKind, model: string | null, at: string) =>
      makeFinding("f", { resolution: { kind, model, at, reason: null } });
    const checked = fixed("model_checked", "claude-opus-5-5-20261101", "2026-11-20T10:00:00Z");
    const marked = fixed("user_marked", null, "2026-11-20T11:59:30Z");
    const left = fixed("file_left_change", null, "2026-11-19T12:00:00Z");

    expect(resolutionByline(makeFinding("f"), now)).toBeNull();
    expect(resolutionByline(checked, now)).toBe("Checked by claude-opus-5-5-20261101 2 h ago");
    expect(resolutionByline(marked, now)).toBe("Marked by you just now");
    expect(resolutionByline(left, now)).toBe("Fixed yesterday: the file is no longer in the change");

    await i18n.changeLanguage("pt-BR");
    expect(resolutionByline(checked, now)).toBe("Conferido por claude-opus-5-5-20261101 há 2 h");
    expect(resolutionByline(marked, now)).toBe("Marcado por você agora mesmo");
    expect(resolutionByline(left, now)).toBe("Corrigido ontem: o arquivo não faz mais parte da mudança");
  });

  it("copies a finding whose code is gone without inventing code", () => {
    const prompt = agentPrompt(
      makeFinding("f", { location: { kind: "not_found", problem: "code_changed" }, excerpt: [], related: [] }),
    );
    expect(prompt).toContain("It cited line 52, but that code could not be found any more");
    expect(prompt).not.toContain("```");
    expect(prompt).not.toContain("Related, but not proof");
  });
});
