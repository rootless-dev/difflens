import { describe, expect, it } from "vitest";

import {
  attention,
  checksCount,
  functionDraftBody,
  functionSignal,
  functionsPrompt,
  type OverLimit,
} from "@/features/review/quality/quality-text";
import { setLanguage } from "@/i18n";
import type { CrossedLimit } from "@/lib/api";
import { makeCheck, makeQualityFunction, makeQualityReport } from "@/test/tests-fixtures";

const fn = makeQualityFunction("shipping.calculateShipping", {
  file: "internal/shipping/cost.go",
  startLine: 10,
  endLine: 13,
  code: ["func calculateShipping() int {", "\treturn 1", "}"],
});
const ref = { id: fn.id, qualifiedName: fn.qualifiedName };
const limit = (kind: CrossedLimit["kind"], value: number, bound: number): CrossedLimit => ({
  kind,
  function: ref,
  value,
  limit: bound,
});

describe("quality text", () => {
  it("splits the functions by whether the core says they cross a limit, in the report's order", () => {
    const report = makeQualityReport();
    const { over, within } = attention(report);
    expect(over.map((item) => [item.fn.id, item.crossings.map((crossing) => crossing.kind)])).toEqual([
      ["refund.Validate", ["complexity"]],
      ["refund.Service.Request", ["complexity", "crap"]],
    ]);
    expect(within.map((item) => item.id)).toEqual(["payment.Repo.Get"]);
  });

  it("suggests a direction from the limits crossed", () => {
    const body = (crossings: CrossedLimit[]) => functionDraftBody({ fn, crossings });
    expect(body([limit("complexity", 14, 10)])).toBe(
      "`calculateShipping` has complexity 14 (limit 10); consider splitting it into smaller functions.",
    );
    expect(body([limit("changed_lines_coverage", 0.4, 0.8)])).toBe(
      "`calculateShipping` has 40% of its changed lines covered (target 80%); consider covering the new branches with tests.",
    );
    expect(body([limit("complexity", 14, 10), limit("crap", 40.5, 30), limit("changed_lines_coverage", 0.4, 0.8)])).toBe(
      "`calculateShipping` has complexity 14 (limit 10), a CRAP score of 40.5 (limit 30) and 40% of its changed lines " +
        "covered (target 80%); consider splitting it into smaller functions and covering the new branches.",
    );
  });

  it("writes the draft and the prompt in the language shown", () => {
    setLanguage("pt-BR");
    const item: OverLimit = { fn, crossings: [limit("complexity", 14, 10), limit("changed_lines_coverage", 0.4, 0.8)] };
    expect(functionDraftBody(item)).toBe(
      "`calculateShipping` tem complexidade 14 (limite 10) e 40% das linhas alteradas cobertas (meta 80%); " +
        "considere dividi-la em funções menores e cobrir os novos ramos.",
    );
    const prompt = functionsPrompt([item]);
    expect(prompt).toMatch(/^Refatore a função `calculateShipping` em internal\/shipping\/cost\.go, linhas 10 a 13:/);
    expect(prompt).toContain("O código dela (internal/shipping/cost.go:10-13):");
    // Cut short by the core: says so.
    expect(prompt).toContain("(só as primeiras 3 linhas aparecem)");
    expect(prompt).toMatch(/Execute os testes ao terminar\.$/);
  });

  it("keys a function's draft by its file and id", () => {
    expect(functionSignal(fn)).toBe("quality:internal/shipping/cost.go:shipping.calculateShipping");
  });

  it("counts every finding of the checks, and races found", () => {
    const report = makeQualityReport({
      checks: [
        makeCheck("go vet"),
        makeCheck("staticcheck", {
          status: "findings",
          findings: [
            { file: "a.go", line: 1, message: "m", rule: null, severity: "info" },
            { file: "a.go", line: 2, message: "m", rule: null, severity: "error" },
          ],
        }),
      ],
      raceDetector: "races",
    });
    expect(checksCount(report)).toBe(3);
    expect(checksCount(makeQualityReport())).toBe(0);
  });
});
