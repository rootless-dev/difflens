import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { canSaveClaudeCommand, TerminalSection } from "@/features/preferences/TerminalSection";
import { setLanguage } from "@/i18n";
import {
  type ClaudeCommandSetting,
  getClaudeCommand,
  MAX_CLAUDE_COMMAND_CHARS,
  saveClaudeCommand,
  setClaudeCommandMode,
} from "@/lib/api";
import { apiError } from "@/test/fixtures";

vi.mock("@/lib/api", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/lib/api")>()),
  getClaudeCommand: vi.fn(),
  setClaudeCommandMode: vi.fn(),
  saveClaudeCommand: vi.fn(),
}));

/** The core as the mocks see it: what was saved, and what each call returns. */
let stored: ClaudeCommandSetting;

beforeEach(() => {
  vi.resetAllMocks();
  stored = { mode: "default", custom: null };
  vi.mocked(getClaudeCommand).mockImplementation(async () => stored);
  vi.mocked(setClaudeCommandMode).mockImplementation(async (mode) => {
    stored = { ...stored, mode };
    return stored;
  });
  vi.mocked(saveClaudeCommand).mockImplementation(async (command) => {
    stored = { mode: "custom", custom: command.trim() };
    return stored;
  });
});

async function renderSection(setting: ClaudeCommandSetting = stored) {
  stored = setting;
  const user = userEvent.setup();
  render(<TerminalSection />);
  const group = await screen.findByRole("radiogroup", { name: "Claude Code command" });
  return {
    user,
    group,
    defaultRadio: within(group).getByRole("radio", { name: "Default" }),
    customRadio: within(group).getByRole("radio", { name: "Custom command" }),
  };
}

const commandInput = () => screen.getByRole("textbox", { name: "Command" });
const queryCommandInput = () => screen.queryByRole("textbox", { name: "Command" });
const saveButton = () => screen.getByRole("button", { name: "Save" });

describe("the Claude Code command", () => {
  it("is a labelled radio group on Default until the user chooses another", async () => {
    const { group, defaultRadio, customRadio } = await renderSection();
    expect(screen.getByRole("region", { name: "Terminal" })).toBeInTheDocument();
    expect(group).toHaveAccessibleDescription(/Sessions already open keep theirs/);
    expect(defaultRadio).toBeChecked();
    expect(defaultRadio).toHaveAccessibleDescription("Runs claude.");
    expect(customRadio).not.toBeChecked();
    expect(queryCommandInput()).not.toBeInTheDocument();
  });

  it("offers an empty command to type when Custom is chosen with none saved; sessions keep the default", async () => {
    const { user, customRadio } = await renderSection();

    await user.click(customRadio);

    expect(setClaudeCommandMode).toHaveBeenCalledExactlyOnceWith("custom");
    expect(customRadio).toBeChecked();
    expect(commandInput()).toHaveValue("");
    expect(commandInput()).not.toHaveAttribute("readonly");
    expect(commandInput()).toHaveAccessibleDescription("Until you save a command, new sessions run claude.");
    expect(saveButton()).toBeDisabled();
    expect(screen.queryByRole("button", { name: "Cancel" })).not.toBeInTheDocument();
    expect(saveClaudeCommand).not.toHaveBeenCalled();
  });

  it("enables Save only for a command that is not blank", async () => {
    const { user, customRadio } = await renderSection();
    await user.click(customRadio);

    await user.type(commandInput(), "   ");
    expect(saveButton()).toBeDisabled();

    await user.type(commandInput(), "claude --model opus");
    expect(saveButton()).toBeEnabled();
  });

  it("saves the command trimmed, then shows it read-only with Edit focused", async () => {
    const { user, customRadio } = await renderSection();
    await user.click(customRadio);

    await user.type(commandInput(), "  claude --model opus ");
    await user.click(saveButton());

    expect(saveClaudeCommand).toHaveBeenCalledExactlyOnceWith("claude --model opus");
    const edit = await screen.findByRole("button", { name: "Edit" });
    expect(commandInput()).toHaveValue("claude --model opus");
    expect(commandInput()).toHaveAttribute("readonly");
    expect(commandInput()).not.toHaveAccessibleDescription();
    expect(edit).toHaveFocus();
    expect(screen.queryByRole("button", { name: "Save" })).not.toBeInTheDocument();
  });

  it("saves with Enter", async () => {
    const { user, customRadio } = await renderSection();
    await user.click(customRadio);

    await user.type(commandInput(), "/opt/bin/claude-wrapper{Enter}");

    expect(saveClaudeCommand).toHaveBeenCalledExactlyOnceWith("/opt/bin/claude-wrapper");
    expect(await screen.findByRole("button", { name: "Edit" })).toHaveFocus();
  });

  it("does not change a read-only command by typing in it", async () => {
    const { user } = await renderSection({ mode: "custom", custom: "claude --model opus" });

    await user.type(commandInput(), "x");

    expect(commandInput()).toHaveValue("claude --model opus");
  });

  it("makes the command editable with Edit, Save waiting for a change", async () => {
    const { user } = await renderSection({ mode: "custom", custom: "claude --model opus" });

    await user.click(screen.getByRole("button", { name: "Edit" }));

    expect(commandInput()).toHaveFocus();
    expect(commandInput()).not.toHaveAttribute("readonly");
    expect(saveButton()).toBeDisabled();
    await user.clear(commandInput());
    await user.type(commandInput(), " claude --model opus ");
    expect(saveButton()).toBeDisabled();
    await user.type(commandInput(), "--resume");
    expect(saveButton()).toBeEnabled();
  });

  it("cancels an edit back to the saved command, read-only, with Edit focused", async () => {
    const { user } = await renderSection({ mode: "custom", custom: "claude --model opus" });
    await user.click(screen.getByRole("button", { name: "Edit" }));
    await user.clear(commandInput());
    await user.type(commandInput(), "something else");

    await user.click(screen.getByRole("button", { name: "Cancel" }));

    expect(commandInput()).toHaveValue("claude --model opus");
    expect(commandInput()).toHaveAttribute("readonly");
    expect(screen.getByRole("button", { name: "Edit" })).toHaveFocus();
    expect(saveClaudeCommand).not.toHaveBeenCalled();
  });

  it("cancels an edit with Escape", async () => {
    const { user } = await renderSection({ mode: "custom", custom: "claude" });
    await user.click(screen.getByRole("button", { name: "Edit" }));
    await user.type(commandInput(), " --resume{Escape}");

    expect(commandInput()).toHaveValue("claude");
    expect(commandInput()).toHaveAttribute("readonly");
  });

  it("saves Default at once and keeps the custom command for when Custom is chosen again", async () => {
    const { user, defaultRadio, customRadio } = await renderSection({ mode: "custom", custom: "claude --model opus" });

    await user.click(defaultRadio);

    expect(setClaudeCommandMode).toHaveBeenCalledExactlyOnceWith("default");
    expect(defaultRadio).toBeChecked();
    expect(queryCommandInput()).not.toBeInTheDocument();
    expect(saveClaudeCommand).not.toHaveBeenCalled();

    await user.click(customRadio);

    expect(setClaudeCommandMode).toHaveBeenLastCalledWith("custom");
    expect(commandInput()).toHaveValue("claude --model opus");
    expect(commandInput()).toHaveAttribute("readonly");
    expect(screen.getByRole("button", { name: "Edit" })).toBeInTheDocument();
  });

  it("drops an unsaved edit when the user goes to Default and back", async () => {
    const { user, defaultRadio, customRadio } = await renderSection({ mode: "custom", custom: "claude" });
    await user.click(screen.getByRole("button", { name: "Edit" }));
    await user.type(commandInput(), " --unsaved");

    await user.click(defaultRadio);
    await user.click(customRadio);

    expect(commandInput()).toHaveValue("claude");
    expect(commandInput()).toHaveAttribute("readonly");
  });

  it("says why a command was not saved and keeps it editable, focused", async () => {
    vi.mocked(saveClaudeCommand).mockRejectedValue(
      apiError("internal", "The Claude Code command must be a single line, without control characters."),
    );
    const { user, customRadio } = await renderSection();
    await user.click(customRadio);
    await user.type(commandInput(), "claude --x");

    await user.click(saveButton());

    expect(await screen.findByRole("alert")).toHaveTextContent(
      "The command could not be saved: The Claude Code command must be a single line, without control characters.",
    );
    expect(commandInput()).not.toHaveAttribute("readonly");
    expect(commandInput()).toHaveFocus();
  });

  it("says when a choice could not be saved, leaving the one before", async () => {
    vi.mocked(setClaudeCommandMode).mockRejectedValue(apiError("internal", "Project storage failed: disk full"));
    const { user, defaultRadio, customRadio } = await renderSection();

    await user.click(customRadio);

    expect(await screen.findByRole("alert")).toHaveTextContent(
      "The choice could not be saved: Project storage failed: disk full",
    );
    expect(defaultRadio).toBeChecked();
    expect(queryCommandInput()).not.toBeInTheDocument();
  });

  it("offers to try again when the setting could not be read", async () => {
    vi.mocked(getClaudeCommand).mockRejectedValueOnce(apiError("internal", "Project storage failed: locked"));
    const user = userEvent.setup();
    render(<TerminalSection />);

    expect(await screen.findByRole("alert")).toHaveTextContent("Project storage failed: locked");
    await user.click(screen.getByRole("button", { name: "Try again" }));

    expect(await screen.findByRole("radiogroup", { name: "Claude Code command" })).toBeInTheDocument();
  });

  it("is in Brazilian Portuguese", async () => {
    setLanguage("pt-BR");
    stored = { mode: "custom", custom: "claude" };
    render(<TerminalSection />);
    const group = await screen.findByRole("radiogroup", { name: "Comando do Claude Code" });
    expect(within(group).getByRole("radio", { name: "Padrão" })).not.toBeChecked();
    expect(within(group).getByRole("radio", { name: "Padrão" })).toHaveAccessibleDescription("Executa claude.");
    expect(within(group).getByRole("radio", { name: "Comando customizado" })).toBeChecked();
    expect(screen.getByRole("textbox", { name: "Comando" })).toHaveAttribute("readonly");
    expect(screen.getByRole("button", { name: "Editar" })).toBeInTheDocument();
  });
});

describe("whether a command can be saved", () => {
  it.each([
    ["claude --model opus", null, true],
    ["  claude  ", "claude --model opus", true],
    ["", null, false],
    ["   ", null, false],
    ["claude", "claude", false],
    ["  claude ", "claude", false],
    ["claude\nrm -rf ~", null, false],
    ["claude\r", "x", true],
    ["claude\r--resume", null, false],
    ["x".repeat(MAX_CLAUDE_COMMAND_CHARS), null, true],
    ["x".repeat(MAX_CLAUDE_COMMAND_CHARS + 1), null, false],
  ] as const)("%j over %j: %s", (draft, saved, expected) => {
    expect(canSaveClaudeCommand(draft, saved)).toBe(expected);
  });
});
