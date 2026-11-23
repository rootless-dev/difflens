import { describe, expect, it } from "vitest";

import { mutantRows } from "@/features/review/tests/mutant-parts";
import { mutantPrompt, mutationScore } from "@/features/review/tests/mutation-text";
import { i18n } from "@/i18n";
import { makeMutant, makeMutationRun } from "@/test/tests-fixtures";

describe("the score of a mutation run", () => {
  it("counts the killed out of the killed, survived and timed out, leaving errors and no coverage out", () => {
    const { summary } = makeMutationRun();
    expect(mutationScore(summary)).toEqual({ killed: 1, total: 4 });
    expect(mutationScore({ ...summary, killed: 12, survived: 2, timeout: 1, error: 3, noCoverage: 4 })).toEqual({
      killed: 12,
      total: 15,
    });
  });
});

describe("the prompt a mutant that survived is copied as", () => {
  const run = makeMutationRun();

  it("says where, the line as it is and as mutated, that no test failed, the tests nearby, and the ask", () => {
    expect(mutantPrompt(run.mutants[0], run)).toBe(
      [
        "Write the missing test for internal/refund/service.go:52, in refund.Service.Request.",
        "Mutation testing changed this line, and every test still passed: the tests run it but do not check what it does.",
        "The change (comparison boundary):\n```diff\n- \tif balance >= amount {\n+ \tif balance > amount {\n```",
        "In the mutation tool's words: replace >= with > in Service.Request",
        "Tests that catch other changes in this function: TestRequest.",
        "Write a test that fails with this change and passes with the code as it is. Do not change the production " +
          "code. To confirm, apply the change by hand, see the test fail, then undo it.",
      ].join("\n\n"),
    );
  });

  it("leaves out what it does not know: the function, the tests nearby, the lines", () => {
    const loose = makeMutant("m-9", { function: null, before: [], after: [], description: "" });
    const prompt = mutantPrompt(loose, null);
    expect(prompt).toMatch(/^Write the missing test for internal\/refund\/service\.go:52\.\n/);
    expect(prompt).toContain("```diff\n- >=\n+ >\n```");
    expect(prompt).not.toContain("In the mutation tool's words");
    expect(prompt).not.toContain("Tests that catch other changes");
  });

  it("is written in the language shown, the code as it is", async () => {
    await i18n.changeLanguage("pt-BR");
    const prompt = mutantPrompt(run.mutants[1], run);
    expect(prompt).toContain("Escreva o teste que falta para internal/refund/service.go:58, em refund.Service.Request.");
    expect(prompt).toContain(
      "O teste de mutação alterou esta linha e todos os testes continuaram passando: os testes a executam, mas não " +
        "verificam o que ela faz.",
    );
    expect(prompt).toContain("A alteração (chamada removida):\n```diff\n- \t\ts.notify(refund)\n+ \n```");
    expect(prompt).toContain("Testes que pegam outras alterações nesta função: TestRequest.");
    expect(prompt).toContain("Não altere o código de produção.");
    await i18n.changeLanguage("en");
  });
});

describe("the code of a mutant", () => {
  it("marks the piece replaced and what replaced it, when the line holds it where the mutant says", () => {
    const rows = mutantRows(makeMutant("m-1"));
    expect(rows.map((row) => [row.sign, row.line, row.parts.filter((part) => part.marked).map((part) => part.text)])).toEqual([
      ["-", 52, [">="]],
      ["+", 52, [">"]],
    ]);
  });

  it("shows the lines plain when they do not hold the piece, and the pieces alone when unread", () => {
    const moved = mutantRows(makeMutant("m-1", { column: 3 }));
    expect(moved.every((row) => row.parts.every((part) => !part.marked))).toBe(true);
    expect(mutantRows(makeMutant("m-1", { before: [], after: [] }))).toEqual([
      { sign: "-", line: null, parts: [{ text: ">=", marked: true }] },
      { sign: "+", line: null, parts: [{ text: ">", marked: true }] },
    ]);
  });
});
