import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { ModelSection } from "@/features/preferences/ModelSection";
import {
  cancelModelRun,
  getModelSettings,
  listModels,
  removeModelKey,
  saveModelChoice,
  setModelKey,
  setModelProvider,
  setModelTask,
  testModel,
} from "@/lib/api";
import { apiError, deferred } from "@/test/fixtures";
import { makeModelSettings, makeModelTest } from "@/test/model-fixtures";

vi.mock("@/lib/api", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/lib/api")>()),
  getModelSettings: vi.fn(),
  setModelProvider: vi.fn(),
  saveModelChoice: vi.fn(),
  setModelKey: vi.fn(),
  setModelTask: vi.fn(),
  removeModelKey: vi.fn(),
  listModels: vi.fn(),
  testModel: vi.fn(),
  cancelModelRun: vi.fn(),
}));

beforeEach(() => {
  vi.resetAllMocks();
  vi.mocked(getModelSettings).mockResolvedValue(makeModelSettings());
  vi.mocked(cancelModelRun).mockResolvedValue(undefined);
});

async function renderSection() {
  const view = render(<ModelSection />);
  const section = within(await screen.findByRole("region", { name: "Model" }));
  await section.findByRole("group", { name: "Model provider" });
  return { ...view, section };
}

const provider = (name: string) => within(screen.getByRole("group", { name }));

describe("Preferences › Model", () => {
  it("starts on Claude Code, shows where claude is, and says where the code goes", async () => {
    const { section } = await renderSection();
    expect(section.getByRole("button", { name: "Claude Code" })).toHaveAttribute("aria-pressed", "true");
    const cli = provider("Claude Code");
    expect(cli.getByText("/Users/dana/.local/bin/claude")).toBeInTheDocument();
    expect(cli.getByText(/2\.1\.328 \(Claude Code\)/)).toBeInTheDocument();
    expect(cli.queryByLabelText("API key")).not.toBeInTheDocument();
    expect(cli.getByLabelText("Model")).toHaveValue("opus");
    expect(
      section.getByText(/Using the model sends the code being reviewed .* to Anthropic, through Claude Code/),
    ).toBeInTheDocument();
    // Opening the screen asks no model.
    expect(testModel).not.toHaveBeenCalled();
    expect(listModels).not.toHaveBeenCalled();
  });

  it("says when claude is not found and does not offer Test", async () => {
    vi.mocked(getModelSettings).mockResolvedValue(
      makeModelSettings("claude_cli", { claude_cli: { cli: { path: null, version: null }, ready: false } }),
    );
    await renderSection();
    const cli = provider("Claude Code");
    expect(cli.getByText(/was not found\. Install Claude Code/)).toBeInTheDocument();
    expect(cli.getByRole("button", { name: "Test" })).toBeDisabled();
    expect(cli.getByText("Install Claude Code to test it.")).toBeInTheDocument();
  });

  it("switches provider, saves a key once and offers the models it lists", async () => {
    const user = userEvent.setup();
    vi.mocked(setModelProvider).mockResolvedValue(makeModelSettings("openai"));
    const withKey = makeModelSettings("openai", { openai: { keySaved: true } });
    vi.mocked(setModelKey).mockResolvedValue({
      settings: withKey,
      models: [
        { id: "gpt-5.5", name: "gpt-5.5" },
        { id: "gpt-4.1", name: "gpt-4.1" },
      ],
    });
    const { section, container } = await renderSection();

    await user.click(section.getByRole("button", { name: "OpenAI" }));
    expect(setModelProvider).toHaveBeenCalledWith("openai");
    const openai = provider("OpenAI API");
    expect(openai.getByText("No key")).toBeInTheDocument();
    expect(openai.getByRole("button", { name: "List models" })).toBeDisabled();
    expect(openai.getByRole("button", { name: "Test" })).toBeDisabled();
    expect(section.getByText(/sends the code being reviewed .* to OpenAI\./)).toBeInTheDocument();

    const key = openai.getByLabelText("API key");
    expect(key).toHaveAttribute("type", "password");
    await user.type(key, " sk-proj-secret ");
    await user.click(openai.getByRole("button", { name: "Save key" }));

    expect(setModelKey).toHaveBeenCalledWith("openai", "sk-proj-secret");
    expect(await openai.findByText(/Key saved in the Keychain\. 2 models found/)).toBeInTheDocument();
    expect(key).toHaveValue("");
    expect(openai.getByText("Key saved")).toBeInTheDocument();
    expect(openai.getByRole("button", { name: "Remove key" })).toBeInTheDocument();
    const options = Array.from(container.querySelectorAll("datalist option")).map((option) =>
      option.getAttribute("value"),
    );
    expect(options).toEqual(["gpt-5.5", "gpt-4.1"]);
    // The key never shows up again anywhere on the screen.
    expect(container.innerHTML).not.toContain("sk-proj-secret");
  });

  it("shows why a key was refused", async () => {
    const user = userEvent.setup();
    vi.mocked(getModelSettings).mockResolvedValue(makeModelSettings("gemini"));
    vi.mocked(setModelKey).mockRejectedValue(
      apiError("auth_failed", "The Google Gemini API did not accept the API key. It may be wrong or revoked."),
    );
    await renderSection();
    const gemini = provider("Google Gemini API");
    await user.type(gemini.getByLabelText("API key"), "AIza-bad");
    await user.click(gemini.getByRole("button", { name: "Save key" }));
    expect(await gemini.findByRole("alert")).toHaveTextContent("did not accept the API key");
    expect(gemini.getByText("No key")).toBeInTheDocument();
  });

  it("saves the model and the effort of the Anthropic API, then removes its key", async () => {
    const user = userEvent.setup();
    const withKey = makeModelSettings("anthropic", { anthropic: { keySaved: true, ready: true } });
    vi.mocked(getModelSettings).mockResolvedValue(withKey);
    vi.mocked(listModels).mockResolvedValue([{ id: "claude-sonnet-5-5", name: "Claude Sonnet 5.5" }]);
    vi.mocked(saveModelChoice).mockResolvedValue(
      makeModelSettings("anthropic", {
        anthropic: { keySaved: true, ready: true, model: "claude-sonnet-5-5", effort: "max" },
      }),
    );
    vi.mocked(removeModelKey).mockResolvedValue(makeModelSettings("anthropic"));
    await renderSection();
    const anthropic = provider("Anthropic API");
    const effort = () => within(anthropic.getByRole("group", { name: "Effort" }));
    expect(effort().getByRole("button", { name: "high" })).toHaveAttribute("aria-pressed", "true");

    await user.click(anthropic.getByRole("button", { name: "List models" }));
    expect(listModels).toHaveBeenCalledWith("anthropic");
    expect(await anthropic.findByText("1 model found; pick one under Model.")).toBeInTheDocument();

    const model = anthropic.getByLabelText("Model");
    await user.clear(model);
    await user.type(model, "claude-sonnet-5-5");
    await user.click(effort().getByRole("button", { name: "max" }));
    // Test waits for the change to be saved.
    expect(anthropic.getByRole("button", { name: "Test" })).toBeDisabled();
    expect(anthropic.getByText("Save the model first.")).toBeInTheDocument();
    await user.click(anthropic.getByRole("button", { name: "Save" }));
    expect(saveModelChoice).toHaveBeenCalledWith("anthropic", "claude-sonnet-5-5", "max");
    expect(await anthropic.findByText("Saved.")).toBeInTheDocument();
    expect(anthropic.getByRole("button", { name: "Test" })).toBeEnabled();

    await user.click(anthropic.getByRole("button", { name: "Remove key" }));
    expect(removeModelKey).toHaveBeenCalledWith("anthropic");
    expect(await anthropic.findByText("The key was deleted from the Keychain.")).toBeInTheDocument();
    expect(anthropic.getByText("No key")).toBeInTheDocument();
  });

  it("tests the provider in use and shows the reply, the model and the time", async () => {
    const user = userEvent.setup();
    vi.mocked(testModel).mockResolvedValue(makeModelTest());
    await renderSection();
    const cli = provider("Claude Code");
    await user.click(cli.getByRole("button", { name: "Test" }));
    expect(testModel).toHaveBeenCalledOnce();
    const shown = await cli.findByText(/Answered by/);
    expect(shown.closest("p")).toHaveTextContent(
      "“Happy reviewing!” Answered by claude-opus-5-5 (Claude Code) in 4.2 s, 3012 tokens in and 30 out.",
    );
  });

  it("shows why a test failed", async () => {
    const user = userEvent.setup();
    vi.mocked(testModel).mockRejectedValue(
      apiError("model_refused", "The model of the Claude Code declined to answer."),
    );
    await renderSection();
    const cli = provider("Claude Code");
    await user.click(cli.getByRole("button", { name: "Test" }));
    expect(await cli.findByRole("alert")).toHaveTextContent("declined to answer");
  });

  it("stops a running test with its run id", async () => {
    const user = userEvent.setup();
    const answer = deferred<ReturnType<typeof makeModelTest>>();
    vi.mocked(testModel).mockReturnValue(answer.promise);
    vi.mocked(cancelModelRun).mockImplementation(async () => {
      answer.reject(apiError("cancelled", "Stopped."));
    });
    await renderSection();
    const cli = provider("Claude Code");
    await user.click(cli.getByRole("button", { name: "Test" }));
    expect(cli.getByRole("button", { name: "Testing…" })).toBeDisabled();

    await user.click(cli.getByRole("button", { name: "Stop" }));
    const runId = vi.mocked(testModel).mock.calls[0][0];
    expect(cancelModelRun).toHaveBeenCalledWith(runId);
    expect(await cli.findByText("Stopped.")).toBeInTheDocument();
    expect(cli.queryByRole("alert")).not.toBeInTheDocument();
    expect(cli.getByRole("button", { name: "Test" })).toBeEnabled();
  });

  it("puts every task on the default model, and offers only the providers that are set up", async () => {
    const user = userEvent.setup();
    vi.mocked(getModelSettings).mockResolvedValue(
      makeModelSettings("claude_cli", { anthropic: { keySaved: true, ready: true, configured: true } }),
    );
    const { section } = await renderSection();
    const tasks = within(section.getByRole("region", { name: "Model per task" }));
    const names = ["Insights", "Summary", "Short explanation", "Detailed explanation", "Test diagnosis", "Translation"];
    for (const name of names) {
      const row = within(tasks.getByRole("group", { name }));
      expect(row.getByText("Uses the default: Claude Code, model opus.")).toBeInTheDocument();
      expect(row.getByRole("combobox", { name: `Model for ${name}` })).toHaveTextContent("Use the default");
    }
    // In the order of MODEL_TASKS; the test diagnosis says where it is used.
    const rows = names.map((name) => tasks.getByRole("group", { name }));
    for (const [index, row] of rows.slice(1).entries()) {
      expect(rows[index].compareDocumentPosition(row) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    }
    const diagnosis = within(tasks.getByRole("group", { name: "Test diagnosis" }));
    expect(diagnosis.getByText("Explain the failure, on a failing test of the Tests tab.")).toBeInTheDocument();

    await user.click(tasks.getByRole("combobox", { name: "Model for Insights" }));
    const options = await screen.findAllByRole("option");
    expect(options.map((option) => option.textContent)).toEqual(["Use the default", "Claude Code", "Anthropic API"]);
    expect(setModelTask).not.toHaveBeenCalled();
    expect(testModel).not.toHaveBeenCalled();
  });

  it("gives a task a model of its own with the default's controls, and puts it back on the default", async () => {
    const user = userEvent.setup();
    const configured = { anthropic: { keySaved: true, ready: true, configured: true } };
    vi.mocked(getModelSettings).mockResolvedValue(makeModelSettings("claude_cli", configured));
    const chosen = { provider: "anthropic", model: "claude-sonnet-5-5", effort: "low" } as const;
    vi.mocked(setModelTask).mockResolvedValueOnce(
      makeModelSettings("claude_cli", configured, { insights: { choice: chosen } }),
    );
    vi.mocked(setModelTask).mockResolvedValueOnce(makeModelSettings("claude_cli", configured));
    const { section } = await renderSection();
    const tasks = within(section.getByRole("region", { name: "Model per task" }));
    const insights = within(tasks.getByRole("group", { name: "Insights" }));

    await user.click(insights.getByRole("combobox", { name: "Model for Insights" }));
    await user.click(await screen.findByRole("option", { name: "Anthropic API" }));
    // Nothing saved until Save; the default's model and effort to start from.
    expect(setModelTask).not.toHaveBeenCalled();
    const model = insights.getByLabelText("Model");
    expect(model).toHaveValue("claude-opus-5-5");
    const effort = () => within(insights.getByRole("group", { name: "Effort" }));
    expect(effort().getByRole("button", { name: "high" })).toHaveAttribute("aria-pressed", "true");
    await user.clear(model);
    expect(insights.getByRole("button", { name: "Save" })).toBeDisabled();
    await user.type(model, "claude-sonnet-5-5");
    await user.click(effort().getByRole("button", { name: "low" }));
    await user.click(insights.getByRole("button", { name: "Save" }));
    expect(setModelTask).toHaveBeenCalledExactlyOnceWith("insights", chosen);
    expect(await insights.findByText("Saved.")).toBeInTheDocument();
    expect(insights.getByText("Uses Anthropic API, model claude-sonnet-5-5.")).toBeInTheDocument();
    expect(insights.getByRole("button", { name: "Save" })).toBeDisabled();
    // The other tasks and the default stay as they were.
    expect(
      within(tasks.getByRole("group", { name: "Summary" })).getByText("Uses the default: Claude Code, model opus."),
    ).toBeInTheDocument();
    expect(section.getByRole("button", { name: "Claude Code" })).toHaveAttribute("aria-pressed", "true");

    await user.click(insights.getByRole("combobox", { name: "Model for Insights" }));
    await user.click(await screen.findByRole("option", { name: "Use the default" }));
    expect(setModelTask).toHaveBeenLastCalledWith("insights", null);
    expect(await insights.findByText("Uses the default: Claude Code, model opus.")).toBeInTheDocument();
    expect(insights.queryByLabelText("Model")).not.toBeInTheDocument();
  });

  it("says when the provider chosen for a task is no longer set up", async () => {
    const user = userEvent.setup();
    vi.mocked(getModelSettings).mockResolvedValue(
      makeModelSettings("claude_cli", {}, {
        summary: { choice: { provider: "openai", model: "gpt-5.5", effort: null }, needsAttention: true },
      }),
    );
    const { section } = await renderSection();
    const tasks = within(section.getByRole("region", { name: "Model per task" }));
    const summary = within(tasks.getByRole("group", { name: "Summary" }));
    expect(summary.getByText("Uses OpenAI API, model gpt-5.5.")).toBeInTheDocument();
    expect(
      summary.getByText(
        "OpenAI API is not set up any more, so calls for this task fail until you set it up again or choose another " +
          "model. It does not fall back to the default.",
      ),
    ).toBeInTheDocument();
    const picker = summary.getByRole("combobox", { name: "Model for Summary" });
    expect(picker).toHaveTextContent("OpenAI API (not set up)");
    // Still chosen, but it can only be left for the default or a provider that is set up.
    await user.click(picker);
    expect(await screen.findByRole("option", { name: "OpenAI API (not set up)" })).toHaveAttribute(
      "aria-disabled",
      "true",
    );
    expect(screen.getAllByRole("option").map((option) => option.textContent)).toEqual([
      "Use the default",
      "Claude Code",
      "OpenAI API (not set up)",
    ]);
    // The other tasks need nothing.
    expect(within(tasks.getByRole("group", { name: "Insights" })).queryByText(/not set up any more/)).toBeNull();
  });

  it("stops a running test when the screen closes", async () => {
    const user = userEvent.setup();
    vi.mocked(testModel).mockReturnValue(new Promise(() => {}));
    const { unmount } = await renderSection();
    await user.click(provider("Claude Code").getByRole("button", { name: "Test" }));
    const runId = vi.mocked(testModel).mock.calls[0][0];
    unmount();
    expect(cancelModelRun).toHaveBeenCalledWith(runId);
  });
});
