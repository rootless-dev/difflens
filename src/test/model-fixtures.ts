import {
  MODEL_TASKS,
  type ModelProviderKey,
  type ModelProviderState,
  type ModelSettings,
  type ModelTask,
  type ModelTaskState,
  type ModelTest,
} from "@/lib/api";

const NAMES: Record<ModelProviderKey, string> = {
  claude_cli: "Claude Code",
  anthropic: "Anthropic API",
  openai: "OpenAI API",
  gemini: "Google Gemini API",
  openrouter: "OpenRouter",
};

/** A provider as a fresh install has it: Claude Code found and ready, the APIs without a key. */
export function makeProviderState(
  provider: ModelProviderKey,
  overrides: Partial<ModelProviderState> = {},
): ModelProviderState {
  const cli = provider === "claude_cli";
  return {
    provider,
    name: NAMES[provider],
    needsKey: !cli,
    keySaved: false,
    keyUrl: cli ? "" : `https://keys.example.com/${provider}`,
    model: cli ? "opus" : provider === "anthropic" ? "claude-opus-5-5" : null,
    effort: provider === "anthropic" ? "high" : null,
    suggestedModels: cli
      ? [
          { id: "opus", name: "Opus (latest)" },
          { id: "sonnet", name: "Sonnet (latest)" },
        ]
      : [],
    cli: cli ? { path: "/Users/dana/.local/bin/claude", version: "2.1.328 (Claude Code)" } : null,
    ready: cli,
    configured: cli,
    ...overrides,
  };
}

/**
 * Settings with `active` the default provider; `changes` replace parts of single providers, `tasks` give tasks a
 * model of their own (every other one is on the default).
 */
export function makeModelSettings(
  active: ModelProviderKey = "claude_cli",
  changes: Partial<Record<ModelProviderKey, Partial<ModelProviderState>>> = {},
  tasks: Partial<Record<ModelTask, Partial<Omit<ModelTaskState, "task">>>> = {},
): ModelSettings {
  const keys: ModelProviderKey[] = ["claude_cli", "anthropic", "openai", "gemini", "openrouter"];
  return {
    activeProvider: active,
    providers: keys.map((key) => makeProviderState(key, changes[key])),
    tasks: MODEL_TASKS.map((task) => ({ task, choice: null, needsAttention: false, ...tasks[task] })),
  };
}

export function makeModelTest(overrides: Partial<ModelTest> = {}): ModelTest {
  return {
    provider: "claude_cli",
    providerName: "Claude Code",
    model: "claude-opus-5-5",
    reply: "Happy reviewing!",
    elapsedMs: 4200,
    inputTokens: 3012,
    outputTokens: 30,
    ...overrides,
  };
}
