import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { DiffEditorProps } from "@/features/review/DiffEditor";
import { ReviewScreen } from "@/features/review/ReviewScreen";
import { i18n } from "@/i18n";
import {
  cancelModelRun,
  type Finding,
  findCard,
  getFileVersions,
  getInsights,
  getOverview,
  getReviewState,
  getTestsState,
  type Insights,
  linkPullRequest,
  listChangedFiles,
  type OverviewSummary,
  planInsights,
  planInsightsTranslation,
  planOverviewTranslation,
  type ReviewTarget,
  runInsightsBatch,
  startInsights,
  summarizeOverview,
  translateInsights,
  translateOverview,
  type TranslationPlan,
} from "@/lib/api";
import { apiError, deferred, makeChangedFile, makeChangeSet, textSide } from "@/test/fixtures";
import { makeFinding, makeInsights, makeModelInUse, makePlan } from "@/test/insights-fixtures";
import { makeOverview } from "@/test/overview-fixtures";
import { makeReviewState } from "@/test/review-fixtures";
import { answerReviewOpening, changedFilesRead } from "@/test/review-screen";
import { makeTestsState } from "@/test/tests-fixtures";

vi.mock("@/lib/api", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/lib/api")>()),
  listChangedFiles: vi.fn(),
  getFileVersions: vi.fn(),
  getTestsState: vi.fn(),
  getMutationState: vi.fn(),
  getReviewState: vi.fn(),
  linkPullRequest: vi.fn(),
  getInsights: vi.fn(),
  planInsights: vi.fn(),
  startInsights: vi.fn(),
  runInsightsBatch: vi.fn(),
  setFindingVerdict: vi.fn(),
  cancelModelRun: vi.fn(),
  findCard: vi.fn(),
  getOverview: vi.fn(),
  planOverview: vi.fn(),
  summarizeOverview: vi.fn(),
  getFlow: vi.fn(),
  getQuality: vi.fn(),
  planInsightsTranslation: vi.fn(),
  translateInsights: vi.fn(),
  planOverviewTranslation: vi.fn(),
  translateOverview: vi.fn(),
}));

vi.mock("@/features/review/DiffEditor", () => ({
  default: (props: DiffEditorProps) => <div data-testid="diff-editor" data-path={props.path} />,
}));

const BRANCH: ReviewTarget = { kind: "branch", branch: "feat/refund-flow", base: "main" };
const changeSet = makeChangeSet([makeChangedFile("internal/refund/service.go", { status: "added" })]);
const REVIEW = { target: BRANCH, oldSha: "a1b2c3d4e5f6a7b8", newSha: "e7f3b10c9d8e7f6a" };

const PORTUGUESE = {
  language: "pt-BR" as const,
  title: "O reembolso pode ser pago e nunca registrado",
  explanation: "O gateway paga antes de o reembolso ser gravado.",
  suggestedDirection: "Grave o reembolso como pendente antes de chamar o gateway.",
  evidence: ["A linha 52 pede o reembolso; a linha 56 só o grava depois."],
  related: [],
};

const ENGLISH_FINDING = makeFinding("f-1");

function translated(finding: Finding): Finding {
  return { ...finding, translations: [PORTUGUESE] };
}

function plan(overrides: Partial<TranslationPlan> = {}): TranslationPlan {
  return {
    items: 1,
    calls: [{ ids: [ENGLISH_FINDING.id], tokens: 900 }],
    tokens: 900,
    model: makeModelInUse(),
    ...overrides,
  };
}

let bar: HTMLDivElement;

beforeEach(() => {
  vi.resetAllMocks();
  answerReviewOpening();
  bar = document.createElement("div");
  document.body.append(bar);
  vi.mocked(listChangedFiles).mockResolvedValue(changeSet);
  vi.mocked(getFileVersions).mockResolvedValue({ old: textSide(""), new: textSide("package refund\n") });
  vi.mocked(getTestsState).mockResolvedValue(makeTestsState());
  vi.mocked(getReviewState).mockResolvedValue(makeReviewState());
  vi.mocked(linkPullRequest).mockResolvedValue({ kind: "no_remote" });
  vi.mocked(getInsights).mockResolvedValue(makeInsights({ findings: [ENGLISH_FINDING] }));
  vi.mocked(planInsights).mockResolvedValue(makePlan());
  vi.mocked(cancelModelRun).mockResolvedValue(undefined);
});

afterEach(() => {
  bar.remove();
});

/** Every call that asks a model, of any kind. */
const modelCalls = (): unknown[] => [
  ...vi.mocked(startInsights).mock.calls,
  ...vi.mocked(runInsightsBatch).mock.calls,
  ...vi.mocked(translateInsights).mock.calls,
  ...vi.mocked(summarizeOverview).mock.calls,
  ...vi.mocked(translateOverview).mock.calls,
];

async function openInsightsIn(language: "en" | "pt-BR") {
  const user = userEvent.setup();
  render(<ReviewScreen projectId="p-1" target={BRANCH} barSlot={bar} />);
  await changedFilesRead();
  await act(() => i18n.changeLanguage(language));
  // "Insights" in both languages; the tab list's own name is translated.
  await user.click(screen.getByRole("tab", { name: /^Insights/ }));
  await screen.findByRole("heading", { level: 1 });
  return user;
}

const banner = () => screen.getByRole("region", { name: "Idioma deste texto" });

describe("findings written in another language", () => {
  it("are shown as written, under a banner, and switching the language asks no model", async () => {
    await openInsightsIn("pt-BR");
    expect(banner()).toHaveTextContent("Este achado foi escrito em inglês.");
    expect(within(banner()).getByRole("button", { name: "Traduzir" })).toBeInTheDocument();
    expect(within(banner()).getByRole("button", { name: "Analisar de novo" })).toBeInTheDocument();
    expect(screen.getByRole("heading", { level: 1, name: ENGLISH_FINDING.title })).toBeInTheDocument();

    await act(() => i18n.changeLanguage("en"));
    expect(screen.queryByRole("region", { name: "Language of this text" })).not.toBeInTheDocument();
    await act(() => i18n.changeLanguage("pt-BR"));
    expect(banner()).toBeInTheDocument();
    expect(modelCalls()).toEqual([]);
    expect(planInsightsTranslation).not.toHaveBeenCalled();
  });

  it("are translated only after Translate shows who would be asked, then read in the language shown", async () => {
    vi.mocked(planInsightsTranslation).mockResolvedValue(plan());
    vi.mocked(translateInsights).mockResolvedValue(makeInsights({ findings: [translated(ENGLISH_FINDING)] }));
    const user = await openInsightsIn("pt-BR");

    await user.click(within(banner()).getByRole("button", { name: "Traduzir" }));
    expect(planInsightsTranslation).toHaveBeenCalledExactlyOnceWith("p-1", REVIEW, "pt-BR");
    expect(
      await within(banner()).findByText(
        "Traduzir envia o texto de 1 achado para Claude Code, modelo opus, em 1 chamada, cerca de 900 tokens de " +
          "entrada. Só o texto muda: o achado, o veredito e o local continuam como estão.",
      ),
    ).toBeInTheDocument();
    expect(translateInsights).not.toHaveBeenCalled();

    await user.click(within(banner()).getByRole("button", { name: "Traduzir" }));
    await waitFor(() => expect(screen.queryByRole("region", { name: "Idioma deste texto" })).not.toBeInTheDocument());
    expect(translateInsights).toHaveBeenCalledExactlyOnceWith(
      "p-1",
      REVIEW,
      "pt-BR",
      [ENGLISH_FINDING.id],
      expect.stringMatching(/^translate-/),
    );
    expect(screen.getByRole("heading", { level: 1, name: PORTUGUESE.title })).toBeInTheDocument();
    expect(screen.getByText(PORTUGUESE.suggestedDirection)).toBeInTheDocument();

    // The original is still there: back in English it shows, with no call.
    await act(() => i18n.changeLanguage("en"));
    expect(screen.getByRole("heading", { level: 1, name: ENGLISH_FINDING.title })).toBeInTheDocument();
    expect(translateInsights).toHaveBeenCalledOnce();
  });

  it("runs a large translation in the planned calls, and Stop cancels the one in flight", async () => {
    const other = makeFinding("f-2", { title: "Retrying POST /refunds refunds twice" });
    vi.mocked(getInsights).mockResolvedValue(makeInsights({ findings: [ENGLISH_FINDING, other] }));
    vi.mocked(planInsightsTranslation).mockResolvedValue(
      plan({
        items: 2,
        calls: [
          { ids: [ENGLISH_FINDING.id], tokens: 5000 },
          { ids: ["f-2"], tokens: 5000 },
        ],
        tokens: 10_000,
      }),
    );
    const first = deferred<Insights>();
    vi.mocked(translateInsights).mockReturnValueOnce(first.promise);
    const user = await openInsightsIn("pt-BR");

    await user.click(within(banner()).getByRole("button", { name: "Traduzir" }));
    await within(banner()).findByText(/em 2 chamadas, cerca de 10\.000 tokens/);
    await user.click(within(banner()).getByRole("button", { name: "Traduzir" }));
    expect(await within(banner()).findByText("Traduzindo com Claude Code, modelo opus…")).toBeInTheDocument();

    await user.click(within(banner()).getByRole("button", { name: "Parar" }));
    const runId = vi.mocked(translateInsights).mock.calls[0][4];
    expect(cancelModelRun).toHaveBeenCalledWith(runId);
    first.reject(apiError("cancelled", "Stopped."));

    // Stopped: nothing failed, the second call never ran, and Translate is offered again.
    expect(await within(banner()).findByRole("button", { name: "Traduzir" })).toBeInTheDocument();
    expect(translateInsights).toHaveBeenCalledOnce();
    expect(within(banner()).queryByText(/falhou/)).not.toBeInTheDocument();
  });

  it("say why a translation failed and keep the original", async () => {
    vi.mocked(planInsightsTranslation).mockResolvedValue(plan());
    vi.mocked(translateInsights).mockRejectedValue(apiError("rate_limited", "Too many requests."));
    const user = await openInsightsIn("pt-BR");
    await user.click(within(banner()).getByRole("button", { name: "Traduzir" }));
    await user.click(await within(banner()).findByRole("button", { name: "Traduzir" }));
    expect(await within(banner()).findByText(/^A tradução falhou: /)).toBeInTheDocument();
    expect(screen.getByRole("heading", { level: 1, name: ENGLISH_FINDING.title })).toBeInTheDocument();
  });

  it("are not offered a translation when a version in the language shown exists", async () => {
    const portuguese: Finding = {
      ...makeFinding("f-1", { ...PORTUGUESE }),
      translations: [{ ...PORTUGUESE, language: "en", title: ENGLISH_FINDING.title }],
    };
    vi.mocked(getInsights).mockResolvedValue(makeInsights({ findings: [portuguese] }));
    await openInsightsIn("en");
    expect(screen.queryByRole("region", { name: "Language of this text" })).not.toBeInTheDocument();
    expect(screen.getByRole("heading", { level: 1, name: ENGLISH_FINDING.title })).toBeInTheDocument();
  });

  it("Analyze again from the banner shows what Analyze would send first", async () => {
    const user = await openInsightsIn("pt-BR");
    await user.click(within(banner()).getByRole("button", { name: "Analisar de novo" }));
    expect(await screen.findByRole("button", { name: "Analisar" })).toBeInTheDocument();
    expect(startInsights).not.toHaveBeenCalled();
  });
});

function summary(overrides: Partial<OverviewSummary> = {}): OverviewSummary {
  return {
    headSha: changeSet.newSha,
    provider: "claude_cli",
    providerName: "Claude Code",
    model: "claude-opus-5-5",
    createdAt: "2026-11-19T10:00:00Z",
    condensed: false,
    outdated: false,
    cardKey: null,
    cardComments: 0,
    language: "en",
    translations: [],
    title: "Adds the refund flow",
    lead: "Customers can get a payment back.",
    points: ["A new `POST /refunds` endpoint takes the payment.", "The gateway pays it back."],
    attention: ["The diff also carries the payouts work."],
    cardSummary: "",
    criteria: [],
    ...overrides,
  };
}

describe("a summary written in another language", () => {
  it("is translated on Translate and read in the language shown", async () => {
    vi.mocked(findCard).mockResolvedValue({ kind: "no_key", branch: BRANCH.branch });
    vi.mocked(getOverview).mockResolvedValue(makeOverview({ summary: summary() }));
    vi.mocked(planOverviewTranslation).mockResolvedValue(plan({ calls: [{ ids: [], tokens: 400 }], tokens: 400 }));
    vi.mocked(translateOverview).mockResolvedValue(
      makeOverview({
        summary: summary({
          translations: [
            {
              language: "pt-BR",
              title: "Adiciona o fluxo de reembolso",
              lead: "Clientes podem receber um pagamento de volta.",
              points: ["Um novo endpoint `POST /refunds` recebe o pagamento.", "O gateway o devolve."],
              attention: ["O diff também traz o trabalho de repasses."],
              cardSummary: "",
              criteria: [],
            },
          ],
        }),
      }),
    );
    const user = userEvent.setup();
    render(<ReviewScreen projectId="p-1" target={BRANCH} barSlot={bar} />);
    await screen.findByRole("heading", { level: 1, name: "Adds the refund flow" });
    await act(() => i18n.changeLanguage("pt-BR"));

    expect(banner()).toHaveTextContent("Este resumo foi escrito em inglês.");
    expect(within(banner()).getByRole("button", { name: "Resumir de novo" })).toBeInTheDocument();
    expect(modelCalls()).toEqual([]);

    await user.click(within(banner()).getByRole("button", { name: "Traduzir" }));
    expect(
      await within(banner()).findByText(/^Traduzir envia o texto deste resumo para Claude Code, modelo opus/),
    ).toBeInTheDocument();
    await user.click(within(banner()).getByRole("button", { name: "Traduzir" }));
    expect(await screen.findByRole("heading", { level: 1, name: "Adiciona o fluxo de reembolso" })).toBeInTheDocument();
    // The lead, each point and what needs attention all read in Portuguese.
    expect(screen.getByText("Clientes podem receber um pagamento de volta.")).toBeInTheDocument();
    expect(
      within(screen.getByRole("list", { name: "Como a mudança funciona" }))
        .getAllByRole("listitem")
        .map((item) => item.textContent),
    ).toEqual(["Um novo endpoint POST /refunds recebe o pagamento.", "O gateway o devolve."]);
    expect(within(screen.getByRole("list", { name: "Antes de ler" })).getByRole("listitem")).toHaveTextContent(
      "O diff também traz o trabalho de repasses.",
    );
    expect(translateOverview).toHaveBeenCalledExactlyOnceWith("p-1", REVIEW, "pt-BR", expect.stringMatching(/^translate-/));
    expect(planOverviewTranslation).toHaveBeenCalledExactlyOnceWith("p-1", REVIEW, "pt-BR");
  });
});
