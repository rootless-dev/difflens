import { type FormEvent, type KeyboardEvent, useEffect, useId, useRef, useState } from "react";
import { Trans, useTranslation } from "react-i18next";

import { ErrorNote } from "@/components/page";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Skeleton } from "@/components/ui/skeleton";
import { PreferencesSection } from "@/features/preferences/PreferencesSection";
import { useAsyncData } from "@/hooks/use-async-data";
import { i18n } from "@/i18n";
import {
  type ClaudeCommandMode,
  type ClaudeCommandSetting,
  getClaudeCommand,
  MAX_CLAUDE_COMMAND_CHARS,
  saveClaudeCommand,
  setClaudeCommandMode,
} from "@/lib/api";
import { errorMessage } from "@/lib/format";

const code = { code: <code className="text-foreground" /> };

/**
 * Whether `draft` can be saved over the custom command saved: not empty once trimmed, not the same command, one line
 * (the shell would run a second line as a command of its own) and within what the core keeps. The core checks it
 * again; this only keeps Save from offering what it would refuse.
 */
export function canSaveClaudeCommand(draft: string, saved: string | null): boolean {
  const command = draft.trim();
  return (
    command !== "" && command !== saved && !/[\r\n]/.test(command) && command.length <= MAX_CLAUDE_COMMAND_CHARS
  );
}

/**
 * Preferences › Terminal: what New Claude Code and Open in Claude Code run, `claude` or a command of the user's own.
 * The core reads it as each session starts, so sessions already open keep theirs.
 */
export function TerminalSection() {
  const { t } = useTranslation();
  const { state, reload, update } = useAsyncData(getClaudeCommand);
  return (
    <PreferencesSection title={t("preferences.terminal.title")}>
      {state.status === "loading" && (
        <div role="status" aria-label={t("preferences.terminal.loading")} className="grid gap-2 py-3">
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
      {state.status === "ready" && <ClaudeCommandForm setting={state.data} onChange={(next) => update(() => next)} />}
    </PreferencesSection>
  );
}

interface ClaudeCommandFormProps {
  setting: ClaudeCommandSetting;
  onChange: (next: ClaudeCommandSetting) => void;
}

/**
 * Default or Custom command, as a radio group; either choice is kept at once. Under Custom, the command saved is
 * shown read-only with Edit, so it is not changed by a stray keystroke; editing offers Save and, when there is a saved
 * command to go back to, Cancel. Focus follows: Save and Cancel hand it to Edit, Edit to the command.
 */
function ClaudeCommandForm({ setting, onChange }: ClaudeCommandFormProps) {
  const { t } = useTranslation();
  const titleId = useId();
  const descriptionId = useId();
  const radioName = useId();
  const defaultDescriptionId = useId();
  const inputId = useId();
  const notSavedId = useId();
  const [editing, setEditing] = useState(setting.custom === null);
  const [draft, setDraft] = useState(setting.custom ?? "");
  const [pending, setPending] = useState<"choose" | "save" | null>(null);
  const [error, setError] = useState<string | null>(null);
  const input = useRef<HTMLInputElement>(null);
  const editButton = useRef<HTMLButtonElement>(null);
  /** Where focus goes once the render that follows an action is on screen. */
  const focusNext = useRef<"input" | "edit" | null>(null);

  useEffect(() => {
    const target = focusNext.current;
    if (target === null) return;
    focusNext.current = null;
    if (target === "edit") {
      editButton.current?.focus();
    } else if (input.current) {
      input.current.focus();
      const end = input.current.value.length;
      input.current.setSelectionRange(end, end);
    }
  });

  const canSave = editing && pending === null && canSaveClaudeCommand(draft, setting.custom);

  async function choose(mode: ClaudeCommandMode) {
    if (mode === setting.mode || pending !== null) return;
    setPending("choose");
    setError(null);
    try {
      const next = await setClaudeCommandMode(mode);
      onChange(next);
      // Coming back to Custom shows the command saved, read-only; with none, an empty command to type.
      setEditing(next.custom === null);
      setDraft(next.custom ?? "");
    } catch (cause: unknown) {
      setError(i18n.t("preferences.terminal.chooseFailed", { message: errorMessage(cause) }));
    } finally {
      setPending(null);
    }
  }

  async function save(event: FormEvent) {
    event.preventDefault();
    if (!canSave) return;
    setPending("save");
    setError(null);
    try {
      const next = await saveClaudeCommand(draft.trim());
      onChange(next);
      setDraft(next.custom ?? "");
      setEditing(false);
      focusNext.current = "edit";
    } catch (cause: unknown) {
      setError(i18n.t("preferences.terminal.saveFailed", { message: errorMessage(cause) }));
      focusNext.current = "input";
    } finally {
      setPending(null);
    }
  }

  function edit() {
    setError(null);
    setEditing(true);
    focusNext.current = "input";
  }

  function cancel() {
    setError(null);
    setDraft(setting.custom ?? "");
    setEditing(false);
    focusNext.current = "edit";
  }

  function onKeyDown(event: KeyboardEvent<HTMLInputElement>) {
    if (event.key === "Escape" && editing && setting.custom !== null && pending === null) {
      event.preventDefault();
      cancel();
    }
  }

  return (
    <div className="border-b px-2 py-3">
      <strong id={titleId} className="block font-medium">
        {t("preferences.terminal.claudeCommand")}
      </strong>
      <span id={descriptionId} className="text-muted-foreground">
        {t("preferences.terminal.claudeCommandDescription")}
      </span>
      <div role="radiogroup" aria-labelledby={titleId} aria-describedby={descriptionId} className="mt-2.5 grid gap-1.5">
        <div className="flex items-center gap-2">
          <label className="flex w-fit cursor-pointer items-center gap-2">
            <input
              type="radio"
              name={radioName}
              value="default"
              checked={setting.mode === "default"}
              onChange={() => void choose("default")}
              aria-describedby={defaultDescriptionId}
              className="accent-primary"
            />
            {t("preferences.terminal.default")}
          </label>
          <span id={defaultDescriptionId} className="text-muted-foreground">
            <Trans i18nKey="preferences.terminal.defaultDescription" components={code} />
          </span>
        </div>
        <label className="flex w-fit cursor-pointer items-center gap-2">
          <input
            type="radio"
            name={radioName}
            value="custom"
            checked={setting.mode === "custom"}
            onChange={() => void choose("custom")}
            className="accent-primary"
          />
          {t("preferences.terminal.custom")}
        </label>
      </div>
      {setting.mode === "custom" && (
        <form
          aria-label={t("preferences.terminal.custom")}
          onSubmit={(event) => void save(event)}
          className="mt-2.5 ml-6 grid max-w-[640px] grid-cols-[auto_1fr_auto] items-center gap-x-2.5 gap-y-1.5"
        >
          <Label htmlFor={inputId}>{t("preferences.terminal.command")}</Label>
          <Input
            ref={input}
            id={inputId}
            value={draft}
            readOnly={!editing}
            onChange={(event) => setDraft(event.target.value)}
            onKeyDown={onKeyDown}
            placeholder={editing ? t("preferences.terminal.placeholder") : undefined}
            maxLength={MAX_CLAUDE_COMMAND_CHARS}
            aria-describedby={setting.custom === null ? notSavedId : undefined}
            autoComplete="off"
            autoCapitalize="off"
            autoCorrect="off"
            spellCheck={false}
            className="read-only:bg-secondary read-only:text-muted-foreground"
          />
          <div className="flex gap-2">
            {editing ? (
              <>
                <Button type="submit" size="sm" disabled={!canSave}>
                  {pending === "save" ? t("preferences.saving") : t("common.actions.save")}
                </Button>
                {setting.custom !== null && (
                  <Button variant="secondary" size="sm" disabled={pending === "save"} onClick={cancel}>
                    {t("common.actions.cancel")}
                  </Button>
                )}
              </>
            ) : (
              <Button ref={editButton} variant="secondary" size="sm" onClick={edit}>
                {t("preferences.terminal.edit")}
              </Button>
            )}
          </div>
          {setting.custom === null && (
            <p id={notSavedId} className="col-start-2 col-end-4 text-xs text-faint">
              <Trans i18nKey="preferences.terminal.notSaved" components={code} />
            </p>
          )}
        </form>
      )}
      {error && <ErrorNote className="pt-2">{error}</ErrorNote>}
    </div>
  );
}
