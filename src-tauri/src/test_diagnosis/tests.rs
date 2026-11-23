use std::collections::{HashMap, HashSet};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use serde_json::{json, Value};

use super::inputs::{self, Subject, MAX_CODE_BYTES, MAX_OUTPUT_BYTES, MAX_PIECES};
use super::service::SYSTEM_PROMPT;
use super::*;
use crate::ai::{ModelProvider, ModelRequest, ModelService};
use crate::analysis::Function;
use crate::error::{ErrorCode, Reason};
use crate::explanations::FileTexts;
use crate::language::Language;
use crate::model::{
    AnalyzerIdentity, BuildError, ChangeSet, DiagnosisConfidence, DiagnosisFault, FailureFrame,
    FunctionRef, Isolation, ModelProviderKind, ModelTask, PackageOutcome, PackageOutcomeKind,
    RaceAccess, RaceAccessKind, RaceSummary, ReviewRef, ReviewTarget, RunOptions, SourceRef,
    TestCase, TestFailure, TestFunctionState, TestOutcome, TestRun, TestRunStatus, TestSummary,
};
use crate::projects::ProjectService;
use crate::review_state::ReviewSession;
use crate::store::Store;
use crate::test_support::{isolated_git, ScriptedModel, TestRepo};

const GO_MOD: &str = "module example.com/app\n\ngo 1.23\n\ntoolchain go1.24.1\n";

const SERVICE_BEFORE: &str = "package auth\n\
    \n\
    type Service struct {\n\
    \tmu        sync.Mutex\n\
    \tlastLogin time.Time\n\
    }\n\
    \n\
    func (s *Service) recordLastLogin(at time.Time) {\n\
    \ts.lastLogin = at\n\
    }\n";

/// SignUp, new on the branch, records the login in a goroutine: lines 12-14.
const SERVICE_AFTER: &str = "package auth\n\
    \n\
    type Service struct {\n\
    \tmu        sync.Mutex\n\
    \tlastLogin time.Time\n\
    }\n\
    \n\
    func (s *Service) recordLastLogin(at time.Time) {\n\
    \ts.lastLogin = at\n\
    }\n\
    \n\
    func (s *Service) SignUp() {\n\
    \tgo s.recordLastLogin(time.Now())\n\
    }\n";

const SERVICE_TEST: &str = "package auth\n\
    \n\
    func TestSignUp(t *testing.T) {\n\
    \ts := &Service{}\n\
    \ts.SignUp()\n\
    \tif s.lastLogin.IsZero() {\n\
    \t\tt.Fatal(\"no login\")\n\
    \t}\n\
    }\n";

const SERVICE: &str = "app/auth/service.go";
const SERVICE_TEST_FILE: &str = "app/auth/service_test.go";

fn function(id: &str, name: &str, file: &str, lines: (u32, u32), test: bool) -> Function {
    Function {
        id: id.to_string(),
        name: name.to_string(),
        qualified_name: name.to_string(),
        container: "app/auth".to_string(),
        file: file.to_string(),
        start_line: lines.0,
        end_line: lines.1,
        generated: false,
        test,
        complexity: None,
        entry_points: Vec::new(),
    }
}

/// The new side's functions.
fn functions() -> Vec<Function> {
    vec![
        function("auth.Service.recordLastLogin", "(*Service).recordLastLogin", SERVICE, (8, 10), false),
        function("auth.Service.SignUp", "(*Service).SignUp", SERVICE, (12, 14), false),
        function("auth.TestSignUp", "TestSignUp", SERVICE_TEST_FILE, (3, 9), true),
    ]
}

fn frame(function: &str, file: &str, line: u32, test: bool) -> FailureFrame {
    FailureFrame {
        function: function.to_string(),
        file: file.to_string(),
        line,
        test,
    }
}

/// The write in the goroutine SignUp starts, then the test's read.
fn race(write_line: u32, goroutines: (u32, u32)) -> TestFailure {
    TestFailure::Race {
        races: vec![RaceSummary {
            accesses: vec![
                RaceAccess {
                    kind: RaceAccessKind::Write,
                    previous: false,
                    goroutine: Some(goroutines.0),
                    frame: Some(frame("(*Service).recordLastLogin", SERVICE, write_line, false)),
                    created_at: Some(frame("(*Service).SignUp", SERVICE, 13, false)),
                },
                RaceAccess {
                    kind: RaceAccessKind::Read,
                    previous: true,
                    goroutine: Some(goroutines.1),
                    frame: Some(frame("TestSignUp", SERVICE_TEST_FILE, 6, true)),
                    created_at: None,
                },
            ],
        }],
    }
}

fn case(name: &str, outcome: TestOutcome, failure: Option<TestFailure>, output: &str) -> TestCase {
    TestCase {
        id: format!("app/auth::{name}"),
        container: "app/auth".to_string(),
        package: "app/auth".to_string(),
        name: name.to_string(),
        hierarchy: name.split('/').map(str::to_string).collect(),
        title: name.to_string(),
        command: None,
        function_id: Some("auth.TestSignUp".to_string()),
        state: TestFunctionState::Untouched,
        outcome,
        duration_ms: 40,
        output: (!output.is_empty()).then(|| output.to_string()),
        file: Some(SERVICE_TEST_FILE.to_string()),
        line: Some(3),
        exercises: Some(FunctionRef {
            id: "auth.Service.SignUp".to_string(),
            qualified_name: "(*Service).SignUp".to_string(),
        }),
        in_changed_package: true,
        failure,
    }
}

fn run(tests: Vec<TestCase>, packages: Vec<PackageOutcome>) -> TestRun {
    TestRun {
        language: "go".to_string(),
        runner: AnalyzerIdentity {
            name: "difflens-go".to_string(),
            version: "0.3.0".to_string(),
        },
        old_sha: "a".repeat(40),
        new_sha: Some("b".repeat(40)),
        isolation: Isolation::Sandbox,
        started_at: "2026-11-21T09:00:00Z".to_string(),
        duration_ms: 1_840,
        status: TestRunStatus::Completed,
        message: None,
        tests,
        summary: TestSummary::default(),
        packages,
        coverage: None,
        race_detector: true,
        races: Vec::new(),
        diagnostics: Vec::new(),
        options: RunOptions::default(),
        partial: None,
    }
}

fn race_output(goroutine: u32, address: &str, duration: &str) -> String {
    format!(
        "=== RUN   TestSignUp\n==================\nWARNING: DATA RACE\nWrite at {address} by \
         goroutine {goroutine}:\n  example.com/app/app/auth.(*Service).recordLastLogin()\n      \
         /work/tree/app/auth/service.go:9 +0x84\n==================\n    testing.go:1490: race \
         detected during execution of test\n--- FAIL: TestSignUp ({duration})\n"
    )
}

/// TestSignUp failing with the data race.
fn race_run(write_line: u32, goroutines: (u32, u32), address: &str, duration: &str) -> TestRun {
    run(
        vec![case(
            "TestSignUp",
            TestOutcome::Fail,
            Some(race(write_line, goroutines)),
            &race_output(goroutines.0, address, duration),
        )],
        vec![package("app/auth", PackageOutcomeKind::Fail, None, None)],
    )
}

fn package(
    name: &str,
    outcome: PackageOutcomeKind,
    output: Option<&str>,
    failure: Option<TestFailure>,
) -> PackageOutcome {
    PackageOutcome {
        container: name.to_string(),
        outcome,
        duration_ms: 0,
        output: output.map(str::to_string),
        failure,
        command: None,
    }
}

fn test_subject() -> Subject {
    Subject {
        package: "app/auth".to_string(),
        test: Some("TestSignUp".to_string()),
    }
}

/// The files as the review reads them: the service changed, its test not.
fn texts() -> HashMap<String, FileTexts> {
    let mut texts = HashMap::new();
    texts.insert(
        SERVICE.to_string(),
        FileTexts {
            old: Some(SERVICE_BEFORE.to_string()),
            new: Some(SERVICE_AFTER.to_string()),
        },
    );
    texts.insert(
        SERVICE_TEST_FILE.to_string(),
        FileTexts {
            old: Some(SERVICE_TEST.to_string()),
            new: Some(SERVICE_TEST.to_string()),
        },
    );
    texts.insert(
        "go.mod".to_string(),
        FileTexts {
            old: Some(GO_MOD.to_string()),
            new: Some(GO_MOD.to_string()),
        },
    );
    texts
}

fn changed() -> HashSet<String> {
    HashSet::from([SERVICE.to_string()])
}

fn branch() -> ReviewTarget {
    ReviewTarget::Branch {
        branch: "feature/AUTH-12".to_string(),
        base: "main".to_string(),
    }
}

/// How many pieces of code a call sends.
fn pieces(code: &inputs::Code) -> usize {
    code.text.lines().filter(|line| line.starts_with("=== ")).count()
}

fn prepared(run: &TestRun, subject: &Subject) -> inputs::Prepared {
    let found = inputs::find(run, subject).expect("the failure is in the run");
    inputs::prepare(&found, run, &functions(), &texts(), &changed(), Some(&branch()))
}

#[test]
fn a_failure_is_found_as_the_tests_tab_shows_it() {
    let failing = race_run(9, (214, 7), "0x00c0004b2a38", "1.84s");
    let found = inputs::find(&failing, &test_subject()).unwrap();
    assert_eq!(found.subject.key(), "app/auth::TestSignUp");
    assert!(matches!(found.failure, Some(TestFailure::Race { .. })));

    // Only its subtests were reported: the first one that failed says why.
    let assertion = TestFailure::Assertion {
        message: "Not equal".to_string(),
        expected: Some("1".to_string()),
        actual: Some("2".to_string()),
        at: None,
        test: Some("TestSignUp/b".to_string()),
    };
    let subtests = run(
        vec![
            case("TestSignUp/c", TestOutcome::Pass, None, ""),
            case("TestSignUp/b", TestOutcome::Fail, Some(assertion.clone()), "b failed"),
            case("TestSignUp/a", TestOutcome::Fail, None, "a failed"),
        ],
        Vec::new(),
    );
    let found = inputs::find(&subtests, &test_subject()).unwrap();
    assert!(found.own.is_none());
    let names: Vec<&str> = found.failed_subtests.iter().map(|sub| sub.name.as_str()).collect();
    assert_eq!(names, ["TestSignUp/a", "TestSignUp/b"]);
    assert_eq!(found.failure, Some(&assertion));

    // Passing, gone, or a run that did not complete: nothing to explain.
    let passing = run(vec![case("TestSignUp", TestOutcome::Pass, None, "")], Vec::new());
    assert!(inputs::find(&passing, &test_subject()).is_none());
    assert!(inputs::find(&run(Vec::new(), Vec::new()), &test_subject()).is_none());
    let mut cancelled = race_run(9, (214, 7), "0x1", "1s");
    cancelled.status = TestRunStatus::Cancelled;
    assert!(inputs::find(&cancelled, &test_subject()).is_none());

    // A package: only one that did not build or failed outside its tests.
    let packages = run(
        Vec::new(),
        vec![
            package("app/containers", PackageOutcomeKind::BuildFailed, Some("# app/containers"), None),
            package("app/auth", PackageOutcomeKind::Fail, None, None),
        ],
    );
    let containers = Subject::stored("app/containers", "");
    assert_eq!(containers.test, None);
    assert_eq!(containers.key(), "app/containers::");
    assert!(inputs::find(&packages, &containers).is_some());
    assert!(inputs::find(&packages, &Subject::stored("app/auth", "")).is_none());
}

#[test]
fn a_data_race_sends_both_accesses_where_their_goroutines_started_and_the_code_around_them() {
    let run = race_run(9, (214, 7), "0x00c0004b2a38", "1.84s");
    let prepared = prepared(&run, &test_subject());
    let prompt = &prepared.prompt;
    assert!(prompt.starts_with("Explain why this test failed, briefly."), "{prompt}");
    assert!(prompt.contains("branch feature/AUTH-12 compared with main"));
    assert!(prompt.contains("Test: TestSignUp, in package app/auth."));
    assert!(prompt.contains("Kind: data race"));
    assert!(prompt.contains(
        "- Write: (*Service).recordLastLogin at app/auth/service.go:9, by goroutine 214, started \
         by (*Service).SignUp at app/auth/service.go:13"
    ));
    assert!(prompt.contains("- Earlier read: TestSignUp at app/auth/service_test.go:6, by goroutine 7"));
    // How it ran: no network, the race detector, the Go version.
    assert!(prompt.contains("with the network off"));
    assert!(prompt.contains("The race detector was on."));
    assert!(prompt.contains("Go version: go.mod asks for go 1.23, toolchain go1.24.1."));
    // The test function, then the functions of both accesses (the goroutine's
    // start is inside SignUp, the function under test, sent once).
    let test = prompt.find("=== the test function: TestSignUp in app/auth/service_test.go, lines 3-9").unwrap();
    let write = prompt
        .find("=== at the failure (the write of the data race): (*Service).recordLastLogin in app/auth/service.go, lines 8-10 (this change edits this file)")
        .unwrap();
    let start = prompt
        .find("=== at the failure (where the goroutine of the write of the data race was started): (*Service).SignUp in app/auth/service.go, lines 12-14")
        .unwrap();
    assert!(test < write && write < start, "{prompt}");
    assert_eq!(pieces(&prepared.code), 3, "{prompt}");
    // The review's diff of the changed file: SignUp is new.
    assert!(prompt.contains("+    13 | \tgo s.recordLastLogin(time.Now())"));
    assert!(prompt.contains("      9 | \ts.lastLogin = at"));
    assert!(prompt.contains("      6 | \tif s.lastLogin.IsZero() {"));
    // The output, as data.
    assert!(prompt.contains("<untrusted_output>\n--- output of TestSignUp:\n=== RUN   TestSignUp"));
    assert!(prompt.contains("- app/auth/service.go: lines 8-10, 12-14"));
    assert!(prompt.contains("- app/auth/service_test.go: lines 3-9"));
    assert_eq!(
        prepared.schema["properties"]["where"]["properties"]["path"]["enum"],
        json!([SERVICE, SERVICE_TEST_FILE, ""])
    );
}

#[test]
fn an_assertion_sends_what_was_expected_and_what_came_and_the_failed_subtests() {
    let failure = TestFailure::Assertion {
        message: "Not equal".to_string(),
        expected: Some("\"credenciais inválidas\"".to_string()),
        actual: Some("\"ldap: invalid credentials (49)\"".to_string()),
        at: Some(SourceRef {
            file: SERVICE_TEST_FILE.to_string(),
            line: 7,
        }),
        test: Some("TestSignUp/AD_refuses".to_string()),
    };
    let run = run(
        vec![
            case("TestSignUp", TestOutcome::Fail, None, "--- FAIL: TestSignUp (0.04s)"),
            case("TestSignUp/AD_accepts", TestOutcome::Pass, None, ""),
            case(
                "TestSignUp/AD_refuses",
                TestOutcome::Fail,
                Some(failure),
                "    service_test.go:7:\n        \tError:      \tNot equal:",
            ),
        ],
        Vec::new(),
    );
    let prompt = prepared(&run, &test_subject()).prompt;
    assert!(prompt.contains("Test: TestSignUp, in package app/auth. Failed subtests: AD_refuses."));
    assert!(prompt.contains("Kind: an assertion failed.\nIn: TestSignUp/AD_refuses\nMessage: Not equal\n"));
    assert!(prompt.contains("Expected: \"credenciais inválidas\"\nActual: \"ldap: invalid credentials (49)\"\nAt: app/auth/service_test.go:7"));
    // The assertion is inside the test function: sent once.
    assert_eq!(prompt.matches("service_test.go, lines").count(), 1, "{prompt}");
    assert!(prompt.contains("--- output of TestSignUp/AD_refuses:\n    service_test.go:7:"));
    assert!(!prompt.contains("AD_accepts"));
}

#[test]
fn a_package_that_did_not_build_sends_the_compiler_errors_and_the_code_they_name() {
    const CONTAINER: &str = "package containers\n\
        \n\
        func Build(db *sql.DB) *auth.Service {\n\
        \tsvc := auth.NewService(db)\n\
        \treturn svc\n\
        }\n";
    let errors = vec![BuildError {
        file: Some("app/containers/auth_container.go".to_string()),
        line: Some(4),
        column: Some(31),
        message: "not enough arguments in call to auth.NewService".to_string(),
        detail: vec!["have (*sql.DB)".to_string(), "want (*sql.DB, auth.Policy)".to_string()],
    }];
    let output = "# app/containers\napp/containers/auth_container.go:4:31: not enough arguments \
                  in call to auth.NewService\n\thave (*sql.DB)\n\twant (*sql.DB, auth.Policy)\n";
    let run = run(
        Vec::new(),
        vec![package(
            "app/containers",
            PackageOutcomeKind::BuildFailed,
            Some(output),
            Some(TestFailure::Build { errors }),
        )],
    );
    let subject = Subject::stored("app/containers", "");
    let found = inputs::find(&run, &subject).unwrap();
    let mut texts = texts();
    texts.insert(
        "app/containers/auth_container.go".to_string(),
        FileTexts {
            old: Some(CONTAINER.to_string()),
            new: Some(CONTAINER.to_string()),
        },
    );
    // No analysis of the package: a window of lines around the error.
    let prepared = inputs::prepare(&found, &run, &[], &texts, &changed(), Some(&branch()));
    let prompt = &prepared.prompt;
    assert!(prompt.starts_with("Explain why this package failed, briefly."));
    assert!(prompt.contains("Package app/containers did not compile; none of its tests ran."));
    assert!(prompt.contains(
        "app/containers/auth_container.go:4:31: not enough arguments in call to auth.NewService\n    have (*sql.DB)\n    want (*sql.DB, auth.Policy)"
    ));
    assert!(prompt.contains(
        "=== at the failure (a compiler error): in app/containers/auth_container.go, lines 1-6 (this change does not edit this file) ==="
    ));
    assert!(prompt.contains("      4 | \tsvc := auth.NewService(db)"));
    assert!(prompt.contains("--- output of package app/containers:\n# app/containers"));
    assert!(prepared.code.allows("app/containers/auth_container.go", 4));
}

#[test]
fn what_a_call_sends_is_capped() {
    let long_test: String = std::iter::once("package auth\n\nfunc TestSignUp(t *testing.T) {\n".to_string())
        .chain((0..400).map(|n| format!("\tstep({n})\n")))
        .chain(std::iter::once("}\n".to_string()))
        .collect();
    let mut texts = texts();
    texts.insert(
        SERVICE_TEST_FILE.to_string(),
        FileTexts {
            old: Some(long_test.clone()),
            new: Some(long_test),
        },
    );
    let mut functions = functions();
    functions[2].end_line = 404;
    // A huge output, and many build errors in files no function holds.
    let output = format!("{}\nTHE LAST LINE", "noise\n".repeat(20_000));
    let mut failing = case("TestSignUp", TestOutcome::Fail, None, &output);
    failing.failure = Some(TestFailure::Build {
        errors: (0..15)
            .map(|n| BuildError {
                file: Some(format!("app/gen/file{n}.go")),
                line: Some(3),
                column: None,
                message: format!("error {n}"),
                detail: Vec::new(),
            })
            .collect(),
    });
    for n in 0..15 {
        let filler = format!("\t// {}\n", "a long line of a generated file ".repeat(3));
        let big = format!("package gen\nfunc Generated{n}() {{\n{}}}\n", filler.repeat(400));
        texts.insert(
            format!("app/gen/file{n}.go"),
            FileTexts {
                old: Some(big.clone()),
                new: Some(big),
            },
        );
        functions.push(function(
            &format!("gen.Generated{n}"),
            &format!("Generated{n}"),
            &format!("app/gen/file{n}.go"),
            (2, 403),
            false,
        ));
    }
    let run = run(vec![failing], Vec::new());
    let found = inputs::find(&run, &test_subject()).unwrap();
    let prepared = inputs::prepare(&found, &run, &functions, &texts, &changed(), Some(&branch()));
    let prompt = &prepared.prompt;
    // The test function stops at its 160th line.
    assert!(prompt.contains("[… its last 242 lines are left out for length]"), "{prompt}");
    // At most 10 errors, and at most MAX_PIECES pieces of code.
    assert!(prompt.contains("error 9\n[5 more errors are left out]"));
    assert!(pieces(&prepared.code) <= MAX_PIECES);
    // Each piece stops at its 100th line, and the code as a whole at its cap.
    assert!(prompt.contains("=== at the failure (a compiler error): Generated0 in app/gen/file0.go, lines 2-403"));
    assert!(prompt.contains("[… its last 302 lines are left out for length]"));
    assert!(prompt.contains("more pieces of code are left out for length"), "{prompt}");
    assert!(prepared.code.text.len() <= MAX_CODE_BYTES + 100);
    // The end of the output, no more than its cap.
    let output_part = prompt.split("<untrusted_output>\n").nth(1).unwrap();
    let output_part = output_part.split("\n</untrusted_output>").next().unwrap();
    assert!(output_part.starts_with("[… the start is left out for length]"));
    assert!(output_part.ends_with("THE LAST LINE"));
    assert!(output_part.len() <= MAX_OUTPUT_BYTES + 64);
}

#[test]
fn the_schema_is_strict() {
    let prepared = prepared(&race_run(9, (1, 2), "0x1", "1s"), &test_subject());
    let schema = &prepared.schema;
    assert_eq!(schema["additionalProperties"], json!(false));
    assert_eq!(
        schema["required"],
        json!(["cause", "fault", "confidence", "where", "fix", "fix_code"])
    );
    assert_eq!(
        schema["properties"]["fault"]["enum"],
        json!(["code", "test", "flaky", "environment"])
    );
    assert_eq!(schema["properties"]["confidence"]["enum"], json!(["high", "medium", "low"]));
    let place = &schema["properties"]["where"];
    assert_eq!(place["additionalProperties"], json!(false));
    assert_eq!(place["required"], json!(["path", "line"]));
    assert_eq!(place["properties"]["line"]["type"], "integer");
}

fn answer(path: &str, line: u32) -> Value {
    json!({
        "cause": " `SignUp` starts a goroutine that writes `lastLogin` while the test reads it. ",
        "fault": "code",
        "confidence": "high",
        "where": { "path": path, "line": line },
        "fix": "Hold `s.mu` in `recordLastLogin`.",
        "fix_code": "```go\ns.mu.Lock()\ndefer s.mu.Unlock()\ns.lastLogin = at\n```\n"
    })
}

#[test]
fn an_answer_is_read_back_and_points_only_at_code_it_was_shown() {
    let prepared = prepared(&race_run(9, (1, 2), "0x1", "1s"), &test_subject());
    let read = inputs::parse(&answer(SERVICE, 9), &prepared.code).unwrap();
    assert_eq!(
        read,
        inputs::Answer {
            cause: "`SignUp` starts a goroutine that writes `lastLogin` while the test reads it."
                .to_string(),
            fault: DiagnosisFault::Code,
            confidence: DiagnosisConfidence::High,
            location: Some(SourceRef {
                file: SERVICE.to_string(),
                line: 9
            }),
            fix: "Hold `s.mu` in `recordLastLogin`.".to_string(),
            fix_code: "s.mu.Lock()\ndefer s.mu.Unlock()\ns.lastLogin = at".to_string(),
        }
    );
    // A line it was not shown (the struct), or no place: no location.
    assert_eq!(inputs::parse(&answer(SERVICE, 4), &prepared.code).unwrap().location, None);
    assert_eq!(inputs::parse(&answer("", 0), &prepared.code).unwrap().location, None);
    assert_eq!(inputs::parse(&answer("elsewhere.go", 9), &prepared.code).unwrap().location, None);
    // An unknown confidence reads as low; an unknown fault or no cause fails.
    let mut unsure = answer(SERVICE, 9);
    unsure["confidence"] = json!("somewhat");
    assert_eq!(
        inputs::parse(&unsure, &prepared.code).unwrap().confidence,
        DiagnosisConfidence::Low
    );
    let mut fault = answer(SERVICE, 9);
    fault["fault"] = json!("cosmic rays");
    let error = inputs::parse(&fault, &prepared.code).unwrap_err();
    assert_eq!(error.reason, Some(Reason::DiagnosisUnknownFault));
    let mut silent = answer(SERVICE, 9);
    silent["cause"] = json!("  ");
    assert_eq!(
        inputs::parse(&silent, &prepared.code).unwrap_err().reason,
        Some(Reason::DiagnosisNoCause)
    );
}

#[test]
fn the_same_failure_in_another_run_has_the_same_fingerprint() {
    let first = prepared(&race_run(9, (214, 7), "0x00c0004b2a38", "1.84s"), &test_subject());
    let again = prepared(&race_run(9, (301, 12), "0x00c000aa0010", "2.10s"), &test_subject());
    assert_eq!(first.failure_fingerprint, again.failure_fingerprint);
    assert_eq!(first.code_fingerprint, again.code_fingerprint);
    // The race at another line is another failure.
    let other = prepared(&race_run(13, (214, 7), "0x00c0004b2a38", "1.84s"), &test_subject());
    assert_ne!(first.failure_fingerprint, other.failure_fingerprint);
    // Another expected value, or another failed subtest, too.
    let assertion = |expected: &str| TestFailure::Assertion {
        message: "Not equal (0.04s) at 0xdeadbeef".to_string(),
        expected: Some(expected.to_string()),
        actual: Some("2".to_string()),
        at: None,
        test: None,
    };
    let fingerprint = |failure: TestFailure, subtests: &[&str]| {
        let mut tests = vec![case("TestSignUp", TestOutcome::Fail, Some(failure), "x")];
        for name in subtests {
            tests.push(case(&format!("TestSignUp/{name}"), TestOutcome::Fail, None, ""));
        }
        let run = run(tests, Vec::new());
        inputs::failure_fingerprint(&inputs::find(&run, &test_subject()).unwrap())
    };
    assert_eq!(fingerprint(assertion("1"), &["a"]), fingerprint(assertion("1"), &["a"]));
    assert_ne!(fingerprint(assertion("1"), &["a"]), fingerprint(assertion("3"), &["a"]));
    assert_ne!(fingerprint(assertion("1"), &["a"]), fingerprint(assertion("1"), &["a", "b"]));
}

#[test]
fn the_go_version_is_read_from_go_mod() {
    assert_eq!(
        inputs::go_version("module x\n\ngo 1.23.4\n").as_deref(),
        Some("go.mod asks for go 1.23.4")
    );
    assert_eq!(inputs::go_version("module x\n"), None);
}

#[test]
fn each_language_reads_its_own_manifests_from_the_test_folder_up() {
    let failing = || {
        let assertion = TestFailure::Assertion {
            message: "boom".to_string(),
            expected: None,
            actual: None,
            at: None,
            test: Some("TestSignUp".to_string()),
        };
        vec![case("TestSignUp", TestOutcome::Fail, Some(assertion), "boom")]
    };
    let with = |language: &str, files: &[(&str, &str)]| {
        let mut run = run(failing(), Vec::new());
        run.language = language.to_string();
        let found = inputs::find(&run, &test_subject()).unwrap();
        let wanted = inputs::wanted_paths(&found, &functions(), language);
        let mut texts = texts();
        for (path, text) in files {
            assert!(wanted.contains(*path), "{path} is not asked for: {wanted:?}");
            texts.insert(
                path.to_string(),
                FileTexts {
                    old: None,
                    new: Some(text.to_string()),
                },
            );
        }
        let prompt = inputs::prepare(&found, &run, &functions(), &texts, &changed(), None).prompt;
        let line = prompt
            .lines()
            .find(|line| line.contains("setup:") || line.contains("toolchain:") || line.contains("Go version:"))
            .unwrap_or_default()
            .to_string();
        (wanted, line)
    };

    let (wanted, line) = with(
        "rust",
        &[
            (
                "app/Cargo.toml",
                "[package]\nname = \"auth\"\nedition = \"2021\"\nrust-version = \"1.75\" # MSRV\n\n\
                 [dev-dependencies]\nproptest = \"1\"\n\n[dev-dependencies.tokio]\nversion = \"1\"\n",
            ),
            ("rust-toolchain.toml", "[toolchain]\nchannel = \"1.80.0\"\n"),
        ],
    );
    assert!(wanted.contains("app/auth/Cargo.toml") && wanted.contains("Cargo.toml"));
    assert!(!wanted.contains("go.mod"), "{wanted:?}");
    assert_eq!(
        line,
        "- Rust toolchain: app/Cargo.toml asks for edition 2021, rust-version 1.75; dev-dependencies: \
         proptest, tokio; rust-toolchain.toml pins channel 1.80.0."
    );

    let (_, line) = with(
        "python",
        &[
            (
                "pyproject.toml",
                "[project]\nname = \"auth\"\nrequires-python = \">=3.10\"\n\
                 [project.optional-dependencies]\ntest = [\"pytest>=8\", \"requests\"]\n",
            ),
            ("app/requirements-dev.txt", "pytest-asyncio==0.23.7\n# a comment\nhypothesis\n"),
            (".python-version", "3.12.4\n"),
        ],
    );
    assert_eq!(
        line,
        "- Python setup: pyproject.toml asks for Python >=3.10; .python-version pins 3.12.4; \
         test tools: pytest-asyncio==0.23.7, hypothesis, pytest>=8."
    );

    let (_, line) = with(
        "typescript",
        &[(
            "app/package.json",
            r#"{ "type": "module", "engines": { "node": ">=20" }, "scripts": { "test": "vitest run" },
                "devDependencies": { "vitest": "^1.6.0", "typescript": "5.4.5", "left-pad": "1" } }"#,
        )],
    );
    assert_eq!(
        line,
        "- Node setup: app/package.json asks for node >=20; \"type\": \"module\"; test script: vitest run; \
         test tools: vitest ^1.6.0, typescript 5.4.5."
    );

    let (_, line) = with("javascript", &[]);
    assert_eq!(line, "- Node setup: unknown.");
    let (wanted, line) = with("go", &[]);
    assert!(wanted.contains("go.mod") && wanted.contains("app/auth/go.mod"));
    assert_eq!(line, "- Go version: go.mod asks for go 1.23, toolchain go1.24.1.");
}

// ---------- the service ----------

struct Fixture {
    repo: TestRepo,
    projects: ProjectService,
    id: String,
}

impl Fixture {
    fn new() -> Self {
        Self::with_store(Store::open_in_memory().unwrap())
    }

    fn with_store(store: Store) -> Self {
        let repo = TestRepo::init();
        repo.write("go.mod", GO_MOD);
        repo.write(SERVICE, SERVICE_BEFORE);
        repo.write(SERVICE_TEST_FILE, SERVICE_TEST);
        repo.commit_all("initial commit");
        repo.git(&["checkout", "--quiet", "-b", "feature/AUTH-12"]);
        repo.write(SERVICE, SERVICE_AFTER);
        repo.commit_all("Record the login in the background");
        let projects = ProjectService::new(store, isolated_git(), None);
        let id = projects
            .add_local(repo.path().to_str().unwrap())
            .unwrap()
            .id;
        Self { repo, projects, id }
    }

    fn review(&self) -> ReviewRef {
        let ChangeSet {
            target,
            old_sha,
            new_sha,
            ..
        } = self.projects.changed_files(&self.id, branch()).unwrap();
        ReviewRef {
            target,
            old_sha,
            new_sha,
        }
    }

    fn session(&self) -> ReviewSession<'_> {
        ReviewSession::new(
            self.projects.store(),
            self.projects.git(),
            &self.id,
            self.repo.path(),
            self.review(),
        )
        .unwrap()
    }

    fn diagnose(
        &self,
        models: &ModelService,
        run: &TestRun,
        language: Language,
        run_id: &str,
    ) -> crate::error::AppResult<crate::model::TestDiagnosis> {
        let functions = functions();
        diagnose(
            &self.session(),
            models,
            Facts {
                run: Some(run),
                functions: &functions,
                kept: None,
            },
            &test_subject(),
            language,
            run_id,
            Arc::new(AtomicBool::new(false)),
        )
    }

    fn read(&self, run: Option<&TestRun>) -> Vec<crate::model::TestDiagnosis> {
        let functions = functions();
        diagnoses(
            &self.session(),
            Facts {
                run,
                functions: &functions,
                kept: None,
            },
        )
        .unwrap()
    }
}

/// Explains every failure as a race in the code, or translates into
/// Portuguese.
fn answering(provider: ModelProviderKind, model: &str) -> Arc<ScriptedModel> {
    ScriptedModel::writing_as(provider, model, |request: &ModelRequest| {
        if request.schema_name == "translated_failure_explanations" {
            let ids = request.schema["properties"]["failures"]["items"]["properties"]["id"]["enum"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            let failures: Vec<Value> = ids
                .iter()
                .map(|id| {
                    json!({
                        "id": id,
                        "cause": "`SignUp` inicia uma goroutine que grava `lastLogin` enquanto o teste lê.",
                        "fix": "Segure `s.mu` em `recordLastLogin`."
                    })
                })
                .collect();
            return Some(Ok(json!({ "failures": failures })));
        }
        Some(Ok(answer(SERVICE, 9)))
    })
}

#[test]
fn a_click_explains_a_failure_with_the_model_of_its_task_and_keeps_it() {
    let fixture = Fixture::new();
    let default = answering(ModelProviderKind::Anthropic, "claude-opus-5-5-20261101");
    let diagnostician = answering(ModelProviderKind::OpenAi, "gpt-5.5");
    let models = default.service().with_task(
        ModelTask::TestDiagnosis,
        Arc::clone(&diagnostician) as Arc<dyn ModelProvider>,
        ModelProviderKind::OpenAi,
        "gpt-5.5",
    );
    let run = race_run(9, (214, 7), "0x00c0004b2a38", "1.84s");
    assert!(fixture.read(Some(&run)).is_empty());

    let written = fixture
        .diagnose(&models, &run, Language::BrazilianPortuguese, "diagnose-1")
        .unwrap();
    // Asked once, of the task's own model, never the default.
    assert!(default.asked().is_empty());
    let asked = diagnostician.asked();
    assert_eq!(asked.len(), 1);
    assert_eq!(asked[0].schema_name, "test_failure_explanation");
    assert_eq!(asked[0].system, SYSTEM_PROMPT);
    assert!(asked[0]
        .prompt
        .starts_with("Write every prose field of your answer in Brazilian Portuguese.\n\nExplain why this test failed"));
    assert!(asked[0].prompt.contains("+    13 | \tgo s.recordLastLogin(time.Now())"));

    assert_eq!(written.key, "app/auth::TestSignUp");
    assert_eq!(written.test.as_deref(), Some("TestSignUp"));
    assert_eq!(written.fault, DiagnosisFault::Code);
    assert_eq!(written.language, Language::BrazilianPortuguese);
    assert_eq!(
        (written.provider, written.model.as_str()),
        (ModelProviderKind::OpenAi, "gpt-5.5")
    );
    assert_eq!(written.run_started_at, "2026-11-21T09:00:00Z");
    assert_eq!(
        written.location,
        Some(SourceRef {
            file: SERVICE.to_string(),
            line: 9
        })
    );
    assert!(!written.outdated);
    // Read again (as on opening the tab): the same.
    assert_eq!(fixture.read(Some(&run)), vec![written.clone()]);

    // Explain again replaces it.
    let again = fixture
        .diagnose(&models, &run, Language::English, "diagnose-2")
        .unwrap();
    assert_eq!(again.language, Language::English);
    assert_eq!(fixture.read(Some(&run)), vec![again]);
}

#[test]
fn an_explanation_is_outdated_once_the_failure_or_its_code_changes() {
    let fixture = Fixture::new();
    let model = answering(ModelProviderKind::Anthropic, "claude-opus-5-5");
    let models = model.service();
    let run = race_run(9, (214, 7), "0x00c0004b2a38", "1.84s");
    fixture
        .diagnose(&models, &run, Language::English, "diagnose-1")
        .unwrap();

    // Run again, the same race with other goroutines and addresses: current.
    let same = race_run(9, (301, 12), "0x00c000aa0010", "2.10s");
    assert!(!fixture.read(Some(&same))[0].outdated);
    // Another race: outdated.
    let other = race_run(13, (214, 7), "0x00c0004b2a38", "1.84s");
    assert!(fixture.read(Some(&other))[0].outdated);
    // Passing now, or no run: outdated (and no card shows it).
    let passing = run_of_passing();
    assert!(fixture.read(Some(&passing))[0].outdated);
    assert!(fixture.read(None)[0].outdated);

    // The code it was shown changed: a new commit on the branch.
    fixture.repo.write(SERVICE, SERVICE_AFTER.replace("s.lastLogin = at", "s.lastLogin = at.UTC()"));
    fixture.repo.commit_all("Keep logins in UTC");
    assert!(fixture.read(Some(&same))[0].outdated);
}

#[test]
fn reading_again_for_the_same_run_and_sides_assembles_no_code() {
    let fixture = Fixture::new();
    let model = answering(ModelProviderKind::Anthropic, "claude-opus-5-5");
    let run = race_run(9, (214, 7), "0x00c0004b2a38", "1.84s");
    fixture
        .diagnose(&model.service(), &run, Language::English, "diagnose-1")
        .unwrap();
    let memo = InputsMemo::default();
    let functions = functions();
    let read = |key: &crate::test_runs::DerivedKey| {
        diagnoses(
            &fixture.session(),
            Facts {
                run: Some(&run),
                functions: &functions,
                kept: Some(KeptInputs { memo: &memo, key }),
            },
        )
        .unwrap()
    };
    let first = crate::test_runs::DerivedKey::for_tests(&fixture.id, 1);
    assert!(!read(&first)[0].outdated);

    // A new commit changes the code, read under the same key (as if neither
    // the run nor the sides had changed): what was prepared for that key is
    // used, no file is read again...
    fixture.repo.write(SERVICE, SERVICE_AFTER.replace("s.lastLogin = at", "s.lastLogin = at.UTC()"));
    fixture.repo.commit_all("Keep logins in UTC");
    assert!(!read(&first)[0].outdated);
    // ...and under the key other sides (or another run) get, it is prepared
    // anew, against the code now.
    let next = crate::test_runs::DerivedKey::for_tests(&fixture.id, 2);
    assert!(read(&next)[0].outdated);
    // Without a memo, as before.
    assert!(fixture.read(Some(&run))[0].outdated);
}

fn run_of_passing() -> TestRun {
    run(vec![case("TestSignUp", TestOutcome::Pass, None, "")], Vec::new())
}

#[test]
fn a_failure_no_longer_in_the_last_run_is_not_explained() {
    let fixture = Fixture::new();
    let model = answering(ModelProviderKind::Anthropic, "claude-opus-5-5");
    let error = fixture
        .diagnose(&model.service(), &run_of_passing(), Language::English, "diagnose-1")
        .unwrap_err();
    assert_eq!(error.reason, Some(Reason::DiagnosisFailureGone));
    assert!(model.asked().is_empty());
}

#[test]
fn a_failed_or_stopped_call_keeps_nothing() {
    let fixture = Fixture::new();
    let run = race_run(9, (214, 7), "0x1", "1s");
    let invalid = ScriptedModel::new(|_| Some(Ok(json!({ "cause": "", "fault": "code" }))));
    let error = fixture
        .diagnose(&invalid.service(), &run, Language::English, "diagnose-1")
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::ModelInvalidOutput);
    assert!(fixture.read(Some(&run)).is_empty());

    // A call that never answers by itself is stopped by its run id.
    let silent = ScriptedModel::new(|_| None);
    let models = Arc::new(silent.service());
    let canceller = {
        let silent = Arc::clone(&silent);
        let models = Arc::clone(&models);
        std::thread::spawn(move || {
            while silent.asked().is_empty() {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            models.cancel("diagnose-2");
        })
    };
    let error = fixture
        .diagnose(&models, &run, Language::English, "diagnose-2")
        .unwrap_err();
    canceller.join().unwrap();
    assert_eq!(error.code, ErrorCode::Cancelled);
    assert!(fixture.read(Some(&run)).is_empty());
}

#[test]
fn an_explanation_is_translated_on_request_by_the_translation_model() {
    let fixture = Fixture::new();
    let default = answering(ModelProviderKind::Anthropic, "claude-opus-5-5");
    let translator = answering(ModelProviderKind::Gemini, "gemini-3-pro");
    let models = default.service().with_task(
        ModelTask::Translation,
        Arc::clone(&translator) as Arc<dyn ModelProvider>,
        ModelProviderKind::Gemini,
        "gemini-3-pro",
    );
    let run = race_run(9, (214, 7), "0x1", "1s");
    let written = fixture
        .diagnose(&models, &run, Language::English, "diagnose-1")
        .unwrap();
    let keys = vec![written.key.clone()];

    let plan = plan_translation(&fixture.session(), &models, Language::BrazilianPortuguese, &keys).unwrap();
    assert_eq!(plan.items, 1);
    assert_eq!(plan.calls.len(), 1);
    assert_eq!(plan.model.provider, ModelProviderKind::Gemini);
    // Nothing to translate into the language it was written in.
    assert_eq!(
        plan_translation(&fixture.session(), &models, Language::English, &keys)
            .unwrap()
            .items,
        0
    );

    let functions = functions();
    let facts = Facts {
        run: Some(&run),
        functions: &functions,
        kept: None,
    };
    let translated = translate(
        &fixture.session(),
        &models,
        facts,
        Language::BrazilianPortuguese,
        &plan.calls[0].ids,
        "translate-1",
    )
    .unwrap();
    assert_eq!(translator.asked().len(), 1);
    assert_eq!(translator.asked()[0].schema_name, "translated_failure_explanations");
    // Only the prose is sent: not the code of the fix.
    assert!(!translator.asked()[0].prompt.contains("defer s.mu.Unlock()"));
    let versions = &translated[0].translations;
    assert_eq!(versions.len(), 1);
    assert_eq!(versions[0].language, Language::BrazilianPortuguese);
    assert_eq!(versions[0].fix, "Segure `s.mu` em `recordLastLogin`.");
    // The explanation itself stays as it was, in English.
    assert_eq!(translated[0].language, Language::English);
    assert_eq!(translated[0].fix_code, written.fix_code);
    assert_eq!(
        plan_translation(&fixture.session(), &models, Language::BrazilianPortuguese, &keys)
            .unwrap()
            .items,
        0
    );
    // Explained again: its translations go.
    fixture
        .diagnose(&models, &run, Language::English, "diagnose-2")
        .unwrap();
    assert!(fixture.read(Some(&run))[0].translations.is_empty());
}

#[test]
fn an_explanation_is_read_back_current_after_a_restart() {
    let folder = tempfile::tempdir().unwrap();
    let database = folder.path().join("difflens.db");
    let mut fixture = Fixture::with_store(Store::open(&database).unwrap());
    let model = answering(ModelProviderKind::ClaudeCli, "claude-opus-5-5");
    // A top-level test failing through its subtest, as the one explained in
    // the app was; each call reads the run anew.
    let failing = || {
        let mut subtest = case("TestSignUp/service_account", TestOutcome::Fail, None, "");
        subtest.failure = Some(race(9, (214, 7)));
        run(vec![case("TestSignUp", TestOutcome::Fail, None, "x"), subtest], Vec::new())
    };
    let written = fixture
        .diagnose(&model.service(), &failing(), Language::BrazilianPortuguese, "diagnose-1")
        .unwrap();

    // The app closed and opened again: a new store over the same file, and
    // the last run read again.
    fixture.projects = ProjectService::new(Store::open(&database).unwrap(), isolated_git(), None);
    let read = fixture.read(Some(&failing()));
    assert_eq!(read, vec![written]);
    assert!(!read[0].outdated);
}

// ---------- the review comment written from an explanation ----------

use std::sync::atomic::{AtomicU32, Ordering};

use super::comment::{self, GeneralReason, Placement};
use crate::hunks::ChangedLines;
use crate::model::{CommentKind, CommentState, WrittenReviewComment};

#[test]
fn a_comment_goes_on_a_line_the_change_adds_or_changes_and_else_on_the_pull_request() {
    let files: HashSet<String> = HashSet::from([SERVICE.to_string(), SERVICE_TEST_FILE.to_string()]);
    let changed = ChangedLines::default()
        .with_added(SERVICE, 11, 14)
        .with_removed(SERVICE, 9, 9);
    let at = |file: &str, line: u32| SourceRef {
        file: file.to_string(),
        line,
    };
    // An added (or modified) line of the new side.
    assert_eq!(
        comment::placement(Some(&at(SERVICE, 13)), &files, &changed),
        Placement::Line(at(SERVICE, 13))
    );
    assert_eq!(
        comment::placement(Some(&at(SERVICE, 11)), &files, &changed),
        Placement::Line(at(SERVICE, 11))
    );
    // A line of a changed file the change does not add: on the pull request
    // (the old side's removed line 9 is not the new side's line 9).
    for line in [9, 10, 15] {
        assert_eq!(
            comment::placement(Some(&at(SERVICE, line)), &files, &changed),
            Placement::General(GeneralReason::LineNotChanged(at(SERVICE, line)))
        );
    }
    // A file the review does not change, even with a matching line.
    let other = ChangedLines::default().with_added("app/auth/other.go", 1, 50);
    assert_eq!(
        comment::placement(Some(&at("app/auth/other.go", 3)), &files, &other),
        Placement::General(GeneralReason::FileNotInChange(at("app/auth/other.go", 3)))
    );
    // No line named.
    assert_eq!(
        comment::placement(None, &files, &changed),
        Placement::General(GeneralReason::NoLine)
    );
    assert_eq!(
        comment::placement(Some(&at(SERVICE, 0)), &files, &changed),
        Placement::General(GeneralReason::NoLine)
    );
}

#[test]
fn the_comment_schema_is_strict_and_its_parts_are_read_back_on_one_line_each() {
    assert_eq!(
        comment::schema(),
        json!({
            "type": "object",
            "properties": {
                "cause": { "type": "string" },
                "suggestion": { "type": "string" },
                "suggestion_code": { "type": "string" },
                "exposed_by": { "type": "string" },
                "unconfirmed": { "type": "string" }
            },
            "required": ["cause", "suggestion", "suggestion_code", "exposed_by", "unconfirmed"],
            "additionalProperties": false
        })
    );
    let parts = comment::parse(&json!({
        "cause": "  `SignUp` writes `lastLogin`\n\nwhile   the test reads it. ",
        "suggestion": "Hold `s.mu`.",
        "suggestion_code": "```go\ns.mu.Lock()\n\tdefer s.mu.Unlock()\n```\n",
        "exposed_by": "",
        "unconfirmed": " "
    }))
    .unwrap();
    assert_eq!(
        parts,
        comment::Parts {
            cause: "`SignUp` writes `lastLogin` while the test reads it.".to_string(),
            suggestion: "Hold `s.mu`.".to_string(),
            // The code keeps its own lines and indentation, not its fence.
            suggestion_code: "s.mu.Lock()\n\tdefer s.mu.Unlock()".to_string(),
            exposed_by: String::new(),
            unconfirmed: String::new(),
        }
    );
    // A missing part reads as empty; a long one is capped.
    let long = "word ".repeat(400);
    let parts = comment::parse(&json!({ "cause": "Races.", "suggestion": long })).unwrap();
    assert_eq!(parts.suggestion_code, "");
    assert!(parts.suggestion.ends_with('…'));
    assert!(parts.suggestion.chars().count() <= 601);
    let error = comment::parse(&json!({ "cause": "  ", "suggestion": "Hold `s.mu`." })).unwrap_err();
    assert_eq!(error.reason, Some(Reason::ReviewCommentNoBody));
}

#[test]
fn a_comment_names_the_innermost_failed_subtests_else_the_test() {
    let run = run(
        vec![
            case("TestSignUp", TestOutcome::Fail, None, ""),
            case("TestSignUp/AD", TestOutcome::Fail, None, ""),
            case("TestSignUp/AD/refuses", TestOutcome::Fail, None, ""),
            case("TestSignUp/AD/accepts", TestOutcome::Pass, None, ""),
            case("TestSignUp/local", TestOutcome::Fail, None, ""),
        ],
        Vec::new(),
    );
    let found = inputs::find(&run, &test_subject()).unwrap();
    assert_eq!(inputs::failed_names(&found), ["TestSignUp/AD/refuses", "TestSignUp/local"]);
    let alone = race_run(9, (214, 7), "0x1", "1s");
    let found = inputs::find(&alone, &test_subject()).unwrap();
    assert_eq!(inputs::failed_names(&found), ["TestSignUp"]);
}

fn at(file: &str, line: u32) -> SourceRef {
    SourceRef {
        file: file.to_string(),
        line,
    }
}

fn parts() -> comment::Parts {
    comment::Parts {
        cause: "`SignUp` writes `lastLogin` in a goroutine while the test reads it".to_string(),
        suggestion: "Hold `s.mu` in `recordLastLogin`.".to_string(),
        suggestion_code: "s.mu.Lock()\ndefer s.mu.Unlock()".to_string(),
        exposed_by: "It shows up because `SignUp` now runs `recordLastLogin` in a goroutine."
            .to_string(),
        unconfirmed: "The caller of `SignUp` was not shown.".to_string(),
    }
}

fn shape<'a>(language: Language, placement: &'a Placement) -> comment::Shape<'a> {
    comment::Shape {
        language,
        placement,
        target: None,
        package: "app/auth",
        test: "TestSignUp",
        failed: &[],
        kind: Some("data race"),
        confidence: DiagnosisConfidence::High,
    }
}

#[test]
fn a_comment_on_a_line_has_no_where_and_says_nothing_of_what_exposes_it() {
    let placement = Placement::Line(at(SERVICE, 13));
    assert_eq!(
        comment::render(&parts(), &shape(Language::English, &placement)),
        "**Failing test:** `TestSignUp` — data race (only shows with `-race`)\n\n\
         **Cause:** `SignUp` writes `lastLogin` in a goroutine while the test reads it.\n\n\
         **Suggestion:** Hold `s.mu` in `recordLastLogin`.\n\
         ```go\ns.mu.Lock()\ndefer s.mu.Unlock()\n```"
    );
}

#[test]
fn a_comment_on_the_pull_request_names_the_place_and_says_how_it_is_outside_the_change() {
    // The file outside the change, with what exposes the failure.
    let file = Placement::General(GeneralReason::FileNotInChange(at("app/auth/other_test.go", 40)));
    let mut no_code = parts();
    no_code.suggestion_code = String::new();
    assert_eq!(
        comment::render(&no_code, &shape(Language::English, &file)),
        "**Failing test:** `TestSignUp` — data race (only shows with `-race`)\n\n\
         **Where:** `app/auth/other_test.go:40`\n\
         This file is not changed in this pull request. It shows up because `SignUp` now runs \
         `recordLastLogin` in a goroutine.\n\n\
         **Cause:** `SignUp` writes `lastLogin` in a goroutine while the test reads it.\n\n\
         **Suggestion:** Hold `s.mu` in `recordLastLogin`."
    );
    // Only the line outside it, nothing shown to expose it.
    let line = Placement::General(GeneralReason::LineNotChanged(at(SERVICE, 9)));
    let mut unexposed = parts();
    unexposed.exposed_by = String::new();
    let body = comment::render(&unexposed, &shape(Language::English, &line));
    assert!(body.contains(
        "**Where:** `app/auth/service.go:9`\nThis pull request changes this file, but not this \
         line.\n\n**Cause:**"
    ));
    // The working tree's changes are not a pull request.
    let working = ReviewTarget::WorkingTree;
    let mut on_working = shape(Language::English, &file);
    on_working.target = Some(&working);
    assert!(comment::render(&parts(), &on_working)
        .contains("`app/auth/other_test.go:40`\nThis file is not among the changes. It shows up"));
    let mut on_working = shape(Language::BrazilianPortuguese, &line);
    on_working.target = Some(&working);
    assert!(comment::render(&unexposed, &on_working).contains(
        "\nEsse arquivo está entre as alterações, mas essa linha não foi alterada.\n\n"
    ));
    // No line named: no Where, and nothing of what exposes it.
    let none = Placement::General(GeneralReason::NoLine);
    let body = comment::render(&parts(), &shape(Language::English, &none));
    assert!(!body.contains("**Where:**"));
    assert!(!body.contains("It shows up"));
    // Without a file there is no language for the code block.
    assert!(body.contains("**Suggestion:** Hold `s.mu` in `recordLastLogin`.\n```\ns.mu.Lock()"));
}

#[test]
fn only_an_unsure_explanation_says_what_is_not_confirmed() {
    let placement = Placement::Line(at(SERVICE, 13));
    for confidence in [DiagnosisConfidence::High, DiagnosisConfidence::Medium] {
        let mut sure = shape(Language::English, &placement);
        sure.confidence = confidence;
        assert!(!comment::render(&parts(), &sure).contains("Not confirmed"));
    }
    let mut unsure = shape(Language::English, &placement);
    unsure.confidence = DiagnosisConfidence::Low;
    assert!(comment::render(&parts(), &unsure)
        .ends_with("```\n\n_Not confirmed:_ The caller of `SignUp` was not shown."));
    let mut nothing_said = parts();
    nothing_said.unconfirmed = String::new();
    assert!(!comment::render(&nothing_said, &unsure).contains("Not confirmed"));
    let mut unsure = shape(Language::BrazilianPortuguese, &placement);
    unsure.confidence = DiagnosisConfidence::Low;
    assert!(comment::render(&parts(), &unsure).contains("\n\n_Não confirmado:_ The caller"));
}

#[test]
fn the_comment_labels_are_in_the_language_asked_for() {
    let line = Placement::General(GeneralReason::FileNotInChange(at("app/auth/other_test.go", 40)));
    let body = comment::render(&parts(), &shape(Language::BrazilianPortuguese, &line));
    assert_eq!(
        body,
        "**Teste falhando:** `TestSignUp` — data race (só aparece com `-race`)\n\n\
         **Onde:** `app/auth/other_test.go:40`\n\
         Esse arquivo não foi alterado nesta PR. It shows up because `SignUp` now runs \
         `recordLastLogin` in a goroutine.\n\n\
         **Causa:** `SignUp` writes `lastLogin` in a goroutine while the test reads it.\n\n\
         **Sugestão:** Hold `s.mu` in `recordLastLogin`.\n\
         ```go\ns.mu.Lock()\ndefer s.mu.Unlock()\n```"
    );
    let changed = Placement::General(GeneralReason::LineNotChanged(at(SERVICE, 9)));
    assert!(comment::render(&parts(), &shape(Language::BrazilianPortuguese, &changed))
        .contains("\nEsse arquivo foi alterado nesta PR, mas não essa linha. It shows up"));

    // The failed subtests, a few at most; a package; the other kinds.
    let failed: Vec<String> = ["TestSignUp/a", "TestSignUp/b", "TestSignUp/c", "TestSignUp/d"]
        .iter()
        .map(|name| (*name).to_string())
        .collect();
    let mut subtests = shape(Language::English, &line);
    subtests.failed = &failed;
    subtests.kind = Some("failed assertion");
    assert!(comment::render(&parts(), &subtests).starts_with(
        "**Failing test:** `TestSignUp/a`, `TestSignUp/b`, `TestSignUp/c` +1 — failed assertion\n\n"
    ));
    let mut package = shape(Language::BrazilianPortuguese, &line);
    package.test = "";
    package.kind = Some("build failure (the package did not compile)");
    assert!(comment::render(&parts(), &package)
        .starts_with("**Pacote falhando:** `app/auth` — o pacote não compila\n\n"));
    for (kind, english, portuguese) in [
        ("timeout", " — timed out\n", " — tempo esgotado\n"),
        ("panic", " — panic\n", " — panic\n"),
        ("test failure", "`\n", "`\n"),
    ] {
        let mut english_shape = shape(Language::English, &line);
        english_shape.kind = Some(kind);
        assert!(comment::render(&parts(), &english_shape).contains(english), "{kind}");
        let mut portuguese_shape = shape(Language::BrazilianPortuguese, &line);
        portuguese_shape.kind = Some(kind);
        assert!(comment::render(&parts(), &portuguese_shape).contains(portuguese), "{kind}");
    }
    let mut unknown = shape(Language::English, &line);
    unknown.kind = None;
    assert!(comment::render(&parts(), &unknown).starts_with("**Failing test:** `TestSignUp`\n\n"));
}

/// The user's case: a data race in a test file the pull request does not
/// change, explained with medium confidence and no code to suggest.
#[test]
fn a_race_in_a_test_file_outside_the_pull_request_reads_as_a_short_labelled_comment() {
    let placement = Placement::General(GeneralReason::FileNotInChange(at(
        "app/services/auth_service/ad_login_test.go",
        500,
    )));
    let failed = vec!["TestAuthenticateAD_PasswordWithoutTouchingTheADPath/service_account".to_string()];
    let parts = comment::Parts {
        cause: "Provavelmente o login dispara `startPasswordWork` em background via \
                `GoRecover`, e essa goroutine lê o global `log.Logger` em `isOldCryptCost`. A \
                subtest termina sem esperar por ela, e o `t.Cleanup` de `useSsoLogs` restaura \
                `log.Logger` enquanto ela ainda roda."
            .to_string(),
        suggestion: "Faça o teste (ou `newADLoginBed`) esperar esse trabalho em background antes \
                     do cleanup, por exemplo com um `sync.WaitGroup` no hook `after` de \
                     `GoRecover` e um `Wait()` num `t.Cleanup` registrado depois de \
                     `useSsoLogs`."
            .to_string(),
        suggestion_code: String::new(),
        exposed_by: String::new(),
        unconfirmed: "Não deu para ver o bed nem `startPasswordWork`.".to_string(),
    };
    let body = comment::render(
        &parts,
        &comment::Shape {
            language: Language::BrazilianPortuguese,
            placement: &placement,
            target: None,
            package: "app/services/auth_service",
            test: "TestAuthenticateAD_PasswordWithoutTouchingTheADPath",
            failed: &failed,
            kind: Some("data race"),
            confidence: DiagnosisConfidence::Medium,
        },
    );
    assert_eq!(
        body,
        "**Teste falhando:** `TestAuthenticateAD_PasswordWithoutTouchingTheADPath/service_account` \
         — data race (só aparece com `-race`)\n\n\
         **Onde:** `app/services/auth_service/ad_login_test.go:500`\n\
         Esse arquivo não foi alterado nesta PR.\n\n\
         **Causa:** Provavelmente o login dispara `startPasswordWork` em background via \
         `GoRecover`, e essa goroutine lê o global `log.Logger` em `isOldCryptCost`. A subtest \
         termina sem esperar por ela, e o `t.Cleanup` de `useSsoLogs` restaura `log.Logger` \
         enquanto ela ainda roda.\n\n\
         **Sugestão:** Faça o teste (ou `newADLoginBed`) esperar esse trabalho em background \
         antes do cleanup, por exemplo com um `sync.WaitGroup` no hook `after` de `GoRecover` e \
         um `Wait()` num `t.Cleanup` registrado depois de `useSsoLogs`."
    );
}

/// Explains every failure as a race at `SERVICE` line `*line`, and writes
/// every review comment from `comment_answer`.
fn writing(provider: ModelProviderKind, model: &str, line: Arc<AtomicU32>) -> Arc<ScriptedModel> {
    ScriptedModel::writing_as(provider, model, move |request: &ModelRequest| {
        if request.schema_name == "review_comment" {
            return Some(Ok(comment_answer()));
        }
        Some(Ok(answer(SERVICE, line.load(Ordering::SeqCst))))
    })
}

fn comment_answer() -> Value {
    json!({
        "cause": "The goroutine `SignUp` starts writes `lastLogin` while the test reads it.",
        "suggestion": "Hold `s.mu` in `recordLastLogin`.",
        "suggestion_code": "```go\ns.mu.Lock()\ndefer s.mu.Unlock()\ns.lastLogin = at\n```",
        "exposed_by": "It shows up because `SignUp` now runs `recordLastLogin` in a goroutine.",
        "unconfirmed": "The caller was not shown."
    })
}

impl Fixture {
    fn changed_lines(&self) -> ChangedLines {
        let review = self.review();
        let patch = self
            .projects
            .git()
            .diff_patch(self.repo.path(), &review.old_sha, review.new_sha.as_deref())
            .unwrap();
        crate::hunks::parse_patch(&patch).unwrap()
    }

    fn write_comment(
        &self,
        models: &ModelService,
        run: Option<&TestRun>,
        language: Language,
    ) -> crate::error::AppResult<WrittenReviewComment> {
        let functions = functions();
        comment::write(
            &self.session(),
            models,
            Facts {
                run,
                functions: &functions,
                kept: None,
            },
            &self.changed_lines(),
            &test_subject().key(),
            language,
            "comment-1",
        )
    }
}

#[test]
fn a_review_comment_is_written_by_its_task_model_and_kept_as_a_line_draft_linked_to_the_explanation() {
    let fixture = Fixture::new();
    let line = Arc::new(AtomicU32::new(13));
    let default = writing(ModelProviderKind::Anthropic, "claude-opus-5-5", Arc::clone(&line));
    let commenter = writing(ModelProviderKind::OpenAi, "gpt-5.5", Arc::clone(&line));
    let models = default.service().with_task(
        ModelTask::ReviewComment,
        Arc::clone(&commenter) as Arc<dyn ModelProvider>,
        ModelProviderKind::OpenAi,
        "gpt-5.5",
    );
    let run = race_run(9, (214, 7), "0x1", "1s");
    fixture.diagnose(&models, &run, Language::English, "diagnose-1").unwrap();
    assert_eq!(default.asked().len(), 1, "the explanation, by its own (default) model");

    let written = fixture
        .write_comment(&models, Some(&run), Language::BrazilianPortuguese)
        .unwrap();

    // Asked once, of the task's own model.
    assert_eq!(default.asked().len(), 1);
    let asked = commenter.asked();
    assert_eq!(asked.len(), 1);
    assert_eq!(asked[0].schema_name, "review_comment");
    assert_eq!(asked[0].system, comment::SYSTEM_PROMPT);
    assert_eq!(asked[0].schema, comment::schema());
    let prompt = &asked[0].prompt;
    assert!(prompt.starts_with(
        "Write every prose field of your answer in Brazilian Portuguese.\n\nWrite the parts of a \
         review comment"
    ));
    for part in [
        "The change under review: branch feature/AUTH-12 compared with main.",
        "Test: TestSignUp, in package app/auth.",
        "Kind: data race.",
        "Cause: `SignUp` starts a goroutine that writes `lastLogin` while the test reads it.",
        "At fault: the code under test (code)",
        "Confidence: high",
        "Line to change: app/auth/service.go:13",
        "Proposed fix: Hold `s.mu` in `recordLastLogin`.",
        "Proposed code:\n<untrusted_code>\ns.mu.Lock()",
        // The code around the line, as the explanation was shown it.
        "=== at the failure",
        "+    13 | \tgo s.recordLastLogin(time.Now())",
        "On line 13 of app/auth/service.go, a line this change adds or changes: the comment is \
         attached there. Leave exposed_by empty.",
    ] {
        assert!(prompt.contains(part), "{part:?} in:\n{prompt}");
    }
    // On a changed line nothing is outside the change.
    assert!(!prompt.contains("## What this change edits"));

    // A draft on that line, put together from the parts: no Where, and
    // nothing of what exposes it or of what is unconfirmed.
    assert_eq!(written.kind, CommentKind::Line);
    let state = fixture.session().state(&[]).unwrap();
    let draft = state
        .comments
        .iter()
        .find(|comment| comment.id == written.comment_id)
        .unwrap();
    assert_eq!(
        (draft.kind, draft.path.as_str(), draft.line, draft.state),
        (CommentKind::Line, SERVICE, 13, CommentState::Draft)
    );
    assert_eq!(
        draft.body,
        "**Teste falhando:** `TestSignUp` — data race (só aparece com `-race`)\n\n\
         **Causa:** The goroutine `SignUp` starts writes `lastLogin` while the test reads it.\n\n\
         **Sugestão:** Hold `s.mu` in `recordLastLogin`.\n\
         ```go\ns.mu.Lock()\ndefer s.mu.Unlock()\ns.lastLogin = at\n```"
    );
    assert_eq!(state.summary.unpublished, 1);
    let read = fixture.read(Some(&run));
    assert_eq!(read[0].comment_id.as_deref(), Some(written.comment_id.as_str()));

    // Asked again while it is there: the same comment, no call.
    let again = fixture.write_comment(&models, Some(&run), Language::English).unwrap();
    assert_eq!(again, written);
    assert_eq!(commenter.asked().len(), 1);
    // Explained again: still linked.
    let explained = fixture.diagnose(&models, &run, Language::English, "diagnose-2").unwrap();
    assert_eq!(explained.comment_id.as_deref(), Some(written.comment_id.as_str()));

    // The draft deleted: unlinked, and Write review comment writes anew.
    crate::review_state::delete_comment(fixture.projects.store(), &fixture.id, &written.comment_id).unwrap();
    assert_eq!(fixture.read(Some(&run))[0].comment_id, None);
    let anew = fixture.write_comment(&models, Some(&run), Language::English).unwrap();
    assert_ne!(anew.comment_id, written.comment_id);
    assert_eq!(commenter.asked().len(), 2);
}

#[test]
fn a_comment_about_a_line_the_change_does_not_touch_goes_on_the_pull_request_naming_it() {
    let fixture = Fixture::new();
    // Line 9 (`s.lastLogin = at`) is the same on both sides.
    let line = Arc::new(AtomicU32::new(9));
    let model = writing(ModelProviderKind::Anthropic, "claude-opus-5-5", line);
    let models = model.service();
    let run = race_run(9, (214, 7), "0x1", "1s");
    fixture.diagnose(&models, &run, Language::English, "diagnose-1").unwrap();

    let written = fixture.write_comment(&models, Some(&run), Language::English).unwrap();

    assert_eq!(written.kind, CommentKind::General);
    let prompt = &model.asked()[1].prompt;
    for part in [
        "On the pull request as a whole: the line is outside the change. This change edits \
         app/auth/service.go but not line 9. The comment already names `app/auth/service.go:9` and \
         says the line is unchanged: do not repeat either.",
        "      9 | \ts.lastLogin = at",
        // What the change edits of the code the failure points at: SignUp,
        // where the goroutine starts; not recordLastLogin nor the test.
        "## What this change edits in the code the failure points at\n\n\
         - `(*Service).SignUp` in app/auth/service.go, lines 12-14 (at the failure (where the \
         goroutine of the write of the data race was started)): 3 lines added or changed by this \
         change\n\
         Fill exposed_by from these and the explanation;",
    ] {
        assert!(prompt.contains(part), "{part:?} in:\n{prompt}");
    }
    assert!(!prompt.contains("`(*Service).recordLastLogin` in"));
    assert!(!prompt.contains("`TestSignUp` in"));
    let state = fixture.session().state(&[]).unwrap();
    let draft = state
        .comments
        .iter()
        .find(|comment| comment.id == written.comment_id)
        .unwrap();
    assert_eq!(
        (draft.kind, draft.path.as_str(), draft.line, draft.current_line),
        (CommentKind::General, "", 0, None)
    );
    assert_eq!(
        draft.body,
        "**Failing test:** `TestSignUp` — data race (only shows with `-race`)\n\n\
         **Where:** `app/auth/service.go:9`\n\
         This pull request changes this file, but not this line. It shows up because `SignUp` now \
         runs `recordLastLogin` in a goroutine.\n\n\
         **Cause:** The goroutine `SignUp` starts writes `lastLogin` while the test reads it.\n\n\
         **Suggestion:** Hold `s.mu` in `recordLastLogin`.\n\
         ```go\ns.mu.Lock()\ndefer s.mu.Unlock()\ns.lastLogin = at\n```"
    );
    assert_eq!(state.summary.unpublished, 1);
}

#[test]
fn a_failure_the_last_run_no_longer_has_still_gets_a_comment_from_its_explanation() {
    let fixture = Fixture::new();
    let line = Arc::new(AtomicU32::new(9));
    let model = writing(ModelProviderKind::Anthropic, "claude-opus-5-5", line);
    let models = model.service();
    let run = race_run(9, (214, 7), "0x1", "1s");
    fixture.diagnose(&models, &run, Language::English, "diagnose-1").unwrap();

    let written = fixture.write_comment(&models, None, Language::English).unwrap();

    let prompt = &model.asked()[1].prompt;
    assert!(prompt.contains(
        "Not known: the last test run no longer has this failure. Fill exposed_by only when the \
         explanation says outright which edit of this change exposes it."
    ));
    assert!(!prompt.contains("Failed subtests"));
    let state = fixture.session().state(&[]).unwrap();
    let draft = state
        .comments
        .iter()
        .find(|comment| comment.id == written.comment_id)
        .unwrap();
    // The kind as kept with the explanation; the test as kept.
    assert!(draft
        .body
        .starts_with("**Failing test:** `TestSignUp` — data race (only shows with `-race`)\n\n"));
}

#[test]
fn an_explanation_kept_before_excerpts_sends_the_lines_around_it_now() {
    let fixture = Fixture::new();
    let line = Arc::new(AtomicU32::new(13));
    let model = writing(ModelProviderKind::Anthropic, "claude-opus-5-5", line);
    let models = model.service();
    let run = race_run(9, (214, 7), "0x1", "1s");
    fixture.diagnose(&models, &run, Language::English, "diagnose-1").unwrap();
    let store = fixture.projects.store();
    let review_id = fixture.session().record().unwrap().id;
    let mut kept = store.diagnoses(&review_id).unwrap().remove(0);
    kept.code_excerpt = None;
    kept.failure_kind = None;
    store.save_diagnosis(&kept).unwrap();

    let written = fixture.write_comment(&models, None, Language::English).unwrap();
    let prompt = &model.asked()[1].prompt;
    assert!(prompt.contains("Kind: not recorded."));
    // The kind read from the last run when the explanation kept none.
    crate::review_state::delete_comment(store, &fixture.id, &written.comment_id).unwrap();
    fixture.write_comment(&models, Some(&run), Language::English).unwrap();
    let prompt = &model.asked()[2].prompt;
    assert!(prompt.contains("Kind: data race."));
    let prompt = &model.asked()[1].prompt;
    assert!(prompt.contains("=== app/auth/service.go, lines 3-14 ==="), "{prompt}");
    assert!(prompt.contains("     13 | \tgo s.recordLastLogin(time.Now())"));
}

#[test]
fn no_comment_is_written_for_a_failure_never_explained() {
    let fixture = Fixture::new();
    let model = writing(
        ModelProviderKind::Anthropic,
        "claude-opus-5-5",
        Arc::new(AtomicU32::new(13)),
    );
    let error = fixture.write_comment(&model.service(), None, Language::English).unwrap_err();
    assert_eq!(error.reason, Some(Reason::DiagnosisGone));
    assert!(model.asked().is_empty());
}
