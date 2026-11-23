import type {
  Duplicate,
  LocatedCheck,
  Mutant,
  MutationRun,
  MutationState,
  QualityFunction,
  QualityReport,
  StaticCheck,
  TestCase,
  TestRun,
  TestsState,
} from "@/lib/api";

/** A Go test unless told otherwise: its hierarchy is the name split at "/", its title the last level read as Go. */
export function makeTestCase(name: string, overrides: Partial<TestCase> = {}): TestCase {
  const container = overrides.container ?? "internal/refund";
  const hierarchy = overrides.hierarchy ?? name.split("/");
  const last = hierarchy.at(-1) ?? name;
  return {
    id: `${container}:${name}`,
    container,
    package: overrides.package ?? container,
    name,
    hierarchy,
    title: hierarchy.length > 1 ? last.replaceAll("_", " ") : name,
    command: null,
    functionId: `${container}.${hierarchy[0]}`,
    state: "new",
    outcome: "pass",
    durationMs: 1,
    output: null,
    file: "internal/refund/service_test.go",
    line: 20,
    exercises: null,
    inChangedPackage: true,
    failure: null,
    ...overrides,
  };
}

const request = { id: "refund.Service.Request", qualifiedName: "refund.Service.Request" };
const validate = { id: "refund.Validate", qualifiedName: "refund.Validate" };
const repoGet = { id: "payment.Repo.Get", qualifiedName: "payment.Repo.Get" };

export const FAILURE_OUTPUT = [
  "--- FAIL: TestService_Request/gateway_declines (0.00s)",
  "    service_test.go:88: expected error wrapping ErrDeclined, got nil",
].join("\n");

/** Failed first, then by container and name, as the run lists them. */
export const refundTests: TestCase[] = [
  makeTestCase("TestService_Request/gateway_declines", {
    outcome: "fail",
    durationMs: 2,
    output: FAILURE_OUTPUT,
    line: 88,
    exercises: request,
  }),
  makeTestCase("TestService_Request/refunds_the_full_amount", { durationMs: 3, exercises: request, line: 40 }),
  makeTestCase("TestService_Request/payment_not_found", { durationMs: 1, exercises: request, line: 62 }),
  makeTestCase("TestValidate/rejects_a_zero_amount", {
    file: "internal/refund/validate_test.go",
    exercises: validate,
  }),
  makeTestCase("TestRefundIDs", { state: "untouched", file: "internal/refund/ids_test.go", line: 9 }),
  makeTestCase("TestRepo_Get/returns_the_refundable_balance", {
    container: "internal/payment",
    state: "modified",
    durationMs: 12,
    file: "internal/payment/repo_test.go",
    exercises: repoGet,
  }),
  makeTestCase("TestLedger_Append", {
    container: "internal/ledger",
    state: "untouched",
    file: "internal/ledger/ledger_test.go",
    inChangedPackage: false,
  }),
  makeTestCase("TestLedger_Balance", {
    container: "internal/ledger",
    state: "untouched",
    file: "internal/ledger/ledger_test.go",
    inChangedPackage: false,
  }),
];

export function makeTestRun(overrides: Partial<TestRun> = {}): TestRun {
  return {
    language: "go",
    runner: { name: "go test", version: "go1.23.4" },
    oldSha: "a1b2c3d4e5f6a7b8",
    newSha: "e7f3b10c9d8e7f6a",
    isolation: "sandbox",
    startedAt: "2026-11-17T10:00:00Z",
    durationMs: 1840,
    status: "completed",
    message: null,
    tests: refundTests,
    summary: { passed: 7, failed: 1, skipped: 0, added: 4 },
    packages: [
      { container: "internal/refund", outcome: "fail", durationMs: 900, output: null, failure: null, command: null },
      { container: "internal/payment", outcome: "pass", durationMs: 400, output: null, failure: null, command: null },
      { container: "internal/ledger", outcome: "pass", durationMs: 300, output: null, failure: null, command: null },
    ],
    coverage: {
      changedLines: { covered: 67, coverable: 100 },
      files: [
        { path: "internal/refund/handler.go", covered: 41, coverable: 50 },
        { path: "internal/refund/service.go", covered: 12, coverable: 25 },
        { path: "internal/payment/repo.go", covered: 14, coverable: 25 },
      ],
      project: { covered: 700, coverable: 1000 },
    },
    raceDetector: true,
    races: [],
    diagnostics: [],
    options: { race: true, coverage: true },
    partial: null,
    ...overrides,
  };
}

export const RACE_OUTPUT = [
  "=== RUN   TestSignUpFromEdge_CreatesAndLogsIn",
  "WARNING: DATA RACE",
  "Write at 0x00c0004b2a38 by goroutine 214:",
  "--- FAIL: TestSignUpFromEdge_CreatesAndLogsIn (1.84s)",
].join("\n");

const frame = (fn: string, file: string, line: number) => ({ function: fn, file, line, test: file.endsWith("_test.go") });

/**
 * A run with a data race in a new test with two subtests (app/controllers), an assertion in a changed test with one
 * of three subtests failing (app/services/auth_service), a package that did not build (app/containers) and one that
 * passed (app/billing).
 */
export function makeFailingRun(overrides: Partial<TestRun> = {}): TestRun {
  const controllers = { container: "app/controllers", file: "app/controllers/edge_login_test.go", line: 412, state: "new" as const };
  const auth = {
    container: "app/services/auth_service",
    file: "app/services/auth_service/auth_service_test.go",
    line: 196,
    state: "modified" as const,
  };
  const race = {
    kind: "race" as const,
    races: [
      {
        accesses: [
          {
            kind: "write" as const,
            previous: false,
            goroutine: 214,
            frame: frame("(*Service).recordLastLogin", "app/services/auth_service/service.go", 661),
            createdAt: frame("(*PasswordPolicy).checkAsync", "app/services/auth_service/password_policy.go", 90),
          },
          {
            kind: "read" as const,
            previous: true,
            goroutine: 7,
            frame: frame("TestSignUpFromEdge_CreatesAndLogsIn.func1", "app/controllers/edge_login_test.go", 437),
            createdAt: null,
          },
        ],
      },
    ],
  };
  const assertion = {
    kind: "assertion" as const,
    message: "Not equal",
    expected: '"credenciais inválidas"',
    actual: '"ldap: invalid credentials (49)"',
    at: { file: "app/services/auth_service/auth_service_test.go", line: 218 },
    test: "TestAuthenticateAD_EveryRefusalIsTheGenericError/AD_refuses",
  };
  const tests: TestCase[] = [
    makeTestCase("TestSignUpFromEdge_CreatesAndLogsIn", {
      ...controllers,
      outcome: "fail",
      durationMs: 1840,
      output: RACE_OUTPUT,
      failure: race,
    }),
    makeTestCase("TestSignUpFromEdge_CreatesAndLogsIn/v1,_the_country_of_the_edge", {
      ...controllers,
      outcome: "fail",
      durationMs: 920,
      output: RACE_OUTPUT,
      failure: race,
    }),
    makeTestCase("TestSignUpFromEdge_CreatesAndLogsIn/v2", { ...controllers, outcome: "fail", durationMs: 880, failure: race }),
    makeTestCase("TestLogin", { ...controllers, state: "untouched", durationMs: 30 }),
    makeTestCase("TestAuthenticateAD_EveryRefusalIsTheGenericError", {
      ...auth,
      outcome: "fail",
      durationMs: 40,
      output: "    auth_service_test.go:218:\n        Error: Not equal",
      failure: assertion,
    }),
    makeTestCase("TestAuthenticateAD_EveryRefusalIsTheGenericError/AD_refuses", { ...auth, outcome: "fail", durationMs: 10, failure: assertion }),
    makeTestCase("TestAuthenticateAD_EveryRefusalIsTheGenericError/AD_accepts", { ...auth, durationMs: 10 }),
    makeTestCase("TestAuthenticateAD_EveryRefusalIsTheGenericError/AD_unreachable", { ...auth, durationMs: 10 }),
    makeTestCase("TestRefreshToken_RotatesOnUse", { ...auth, state: "untouched", durationMs: 80 }),
    makeTestCase("TestAuthenticateAD_AgainstRealServer", { ...auth, state: "untouched", outcome: "skip", durationMs: 0 }),
    makeTestCase("TestCharge", { container: "app/billing", file: "app/billing/billing_test.go", state: "untouched" }),
  ];
  return makeTestRun({
    tests,
    summary: { passed: 5, failed: 5, skipped: 1, added: 3 },
    packages: [
      { container: "app/controllers", outcome: "fail", durationMs: 14_200, output: null, failure: null, command: null },
      {
        container: "app/services/auth_service",
        outcome: "fail",
        durationMs: 22_600,
        output: null,
        failure: null,
        command: null,
      },
      {
        container: "app/containers",
        outcome: "build_failed",
        durationMs: 0,
        command: null,
        output:
          "# app/containers [app/containers.test]\napp/containers/auth_container.go:47:31: not enough arguments in call to auth_service.NewService\n",
        failure: {
          kind: "build",
          errors: [
            {
              file: "app/containers/auth_container.go",
              line: 47,
              column: 31,
              message: "not enough arguments in call to auth_service.NewService",
              detail: ["have (*sql.DB, *ldap.Client)", "want (*sql.DB, *ldap.Client, auth_service.PasswordPolicy)"],
            },
          ],
        },
      },
      { container: "app/billing", outcome: "pass", durationMs: 9_400, output: null, failure: null, command: null },
    ],
    durationMs: 125_000,
    ...overrides,
  });
}

/** A run of `packages` packages of `perPackage` passing tests each. */
export function makeBigRun(packages: number, perPackage: number): TestRun {
  const tests: TestCase[] = [];
  for (let p = 0; p < packages; p++) {
    const container = `pkg/p${String(p).padStart(3, "0")}`;
    for (let n = 0; n < perPackage; n++) {
      tests.push(makeTestCase(`Test${String(n).padStart(4, "0")}`, { container, state: "untouched" }));
    }
  }
  return makeTestRun({
    tests,
    summary: { passed: tests.length, failed: 0, skipped: 0, added: 0 },
    packages: Array.from({ length: packages }, (_, p) => ({
      container: `pkg/p${String(p).padStart(3, "0")}`,
      outcome: "pass" as const,
      durationMs: 100,
      output: null,
      failure: null,
      command: null,
    })),
  });
}

export function makeTestsState(overrides: Partial<TestsState> = {}): TestsState {
  return { run: null, running: false, runningSelection: null, isolation: "sandbox", unavailable: null, ...overrides };
}

/**
 * A mutant of line 52 of refund.Service.Request that survived, `>=` into `>` in `if balance >= amount {`, unless told
 * otherwise.
 */
export function makeMutant(id: string, overrides: Partial<Mutant> = {}): Mutant {
  return {
    id,
    file: "internal/refund/service.go",
    line: 52,
    column: 13,
    endLine: 52,
    endColumn: 15,
    original: ">=",
    mutated: ">",
    operator: "relational_boundary",
    description: "replace >= with > in Service.Request",
    status: "survived",
    killedBy: null,
    durationMs: 1200,
    cached: false,
    function: { id: "refund.Service.Request", qualifiedName: "refund.Service.Request" },
    before: ["\tif balance >= amount {"],
    after: ["\tif balance > amount {"],
    ...overrides,
  };
}

/** A completed run of five mutants: two survived, one timed out, one killed by TestRequest, one error. */
export function makeMutationRun(overrides: Partial<MutationRun> = {}): MutationRun {
  const mutants = overrides.mutants ?? [
    makeMutant("m-1"),
    makeMutant("m-2", {
      line: 58,
      column: 3,
      endColumn: 19,
      original: "s.notify(refund)",
      mutated: "",
      operator: "call_removal",
      description: "remove call to s.notify",
      before: ["\t\ts.notify(refund)"],
      after: [""],
    }),
    makeMutant("m-3", {
      line: 64,
      column: 8,
      endColumn: 10,
      original: "<=",
      mutated: "<",
      status: "timeout",
      before: ["\tfor i <= n {"],
      after: ["\tfor i < n {"],
    }),
    makeMutant("m-4", {
      line: 66,
      column: 17,
      endColumn: 18,
      original: "+",
      mutated: "-",
      operator: "arithmetic",
      status: "killed",
      killedBy: "TestRequest",
      before: ["\ttotal := total + fee"],
      after: ["\ttotal := total - fee"],
    }),
    makeMutant("m-5", {
      line: 68,
      status: "error",
      operator: "return_value",
      original: "err",
      mutated: "nil",
      before: [],
      after: [],
    }),
  ];
  const count = (status: Mutant["status"]) => mutants.filter((mutant) => mutant.status === status).length;
  return {
    language: "go",
    runner: { name: "difflens-go", version: "0.1.0" },
    engine: { name: "difflens-go mutator", version: "0.1.0" },
    oldSha: "a1b2c3d4e5f6a7b8",
    newSha: "e7f3b10c9d8e7f6a",
    isolation: "sandbox",
    startedAt: "2026-11-17T10:30:00Z",
    durationMs: 133_000,
    status: "completed",
    message: null,
    coverageRunAt: "2026-11-17T10:00:00Z",
    mutants,
    summary: {
      candidates: mutants.length,
      sampled: false,
      maxMutants: 200,
      killed: count("killed"),
      survived: count("survived"),
      timeout: count("timeout"),
      error: count("error"),
      noCoverage: count("no_coverage"),
      cached: mutants.filter((mutant) => mutant.cached).length,
    },
    diagnostics: [],
    ...overrides,
  };
}

/** No run yet, none going on, and nothing in the way, unless told otherwise. */
export function makeMutationState(overrides: Partial<MutationState> = {}): MutationState {
  return { run: null, running: false, unavailable: null, maxMutants: 200, ...overrides };
}

export function makeQualityFunction(qualifiedName: string, overrides: Partial<QualityFunction> = {}): QualityFunction {
  const [, ...rest] = qualifiedName.split(".");
  return {
    id: qualifiedName,
    qualifiedName,
    name: rest.join("."),
    container: "internal/refund",
    file: "internal/refund/service.go",
    startLine: 44,
    endLine: (overrides.startLine ?? 44) + 27,
    state: "new",
    complexity: 4,
    previousComplexity: null,
    changedLines: { covered: 9, coverable: 10 },
    crap: 4,
    untested: false,
    code: [],
    ...overrides,
  };
}

export const refundFunctions: QualityFunction[] = [
  makeQualityFunction("refund.Validate", {
    file: "internal/refund/validate.go",
    startLine: 12,
    complexity: 12,
    changedLines: { covered: 16, coverable: 25 },
    crap: 21,
  }),
  makeQualityFunction("refund.Service.Request", {
    complexity: 11,
    changedLines: { covered: 12, coverable: 25 },
    crap: 34.2,
  }),
  makeQualityFunction("payment.Repo.Get", {
    container: "internal/payment",
    file: "internal/payment/repo.go",
    startLine: 27,
    state: "modified",
    complexity: 4,
    previousComplexity: 3,
  }),
];

export function makeCheck(tool: string, overrides: Partial<StaticCheck> = {}): StaticCheck {
  return { tool, status: "clean", message: null, findings: [], ...overrides };
}

/** A staticcheck SA4006 finding on line 50 of refund.Service.Request, unless told otherwise. */
export function makeLocatedCheck(overrides: Partial<LocatedCheck> = {}): LocatedCheck {
  return {
    tool: "staticcheck",
    rule: "SA4006",
    file: "internal/refund/service.go",
    line: 50,
    message: "this value of err is never used",
    class: "correctness",
    severity: "warning",
    function: { id: "refund.Service.Request", qualifiedName: "refund.Service.Request" },
    ...overrides,
  };
}

function codeLines(start: number, end: number, name: string): string[] {
  return Array.from({ length: end - start + 1 }, (_, index) => `\t${name}${start + index} := amount * ${index}`);
}

/**
 * Lines 60 to 67 of refund.Service.Request copying internal/payment/money.go:40-47, unless told otherwise; a
 * repeated_in_change one when `kind` says so, repeated in internal/refund/validate.go and internal/refund/notify.go.
 */
export function makeDuplicate(overrides: Partial<Duplicate> = {}): Duplicate {
  const kind = overrides.kind ?? "copies_existing";
  const block = { file: "internal/refund/service.go", startLine: 60, endLine: 67, code: codeLines(60, 67, "fee") };
  const others =
    kind === "copies_existing"
      ? [{ file: "internal/payment/money.go", startLine: 40, endLine: 47, code: codeLines(40, 47, "cents") }]
      : [
          { file: "internal/refund/validate.go", startLine: 20, endLine: 27, code: codeLines(20, 27, "fee") },
          { file: "internal/refund/notify.go", startLine: 5, endLine: 12, code: codeLines(5, 12, "fee") },
        ];
  return {
    kind,
    block,
    others,
    tokens: 74,
    lines: 8,
    function: { id: "refund.Service.Request", qualifiedName: "refund.Service.Request" },
    ...overrides,
  };
}

export function makeQualityReport(overrides: Partial<QualityReport> = {}): QualityReport {
  return {
    language: "go",
    oldSha: "a1b2c3d4e5f6a7b8",
    newSha: "e7f3b10c9d8e7f6a",
    limits: { complexity: 10, crap: 30, changedLinesCoverage: 0.8, duplicationTokens: 50, duplicationLines: 6 },
    functions: refundFunctions,
    crossed: [
      { kind: "complexity", function: { id: "refund.Validate", qualifiedName: "refund.Validate" }, value: 12, limit: 10 },
      {
        kind: "complexity",
        function: { id: "refund.Service.Request", qualifiedName: "refund.Service.Request" },
        value: 11,
        limit: 10,
      },
      {
        kind: "crap",
        function: { id: "refund.Service.Request", qualifiedName: "refund.Service.Request" },
        value: 34.2,
        limit: 30,
      },
      { kind: "changed_lines_coverage", function: null, value: 0.67, limit: 0.8 },
    ],
    checks: [makeCheck("go vet"), makeCheck("staticcheck")],
    checkFindings: [],
    duplicates: [],
    testDuplicates: [],
    raceDetector: "clean",
    testRunAt: "2026-11-17T10:00:00Z",
    coverage: { state: "measured", reasons: [] },
    diagnostics: [],
    ...overrides,
  };
}
