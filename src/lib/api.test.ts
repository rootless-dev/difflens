import { invoke } from "@tauri-apps/api/core";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { i18n } from "@/i18n";
import {
  addGeneralComment,
  cancelTestDiagnosis,
  diagnoseTestFailure,
  getRunningTestDiagnoses,
  joinTestDiagnosis,
  startInsights,
  summarizeOverview,
  writeFindingComment,
  writeReviewComment,
} from "@/lib/api";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

const REVIEW = { target: { kind: "working_tree" as const }, oldSha: "a1b2c3d", newSha: null };

beforeEach(() => {
  vi.mocked(invoke).mockReset();
  vi.mocked(invoke).mockResolvedValue({});
});

describe("asking for text the model writes", () => {
  it("asks Analyze and Summarize for the language shown when they are clicked", async () => {
    await startInsights("p-1", REVIEW);
    expect(invoke).toHaveBeenLastCalledWith("start_insights", { projectId: "p-1", review: REVIEW, language: "en" });

    await i18n.changeLanguage("pt-BR");
    await startInsights("p-1", REVIEW);
    expect(invoke).toHaveBeenLastCalledWith("start_insights", { projectId: "p-1", review: REVIEW, language: "pt-BR" });
    await summarizeOverview("p-1", REVIEW, null, "run-1");
    expect(invoke).toHaveBeenLastCalledWith("summarize_overview", {
      projectId: "p-1",
      review: REVIEW,
      pullRequest: null,
      runId: "run-1",
      language: "pt-BR",
    });
  });

  it("asks Explain the failure for the language shown when it is clicked", async () => {
    await i18n.changeLanguage("en");
    await diagnoseTestFailure("p-1", REVIEW, "app/containers", null, "diagnose-1");
    expect(invoke).toHaveBeenLastCalledWith("diagnose_test_failure", {
      projectId: "p-1",
      review: REVIEW,
      package: "app/containers",
      test: null,
      language: "en",
      runId: "diagnose-1",
    });
    await i18n.changeLanguage("pt-BR");
    await diagnoseTestFailure("p-1", REVIEW, "app/auth", "TestSignUp", "diagnose-2");
    expect(invoke).toHaveBeenLastCalledWith("diagnose_test_failure", {
      projectId: "p-1",
      review: REVIEW,
      package: "app/auth",
      test: "TestSignUp",
      language: "pt-BR",
      runId: "diagnose-2",
    });
    await i18n.changeLanguage("en");
  });

  it("asks Write review comment for the language shown when it is clicked", async () => {
    await i18n.changeLanguage("pt-BR");
    await writeReviewComment("p-1", REVIEW, "app/auth::TestSignUp", "review-comment-1");
    expect(invoke).toHaveBeenLastCalledWith("write_review_comment", {
      projectId: "p-1",
      review: REVIEW,
      key: "app/auth::TestSignUp",
      language: "pt-BR",
      runId: "review-comment-1",
    });
    await i18n.changeLanguage("en");
  });

  it("asks Write comment on a finding for the language shown when it is clicked", async () => {
    await i18n.changeLanguage("pt-BR");
    await writeFindingComment("p-1", REVIEW, "f-1", "finding-comment-1");
    expect(invoke).toHaveBeenLastCalledWith("write_finding_comment", {
      projectId: "p-1",
      review: REVIEW,
      findingId: "f-1",
      language: "pt-BR",
      runId: "finding-comment-1",
    });
    await i18n.changeLanguage("en");
  });
});

describe("explanations being written and general comments", () => {
  it("names the commands the core keeps them by", async () => {
    await getRunningTestDiagnoses("p-1", REVIEW);
    expect(invoke).toHaveBeenLastCalledWith("get_running_test_diagnoses", { projectId: "p-1", review: REVIEW });
    await joinTestDiagnosis("p-1", REVIEW, "app/auth::TestSignUp");
    expect(invoke).toHaveBeenLastCalledWith("join_test_diagnosis", {
      projectId: "p-1",
      review: REVIEW,
      key: "app/auth::TestSignUp",
    });
    await cancelTestDiagnosis("p-1", REVIEW, "app/auth::TestSignUp");
    expect(invoke).toHaveBeenLastCalledWith("cancel_test_diagnosis", {
      projectId: "p-1",
      review: REVIEW,
      key: "app/auth::TestSignUp",
    });
    await addGeneralComment("p-1", REVIEW, "Split the migration out.");
    expect(invoke).toHaveBeenLastCalledWith("add_general_comment", {
      projectId: "p-1",
      review: REVIEW,
      body: "Split the migration out.",
    });
  });
});
