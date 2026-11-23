// A StrykerJS reporter plugin, loaded by Stryker from its absolute path
// (`plugins` of the run's configuration): it appends what the run learns, as
// it learns it, to the file DIFFLENS_STRYKER_EVENTS, one JSON object per line,
// so that `mutate` sees each result as soon as it is known and keeps them even
// when it has to stop Stryker at its deadline. It imports nothing: Stryker
// finds the plugin by the shape of `strykerPlugins`.
//
//   {"e":"tests","tests":[{"id","name","file","line","column"}]}  the baseline (dry run) is done
//   {"e":"plan","plans":[{"id","plan","status"}]}                 the mutants are planned ("EarlyResult": reused or ignored)
//   {"e":"mutant","id","file","mutator","replacement","start","end","status","reason","killedBy","duration"}

import { appendFileSync } from "node:fs";

const EVENTS = process.env.DIFFLENS_STRYKER_EVENTS;

function emit(event) {
  if (!EVENTS) return;
  try {
    appendFileSync(EVENTS, `${JSON.stringify(event)}\n`);
  } catch {
    // The analyzer reads what it gets; a lost line only costs a progress update.
  }
}

class DiffLensReporter {
  static inject = [];

  onDryRunCompleted(event) {
    const tests = (event?.result?.tests ?? []).map((t) => ({
      id: String(t.id),
      name: t.name,
      file: t.fileName ?? null,
      line: t.startPosition?.line ?? null,
      column: t.startPosition?.column ?? null,
    }));
    emit({ e: "tests", tests });
  }

  onMutationTestingPlanReady(event) {
    const plans = (event?.mutantPlans ?? []).map((p) => ({ id: String(p.mutant.id), plan: p.plan, status: p.mutant.status ?? null }));
    emit({ e: "plan", plans });
  }

  onMutantTested(result) {
    emit({
      e: "mutant",
      id: String(result.id),
      file: result.fileName,
      mutator: result.mutatorName,
      replacement: result.replacement ?? null,
      start: result.location.start,
      end: result.location.end,
      status: result.status,
      reason: result.statusReason ?? null,
      killedBy: (result.killedBy ?? []).map(String),
      duration: result.duration ?? null,
    });
  }
}

export const strykerPlugins = [{ kind: "Reporter", name: "difflens-events", injectableClass: DiffLensReporter }];
