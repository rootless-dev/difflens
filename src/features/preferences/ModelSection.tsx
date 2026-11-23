import { type FormEvent, useEffect, useId, useRef, useState } from "react";
import { Trans, useTranslation } from "react-i18next";

import { ErrorNote, Hint } from "@/components/page";
import { Segment, SegmentGroup } from "@/components/segment";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Skeleton } from "@/components/ui/skeleton";
import { PreferencesSection } from "@/features/preferences/PreferencesSection";
import { modelName } from "@/features/review/insights/insights-text";
import { useAsyncData } from "@/hooks/use-async-data";
import { i18n } from "@/i18n";
import {
  cancelModelRun,
  EFFORTS,
  type Effort,
  getModelSettings,
  isApiError,
  listModels,
  type ModelInfo,
  type ModelProviderKey,
  type ModelProviderState,
  type ModelSettings,
  type ModelTaskChoice,
  type ModelTaskState,
  type ModelTest,
  removeModelKey,
  saveModelChoice,
  setModelKey,
  setModelProvider,
  setModelTask,
  testModel,
} from "@/lib/api";
import { errorMessage, formatDuration } from "@/lib/format";
import { cn } from "@/lib/utils";

/** Shorter names for the provider picker. */
const SHORT_NAME: Record<ModelProviderKey, string> = {
  claude_cli: "Claude Code",
  anthropic: "Anthropic",
  openai: "OpenAI",
  gemini: "Gemini",
  openrouter: "OpenRouter",
};

/** What the last action found. */
type Outcome = { tone: "ok" | "error" | "neutral"; text: string };

function newRunId(): string {
  return globalThis.crypto?.randomUUID?.() ?? `run-${Date.now()}-${Math.random().toString(36).slice(2)}`;
}

/**
 * Preferences › Model: the default model (who reads the code, with what key and model, and a Test that asks it a
 * tiny question), and a model of its own for any task that should not use the default.
 */
export function ModelSection() {
  const { t } = useTranslation();
  const { state, reload, update } = useAsyncData(getModelSettings);
  return (
    <PreferencesSection title={t("preferences.model.title")}>
      {state.status === "loading" && (
        <div role="status" aria-label={t("preferences.model.loading")} className="grid gap-2 py-3">
          <Skeleton className="h-3.5 w-1/3" />
          <Skeleton className="h-3.5 w-2/3" />
        </div>
      )}
      {state.status === "error" && (
        <div className="py-3">
          <ErrorNote>{state.message}</ErrorNote>
          <Button variant="secondary" size="sm" className="mt-2.5" onClick={reload}>
            {t("common.actions.tryAgain")}
          </Button>
        </div>
      )}
      {state.status === "ready" && <ModelSettingsForm settings={state.data} onChange={(next) => update(() => next)} />}
    </PreferencesSection>
  );
}

interface ModelSettingsFormProps {
  settings: ModelSettings;
  onChange: (next: ModelSettings) => void;
}

function ModelSettingsForm({ settings, onChange }: ModelSettingsFormProps) {
  const { t } = useTranslation();
  const [switching, setSwitching] = useState(false);
  const [switchError, setSwitchError] = useState<string | null>(null);
  const active =
    settings.providers.find((provider) => provider.provider === settings.activeProvider) ?? settings.providers[0];

  async function choose(provider: ModelProviderKey) {
    if (provider === settings.activeProvider || switching) return;
    setSwitching(true);
    setSwitchError(null);
    try {
      onChange(await setModelProvider(provider));
    } catch (cause: unknown) {
      setSwitchError(errorMessage(cause));
    } finally {
      setSwitching(false);
    }
  }

  return (
    <>
      <div className="grid grid-cols-[1fr_auto] items-center gap-4 border-b px-2 py-3">
        <div>
          <strong className="block font-medium">{t("preferences.model.who")}</strong>
          <span className="text-muted-foreground">{t("preferences.model.whoNote")}</span>
        </div>
        <SegmentGroup label={t("preferences.model.providerLabel")}>
          {settings.providers.map((provider) => (
            <Segment
              key={provider.provider}
              pressed={provider.provider === settings.activeProvider}
              title={provider.name}
              onClick={() => void choose(provider.provider)}
            >
              {SHORT_NAME[provider.provider]}
            </Segment>
          ))}
        </SegmentGroup>
      </div>
      {switchError && <ErrorNote className="px-2 pt-2">{switchError}</ErrorNote>}
      <ProviderPanel key={active.provider} state={active} onChange={onChange} />
      <Hint className="px-2">
        {t("preferences.model.privacy", { recipient: t(`preferences.model.recipient.${active.provider}`) })}
      </Hint>
      <TaskModels settings={settings} onChange={onChange} />
    </>
  );
}

/**
 * Model per task: each task on the default model, or on a model of its own from a provider that is set up. A task
 * whose provider stopped being set up says so here; its calls fail until it is fixed, never falling back on their own.
 */
function TaskModels({ settings, onChange }: ModelSettingsFormProps) {
  const { t } = useTranslation();
  const id = useId();
  return (
    <section aria-labelledby={id} className="mt-5 border-t">
      <div className="border-b px-2 py-3">
        <h3 id={id} className="font-medium">
          {t("preferences.model.tasks.title")}
        </h3>
        <span className="text-muted-foreground">{t("preferences.model.tasks.note")}</span>
      </div>
      {settings.tasks.map((task) => (
        <TaskRow key={task.task} state={task} settings={settings} onChange={onChange} />
      ))}
      <Hint className="px-2">{t("preferences.model.tasks.setUpHint")}</Hint>
    </section>
  );
}

/** Choosing "Use the default" in a task's picker. */
const DEFAULT_CHOICE = "default";

interface TaskRowProps {
  state: ModelTaskState;
  settings: ModelSettings;
  onChange: (next: ModelSettings) => void;
}

/**
 * One task: which model it uses, a picker of the default and the providers set up, and, for a model of its own, the
 * same model and effort controls as the default's, saved with Save. Back to the default is saved at once.
 */
function TaskRow({ state, settings, onChange }: TaskRowProps) {
  const { t } = useTranslation();
  const modelId = useId();
  const listId = useId();
  const providerOf = (key: ModelProviderKey) =>
    settings.providers.find((provider) => provider.provider === key) ?? settings.providers[0];
  const [draft, setDraft] = useState<ModelTaskChoice | null>(state.choice);
  const [models, setModels] = useState<ModelInfo[]>(
    state.choice ? providerOf(state.choice.provider).suggestedModels : [],
  );
  const [pending, setPending] = useState<"save" | "list" | null>(null);
  const [outcome, setOutcome] = useState<Outcome | null>(null);

  const name = t(`preferences.model.tasks.names.${state.task}`);
  const saved = state.choice;
  const dirty =
    draft !== null &&
    (saved === null ||
      draft.provider !== saved.provider ||
      draft.model.trim() !== saved.model ||
      draft.effort !== saved.effort);
  const defaultState = providerOf(settings.activeProvider);
  const summary = saved
    ? t("preferences.model.tasks.usesOwn", {
        model: modelName({ providerName: providerOf(saved.provider).name, model: saved.model }),
      })
    : t("preferences.model.tasks.usesDefault", {
        model: modelName({ providerName: defaultState.name, model: defaultState.model }),
      });

  // The default and the providers set up; the one chosen before stays listed, unusable, once it is not set up.
  const options = [
    { value: DEFAULT_CHOICE, label: t("preferences.model.tasks.useDefault"), disabled: false },
    ...settings.providers
      .filter((provider) => provider.configured || provider.provider === saved?.provider)
      .map((provider) => ({
        value: provider.provider,
        label: provider.configured
          ? provider.name
          : t("preferences.model.tasks.notSetUp", { provider: provider.name }),
        disabled: !provider.configured,
      })),
  ];

  async function act(kind: "save" | "list", action: () => Promise<Outcome>) {
    setPending(kind);
    setOutcome(null);
    try {
      setOutcome(await action());
    } catch (cause: unknown) {
      setOutcome({ tone: "error", text: errorMessage(cause) });
    } finally {
      setPending(null);
    }
  }

  const save = (choice: ModelTaskChoice | null) =>
    act("save", async () => {
      const next = await setModelTask(state.task, choice);
      onChange(next);
      setDraft(next.tasks.find((task) => task.task === state.task)?.choice ?? null);
      return { tone: "ok", text: i18n.t("preferences.model.saved") };
    });

  function pick(value: string) {
    if (value === DEFAULT_CHOICE) {
      setDraft(null);
      if (saved !== null) void save(null);
      return;
    }
    const provider = providerOf(value as ModelProviderKey);
    const same = saved?.provider === provider.provider ? saved : null;
    setDraft({
      provider: provider.provider,
      model: same?.model ?? provider.model ?? "",
      effort: same ? same.effort : provider.effort,
    });
    setModels(provider.suggestedModels);
    setOutcome(null);
  }

  const submit = (event: FormEvent) => {
    event.preventDefault();
    if (draft === null || !dirty || draft.model.trim() === "" || pending !== null) return;
    void save({ ...draft, model: draft.model.trim() });
  };

  const refreshModels = (provider: ModelProviderKey) =>
    act("list", async () => {
      const listed = await listModels(provider);
      setModels(listed);
      return { tone: "ok", text: i18n.t("preferences.model.modelsFound", { count: listed.length }) };
    });

  const draftProvider = draft ? providerOf(draft.provider) : null;
  return (
    <div role="group" aria-label={name} className="grid gap-2.5 border-b px-2 py-3">
      <div className="grid grid-cols-[1fr_auto] items-center gap-4">
        <div>
          <strong className="block font-medium">{name}</strong>
          <span className="text-muted-foreground">{t(`preferences.model.tasks.descriptions.${state.task}`)}</span>
          <span className="block text-xs text-muted-foreground">{summary}</span>
        </div>
        <Select
          items={options}
          value={draft?.provider ?? DEFAULT_CHOICE}
          onValueChange={(next) => {
            if (next !== null) pick(next);
          }}
        >
          <SelectTrigger
            aria-label={t("preferences.model.tasks.providerLabel", { task: name })}
            disabled={pending !== null}
            className="min-w-40"
          >
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            {options.map((option) => (
              <SelectItem key={option.value} value={option.value} disabled={option.disabled}>
                {option.label}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      </div>
      {state.needsAttention && saved && (
        <ErrorNote>{t("preferences.model.tasks.attention", { provider: providerOf(saved.provider).name })}</ErrorNote>
      )}
      {draft && draftProvider && (
        <form
          aria-label={t("preferences.model.modelForm", { provider: draftProvider.name })}
          onSubmit={submit}
          className="grid gap-2.5"
        >
          <div className="grid grid-cols-[150px_1fr] items-center gap-x-4 gap-y-2.5">
            <Label htmlFor={modelId}>{t("preferences.model.model")}</Label>
            <div className="flex items-center gap-2.5">
              <Input
                id={modelId}
                list={listId}
                value={draft.model}
                onChange={(event) => setDraft({ ...draft, model: event.target.value })}
                placeholder={t("preferences.model.modelPlaceholder")}
                disabled={pending !== null}
                autoComplete="off"
                spellCheck={false}
              />
              <datalist id={listId}>
                {models.map((option) => (
                  <option key={option.id} value={option.id}>
                    {option.name}
                  </option>
                ))}
              </datalist>
              {draftProvider.needsKey && (
                <Button
                  variant="secondary"
                  size="sm"
                  disabled={!draftProvider.configured || pending !== null}
                  onClick={() => void refreshModels(draftProvider.provider)}
                >
                  {pending === "list" ? t("preferences.model.listing") : t("preferences.model.listModels")}
                </Button>
              )}
            </div>
            {draft.effort !== null && (
              <>
                <span className="text-sm font-medium">{t("preferences.model.effort")}</span>
                <SegmentGroup label={t("preferences.model.effort")} className="justify-self-start">
                  {EFFORTS.map((level) => (
                    <Segment
                      key={level}
                      pressed={draft.effort === level}
                      onClick={() => setDraft({ ...draft, effort: level })}
                    >
                      {t(`preferences.model.efforts.${level}`)}
                    </Segment>
                  ))}
                </SegmentGroup>
              </>
            )}
          </div>
          <div className="ml-[166px] flex flex-wrap items-center gap-2.5">
            <Button type="submit" size="sm" disabled={!dirty || draft.model.trim() === "" || pending !== null}>
              {pending === "save" ? t("preferences.saving") : t("common.actions.save")}
            </Button>
            {outcome && <OutcomeText outcome={outcome} />}
          </div>
        </form>
      )}
      {!draft && outcome && <OutcomeText outcome={outcome} />}
    </div>
  );
}

type Pending = "key" | "remove" | "list" | "save";

interface ProviderPanelProps {
  state: ModelProviderState;
  onChange: (next: ModelSettings) => void;
}

/** The provider in use: its key (or the claude command), its model and effort, and Test. */
function ProviderPanel({ state, onChange }: ProviderPanelProps) {
  const { t } = useTranslation();
  const keyId = useId();
  const modelId = useId();
  const listId = useId();
  const [key, setKey] = useState("");
  const [model, setModel] = useState(state.model ?? "");
  const [effort, setEffort] = useState<Effort | null>(state.effort);
  const [models, setModels] = useState<ModelInfo[]>(state.suggestedModels);
  const [pending, setPending] = useState<Pending | null>(null);
  const [outcome, setOutcome] = useState<Outcome | null>(null);

  const dirty = model.trim() !== (state.model ?? "") || effort !== state.effort;
  const canList = !state.needsKey || state.keySaved;

  async function act(kind: Pending, action: () => Promise<Outcome>) {
    setPending(kind);
    setOutcome(null);
    try {
      setOutcome(await action());
    } catch (cause: unknown) {
      setOutcome({ tone: "error", text: errorMessage(cause) });
    } finally {
      setPending(null);
    }
  }

  const saveKey = (event: FormEvent) => {
    event.preventDefault();
    const typed = key.trim();
    if (typed === "" || pending !== null) return;
    void act("key", async () => {
      const saved = await setModelKey(state.provider, typed);
      setKey("");
      setModels(saved.models);
      onChange(saved.settings);
      return { tone: "ok", text: i18n.t("preferences.model.keySavedFound", { count: saved.models.length }) };
    });
  };

  const removeKey = () =>
    act("remove", async () => {
      onChange(await removeModelKey(state.provider));
      setModels([]);
      return { tone: "ok", text: i18n.t("preferences.model.keyRemoved") };
    });

  const refreshModels = () =>
    act("list", async () => {
      const listed = await listModels(state.provider);
      setModels(listed);
      return { tone: "ok", text: i18n.t("preferences.model.modelsFound", { count: listed.length }) };
    });

  const saveChoice = (event: FormEvent) => {
    event.preventDefault();
    if (!dirty || pending !== null) return;
    void act("save", async () => {
      const saved = await saveModelChoice(state.provider, model.trim(), effort);
      onChange(saved);
      return { tone: "ok", text: i18n.t("preferences.model.saved") };
    });
  };

  return (
    <div className="grid gap-3 border-b px-2 py-3" aria-label={state.name} role="group">
      {state.cli ? (
        <ClaudeCommand cli={state.cli} />
      ) : (
        <form
          aria-label={t("preferences.model.keyForm", { provider: state.name })}
          onSubmit={saveKey}
          className="grid gap-2.5"
        >
          <div className="grid grid-cols-[150px_1fr] items-center gap-x-4">
            <Label htmlFor={keyId}>{t("preferences.model.key")}</Label>
            <Input
              id={keyId}
              type="password"
              value={key}
              onChange={(event) => setKey(event.target.value)}
              placeholder={
                state.keySaved ? t("preferences.model.keyPlaceholderSaved") : t("preferences.model.keyPlaceholder")
              }
              disabled={pending !== null}
              autoComplete="off"
              spellCheck={false}
            />
          </div>
          <div className="ml-[166px] flex flex-wrap items-center gap-2.5">
            <Button type="submit" size="sm" disabled={key.trim() === "" || pending !== null}>
              {pending === "key" ? t("preferences.model.checking") : t("preferences.model.saveKey")}
            </Button>
            {state.keySaved && (
              <Button variant="destructive" size="sm" disabled={pending !== null} onClick={() => void removeKey()}>
                {pending === "remove" ? t("preferences.removing") : t("preferences.model.removeKey")}
              </Button>
            )}
            <span role="status" className="text-xs text-muted-foreground">
              {state.keySaved ? t("preferences.model.keySaved") : t("preferences.model.noKey")}
            </span>
          </div>
          <Hint className="mt-0 ml-[166px]">
            <Trans i18nKey="preferences.model.keyHint" values={{ url: state.keyUrl }} components={{ code: <code /> }} />
          </Hint>
        </form>
      )}

      <form
        aria-label={t("preferences.model.modelForm", { provider: state.name })}
        onSubmit={saveChoice}
        className="grid gap-2.5"
      >
        <div className="grid grid-cols-[150px_1fr] items-center gap-x-4 gap-y-2.5">
          <Label htmlFor={modelId}>{t("preferences.model.model")}</Label>
          <div className="flex items-center gap-2.5">
            <Input
              id={modelId}
              list={listId}
              value={model}
              onChange={(event) => setModel(event.target.value)}
              placeholder={
                canList ? t("preferences.model.modelPlaceholder") : t("preferences.model.modelPlaceholderNoKey")
              }
              disabled={pending !== null}
              autoComplete="off"
              spellCheck={false}
            />
            <datalist id={listId}>
              {models.map((option) => (
                <option key={option.id} value={option.id}>
                  {option.name}
                </option>
              ))}
            </datalist>
            {state.needsKey && (
              <Button
                variant="secondary"
                size="sm"
                disabled={!canList || pending !== null}
                onClick={() => void refreshModels()}
              >
                {pending === "list" ? t("preferences.model.listing") : t("preferences.model.listModels")}
              </Button>
            )}
          </div>
          {effort !== null && (
            <>
              <span className="text-sm font-medium">{t("preferences.model.effort")}</span>
              <SegmentGroup label={t("preferences.model.effort")} className="justify-self-start">
                {EFFORTS.map((level) => (
                  <Segment key={level} pressed={effort === level} onClick={() => setEffort(level)}>
                    {t(`preferences.model.efforts.${level}`)}
                  </Segment>
                ))}
              </SegmentGroup>
            </>
          )}
        </div>
        {effort !== null && <Hint className="mt-0 ml-[166px]">{t("preferences.model.effortHint")}</Hint>}
        <div className="ml-[166px] flex flex-wrap items-center gap-2.5">
          <Button type="submit" size="sm" disabled={!dirty || pending !== null}>
            {pending === "save" ? t("preferences.saving") : t("common.actions.save")}
          </Button>
          {outcome && <OutcomeText outcome={outcome} />}
        </div>
      </form>

      <TestRow state={state} blocked={dirty ? t("preferences.model.saveModelFirst") : null} />
    </div>
  );
}

function ClaudeCommand({ cli }: { cli: NonNullable<ModelProviderState["cli"]> }) {
  const { t } = useTranslation();
  return (
    <dl className="grid grid-cols-[150px_1fr] items-center gap-x-4 gap-y-2.5">
      <dt className="text-muted-foreground">{t("preferences.model.command")}</dt>
      <dd role="status">
        {cli.path ? (
          <>
            <Trans i18nKey="preferences.model.cliFound" values={{ path: cli.path }} components={{ code: <code /> }} />
            {cli.version && <span className="text-muted-foreground"> ({cli.version})</span>}
          </>
        ) : (
          <span className="text-destructive">
            <Trans i18nKey="preferences.model.cliMissing" components={{ code: <code /> }} />
          </span>
        )}
      </dd>
      <dt className="text-muted-foreground">{t("preferences.model.signedIn")}</dt>
      <dd>{t("preferences.model.signedInNote")}</dd>
    </dl>
  );
}

/** Test: one tiny question to the provider in use, which can be stopped while it runs. */
function TestRow({ state, blocked }: { state: ModelProviderState; blocked: string | null }) {
  const { t } = useTranslation();
  const [running, setRunning] = useState<string | null>(null);
  const [result, setResult] = useState<ModelTest | null>(null);
  const [outcome, setOutcome] = useState<Outcome | null>(null);
  const runRef = useRef<string | null>(null);

  // Leaving the screen, or switching provider, stops a test still running.
  useEffect(
    () => () => {
      if (runRef.current) void cancelModelRun(runRef.current);
    },
    [],
  );

  async function run() {
    const runId = newRunId();
    runRef.current = runId;
    setRunning(runId);
    setResult(null);
    setOutcome(null);
    try {
      const answer = await testModel(runId);
      if (runRef.current === runId) setResult(answer);
    } catch (cause: unknown) {
      if (runRef.current !== runId) return;
      setOutcome(
        isApiError(cause) && cause.code === "cancelled"
          ? { tone: "neutral", text: i18n.t("preferences.model.stopped") }
          : { tone: "error", text: errorMessage(cause) },
      );
    } finally {
      if (runRef.current === runId) {
        runRef.current = null;
        setRunning(null);
      }
    }
  }

  const reason = !state.ready ? notReadyReason(state) : blocked;

  return (
    <div className="ml-[166px] grid gap-2">
      <div className="flex flex-wrap items-center gap-2.5">
        <Button variant="secondary" size="sm" disabled={running !== null || reason !== null} onClick={() => void run()}>
          {running ? t("preferences.testing") : t("preferences.model.test")}
        </Button>
        {running && (
          <Button variant="secondary" size="sm" onClick={() => void cancelModelRun(running)}>
            {t("preferences.model.stop")}
          </Button>
        )}
        {reason && !running && <span className="text-xs text-muted-foreground">{reason}</span>}
        {!reason && !running && !result && !outcome && (
          <span className="text-xs text-muted-foreground">
            {t("preferences.model.testHint", { provider: state.name })}
          </span>
        )}
        {outcome && <OutcomeText outcome={outcome} />}
      </div>
      {result && (
        <p role="status" className="text-xs">
          <span className="text-add">“{result.reply}”</span>{" "}
          <span className="text-muted-foreground">
            <TestAnswer result={result} />
          </span>
        </p>
      )}
    </div>
  );
}

/** Who answered the test, and how fast; the tokens when the provider counts them. */
function TestAnswer({ result }: { result: ModelTest }) {
  const values = { model: result.model, provider: result.providerName, duration: formatDuration(result.elapsedMs) };
  if (result.inputTokens !== null && result.outputTokens !== null) {
    return (
      <Trans
        i18nKey="preferences.model.answeredWithTokens"
        values={{ ...values, input: result.inputTokens, output: result.outputTokens }}
        components={{ code: <code /> }}
      />
    );
  }
  return <Trans i18nKey="preferences.model.answered" values={values} components={{ code: <code /> }} />;
}

/** Called while rendering, so in the language shown. */
function notReadyReason(state: ModelProviderState): string {
  if (state.cli && !state.cli.path) return i18n.t("preferences.model.notReady.installCli");
  if (state.needsKey && !state.keySaved) return i18n.t("preferences.model.notReady.saveKey");
  return i18n.t("preferences.model.notReady.chooseModel");
}

function OutcomeText({ outcome }: { outcome: Outcome }) {
  return (
    <span
      role={outcome.tone === "error" ? "alert" : "status"}
      className={cn(
        "text-xs",
        outcome.tone === "error" && "text-destructive",
        outcome.tone === "ok" && "text-add",
        outcome.tone === "neutral" && "text-muted-foreground",
      )}
    >
      {outcome.text}
    </span>
  );
}
