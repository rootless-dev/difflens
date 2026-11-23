import { describe, expect, it } from "vitest";

import { i18n } from "@/i18n";
import type { ApiError, PublishFailure } from "@/lib/api";
import {
  commentPublishError,
  coreText,
  modelProblem,
  publishFailureMessage,
  readinessReason,
} from "@/lib/core-text";
import { errorMessage } from "@/lib/format";
import { makeModelInUse } from "@/test/insights-fixtures";
import { makeComment } from "@/test/review-fixtures";

// As the core sends them (see `serialises_its_reason_params_and_detail` in src-tauri/src/error.rs).
const cloneFailed: ApiError = {
  code: "clone_failed",
  message: "Could not clone git@example.com:acme/api.git: fatal: repository not found",
  reason: "clone_failed",
  params: { url: "git@example.com:acme/api.git" },
  detail: "fatal: repository not found",
};

const unreachable: ApiError = {
  code: "network_failed",
  message: "Could not reach the Google Gemini API: it did not answer in time.",
  reason: "model_unreachable",
  params: {
    provider: { message: "the Google Gemini API", reason: "provider_the_api", params: { name: "Google Gemini API" } },
    detail: {
      message: "it did not answer in time.",
      reason: "sentence",
      params: { text: { message: "it did not answer in time", reason: "transport_timeout" } },
    },
  },
};

describe("errorMessage", () => {
  it("words an error of the core from its reason, params and detail in the language shown", async () => {
    expect(errorMessage(cloneFailed)).toBe(cloneFailed.message);
    await i18n.changeLanguage("pt-BR");
    expect(errorMessage(cloneFailed)).toBe(
      "Não foi possível clonar git@example.com:acme/api.git: fatal: repository not found",
    );
  });

  it("words the sentences that fill a message too", async () => {
    expect(errorMessage(unreachable)).toBe(unreachable.message);
    await i18n.changeLanguage("pt-BR");
    expect(errorMessage(unreachable)).toBe(
      "Não foi possível acessar o provedor Google Gemini API: ele não respondeu a tempo.",
    );
  });

  it("shows the message as it is without a reason, or with one the catalog does not know", async () => {
    await i18n.changeLanguage("pt-BR");
    expect(errorMessage({ code: "rate_limited", message: "Slow down." })).toBe("Slow down.");
    expect(errorMessage({ code: "internal", message: "Gone.", reason: "no_such_reason" })).toBe("Gone.");
    expect(errorMessage(new Error("boom"))).toBe("boom");
  });
});

describe("the core-text helpers", () => {
  it("word a model problem, a blocked publish, a comment's publish error and a refused change", async () => {
    const model = makeModelInUse({
      ready: false,
      problem: {
        code: "model_not_configured",
        message: "Save an API key for the OpenAI API in Preferences › Model.",
        reason: "api_key_not_saved",
        params: { provider: { message: "the OpenAI API", reason: "provider_the_api", params: { name: "OpenAI API" } } },
      },
    });
    const blocked: ApiError = {
      code: "publish_blocked",
      message: "The pull request is no longer open.",
      reason: "pull_request_not_open",
    };
    const readiness = { allowed: false, reason: blocked };
    const refused: ApiError = {
      code: "remote_failed",
      message: "Bitbucket refused this: bad anchor",
      reason: "host_refused_said",
      params: { host: "Bitbucket" },
      detail: "bad anchor",
    };
    const failure: PublishFailure = { commentId: "c", path: "a.go", line: 3, ...refused };

    expect(modelProblem(model)).toBe("Save an API key for the OpenAI API in Preferences › Model.");
    expect(readinessReason(readiness)).toBe("The pull request is no longer open.");
    expect(commentPublishError(makeComment("c", { publishError: refused }))).toBe(refused.message);
    expect(publishFailureMessage(failure)).toBe(refused.message);

    await i18n.changeLanguage("pt-BR");
    expect(modelProblem(model)).toBe("Salve uma chave de API para o provedor OpenAI API em Preferências › Modelo.");
    expect(readinessReason(readiness)).toBe("O pull request não está mais aberto.");
    expect(commentPublishError(makeComment("c", { publishError: refused }))).toBe(
      "O Bitbucket recusou isto: bad anchor",
    );
    expect(publishFailureMessage(failure)).toBe("O Bitbucket recusou isto: bad anchor");
  });

  it("say nothing when there is nothing wrong, and show text without a reason as it is", async () => {
    expect(modelProblem(makeModelInUse())).toBeNull();
    expect(readinessReason({ allowed: true, reason: null })).toBeNull();
    expect(commentPublishError(makeComment("c"))).toBeNull();
    await i18n.changeLanguage("pt-BR");
    expect(modelProblem(makeModelInUse({ problem: "Choose a model in Preferences › Model." }))).toBe(
      "Choose a model in Preferences › Model.",
    );
    expect(coreText({ message: "Insufficient credits" })).toBe("Insufficient credits");
  });

  it("show a tool's own text as it is and word the core's analyzer messages", async () => {
    // As the core sends a failed flow analysis (see `text_from_elsewhere_is_a_plain_string...` in error.rs).
    const notInstalled = {
      message: "The Go analyzer is not installed; run pnpm analyzers:build.",
      reason: "analyzer_not_installed",
      params: { analyzer: { message: "The Go analyzer", reason: "analyzer_title", params: { language: "Go" } } },
    };
    expect(coreText("vet: one package skipped")).toBe("vet: one package skipped");
    expect(coreText(notInstalled)).toBe(notInstalled.message);
    await i18n.changeLanguage("pt-BR");
    expect(coreText("vet: one package skipped")).toBe("vet: one package skipped");
    expect(coreText(notInstalled)).toBe("O analisador de Go não está instalado; execute pnpm analyzers:build.");
  });
});
