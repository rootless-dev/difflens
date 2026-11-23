import { act, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { PreparingAnalyzers } from "@/components/preparing-analyzers";
import { FlowView } from "@/features/review/flow/FlowView";
import { applyProvision } from "@/hooks/use-analyzer-provisions";
import { setLanguage } from "@/i18n";
import { type AnalyzerProvision, getAnalyzerProvisions, listenToAnalyzerProvisions } from "@/lib/api";

vi.mock("@/lib/api", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/lib/api")>()),
  getAnalyzerProvisions: vi.fn(),
  listenToAnalyzerProvisions: vi.fn(),
}));

const python = (change: Partial<AnalyzerProvision> = {}): AnalyzerProvision => ({
  analyzer: "python",
  language: "Python",
  status: "preparing",
  step: null,
  message: null,
  ...change,
});

/** Answers the core's list with `running`, and returns what emits an event. */
function core(running: AnalyzerProvision[]) {
  let handler: ((event: AnalyzerProvision) => void) | null = null;
  vi.mocked(getAnalyzerProvisions).mockResolvedValue(running);
  vi.mocked(listenToAnalyzerProvisions).mockImplementation(async (listener) => {
    handler = listener;
    return () => {
      handler = null;
    };
  });
  return (event: AnalyzerProvision) => act(() => handler?.(event));
}

beforeEach(() => {
  vi.mocked(getAnalyzerProvisions).mockReset();
  vi.mocked(listenToAnalyzerProvisions).mockReset();
});

describe("preparing an analyzer", () => {
  it("shows the build going on with its step, then gives the view back", async () => {
    const emit = core([python({ step: "creating_environment" })]);
    render(<PreparingAnalyzers fallback={<p>Analysing the functions</p>} />);

    expect(await screen.findByText("Preparing the Python analyzer…")).toBeInTheDocument();
    expect(screen.getByText("Creating its environment")).toBeInTheDocument();
    expect(screen.getByRole("progressbar", { name: "Preparing the Python analyzer…" })).not.toHaveAttribute(
      "aria-valuenow",
    );
    expect(screen.queryByText("Analysing the functions")).not.toBeInTheDocument();

    emit(python({ step: "installing_dependencies" }));
    expect(screen.getByText("Installing its dependencies")).toBeInTheDocument();

    emit(python({ status: "ready" }));
    expect(screen.queryByText("Preparing the Python analyzer…")).not.toBeInTheDocument();
    expect(screen.getByText("Analysing the functions")).toBeInTheDocument();
  });

  it("shows nothing but the fallback when no analyzer is being prepared, also outside the app", async () => {
    vi.mocked(getAnalyzerProvisions).mockRejectedValue(new Error("no core"));
    vi.mocked(listenToAnalyzerProvisions).mockRejectedValue(new Error("no core"));
    render(<PreparingAnalyzers fallback={<p>Measuring</p>} />);
    expect(await screen.findByText("Measuring")).toBeInTheDocument();
    expect(screen.queryByRole("progressbar")).not.toBeInTheDocument();
  });

  it("reads in Brazilian Portuguese", async () => {
    setLanguage("pt-BR");
    core([python({ step: "compiling", analyzer: "rust", language: "Rust" })]);
    render(<PreparingAnalyzers />);
    expect(await screen.findByText("Preparando o analisador de Rust…")).toBeInTheDocument();
    expect(screen.getByText("Compilando")).toBeInTheDocument();
  });

  it("keeps each analyzer once, with its latest change, and drops it when it ends", () => {
    const started = applyProvision(new Map(), python());
    const stepped = applyProvision(started, python({ step: "installing_dependencies" }));
    const both = applyProvision(stepped, python({ analyzer: "rust", language: "Rust", step: "compiling" }));
    expect([...both.values()].map((event) => [event.analyzer, event.step])).toEqual([
      ["python", "installing_dependencies"],
      ["rust", "compiling"],
    ]);
    const failed = applyProvision(both, python({ status: "failed", message: "nope" }));
    expect([...failed.keys()]).toEqual(["rust"]);
  });

  it("says which analyzer could not be prepared, why, and offers to try again", async () => {
    core([]);
    const onRetry = vi.fn();
    render(
      <FlowView
        state={{
          status: "ready",
          pending: false,
          result: {
            kind: "unavailable",
            reason: "analyzer_unavailable",
            analyzer: "Python",
            message: {
              message: "The Python analyzer needs python3, which was not found on the PATH of your login shell. …",
              reason: "analyzer_runtime_missing",
              params: {
                analyzer: { message: "The Python analyzer", reason: "analyzer_title", params: { language: "Python" } },
                program: "python3",
                hint: { message: "Install Python 3…", reason: "runtime_python_hint" },
              },
            },
          },
        }}
        options={{ includeTests: false, includeCallers: false, includeCallees: false }}
        onOptionsChange={() => {}}
        files={[]}
        onRetry={onRetry}
        onViewDiff={() => {}}
        onOpenDiff={() => {}}
      />,
    );

    expect(screen.getByText("The Python analyzer could not be prepared.")).toBeInTheDocument();
    expect(
      screen.getByText(
        "The Python analyzer needs python3, which was not found on the PATH of your login shell. Install Python 3 (python.org, or brew install python), then try again.",
      ),
    ).toBeInTheDocument();
    act(() => screen.getByRole("button", { name: "Try again" }).click());
    expect(onRetry).toHaveBeenCalledOnce();
  });
});
