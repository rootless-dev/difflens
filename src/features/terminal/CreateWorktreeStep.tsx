import { Check, Circle, LoaderCircle } from "lucide-react";
import type { ReactNode } from "react";
import { Trans, useTranslation } from "react-i18next";

import { ErrorNote } from "@/components/page";
import { Button } from "@/components/ui/button";
import type { CreateStep } from "@/features/terminal/terminals";
import type { WorktreePlan } from "@/lib/api";
import { cn } from "@/lib/utils";

interface CreateWorktreeStepProps {
  branch: string;
  plan: WorktreePlan;
  step: CreateStep;
  onCreate: () => void;
  onCancel: () => void;
}

const code = { code: <code className="text-foreground" /> };

/**
 * What the expanded panel shows for a branch checked out in no folder: what creating its worktree does, and nothing
 * created until "Create worktree and open" is clicked; then the steps as they go, and the session opens there.
 */
export function CreateWorktreeStep({ branch, plan, step, onCreate, onCancel }: CreateWorktreeStepProps) {
  const { t } = useTranslation();
  if (step.phase === "creating") {
    return <Creating branch={branch} plan={plan} step={step} />;
  }
  const values = { branch, path: plan.relativePath, upstream: plan.upstream };
  return (
    <Screen>
      <h3 className="flex items-center gap-2.5 text-sm font-semibold">
        <span>
          <Trans i18nKey="terminal.create.title" values={values} components={code} />
        </span>
      </h3>
      <p className="mt-2 max-w-[74ch]">
        {plan.upstream && `${t("terminal.create.remoteOnly")} `}
        {step.kind === "claude" ? t("terminal.create.introClaude") : t("terminal.create.introShell")}
      </p>
      <p className="mt-2 max-w-[74ch] rounded-lg border bg-card px-3 py-2.5 text-muted-foreground">
        <Trans
          i18nKey={plan.upstream ? "terminal.create.whatRemote" : "terminal.create.whatLocal"}
          values={values}
          components={code}
        />
      </p>
      {step.phase === "failed" && (
        <ErrorNote className="mt-3">{t("terminal.create.failed", { message: step.message })}</ErrorNote>
      )}
      <div className="mt-4 flex items-center gap-2">
        <Button autoFocus onClick={onCreate}>
          {step.phase === "failed" ? t("common.actions.tryAgain") : t("terminal.create.confirm")}
        </Button>
        <Button variant="secondary" onClick={onCancel}>
          {t("common.actions.cancel")}
        </Button>
      </div>
      <p className="mt-3 text-xs text-faint">{t("terminal.create.hint")}</p>
    </Screen>
  );
}

function Creating({
  branch,
  plan,
  step,
}: {
  branch: string;
  plan: WorktreePlan;
  step: Extract<CreateStep, { phase: "creating" }>;
}) {
  const { t } = useTranslation();
  const values = { branch, path: plan.relativePath, upstream: plan.upstream };
  // Creating the tracking branch and checking it out are one git command: they go together.
  const adding = step.excluded ? "now" : "todo";
  return (
    <Screen status>
      <h3 className="flex items-center gap-2.5 text-sm font-semibold">
        <LoaderCircle aria-hidden="true" className="size-3.5 animate-spin text-primary" />
        <span>
          <Trans i18nKey="terminal.create.creating" values={values} components={code} />
        </span>
      </h3>
      <ol className="mt-3.5 grid gap-2">
        <StepLine state={step.excluded ? "done" : "now"}>
          <Trans i18nKey="terminal.create.stepExclude" components={code} />
        </StepLine>
        {plan.upstream && (
          <StepLine state={adding}>
            <Trans i18nKey="terminal.create.stepTracking" values={values} components={code} />
          </StepLine>
        )}
        <StepLine state={adding}>
          <Trans i18nKey="terminal.create.stepAdd" values={values} components={code} />
        </StepLine>
        <StepLine state="todo">
          {step.kind === "claude" ? t("terminal.create.stepOpenClaude") : t("terminal.create.stepOpenShell")}
        </StepLine>
      </ol>
      <div
        role="progressbar"
        aria-label={t("terminal.create.progress")}
        className="mt-4 h-[5px] max-w-[520px] overflow-hidden rounded-[3px] bg-border"
      >
        <i className="block h-full w-1/3 animate-pulse bg-primary" />
      </div>
    </Screen>
  );
}

function StepLine({ state, children }: { state: "done" | "now" | "todo"; children: ReactNode }) {
  const { t } = useTranslation();
  return (
    <li
      className={cn(
        "grid grid-cols-[16px_1fr] items-center gap-2.5",
        state === "now" && "text-foreground",
        state === "done" && "text-muted-foreground",
        state === "todo" && "text-faint",
      )}
    >
      {state === "done" && <Check aria-label={t("terminal.create.done")} className="size-3.5 text-add" />}
      {state === "now" && <LoaderCircle aria-hidden="true" className="size-3.5 animate-spin text-primary" />}
      {state === "todo" && <Circle aria-hidden="true" className="size-3" />}
      <span>{children}</span>
    </li>
  );
}

function Screen({ status = false, children }: { status?: boolean; children: ReactNode }) {
  return (
    <div
      role={status ? "status" : undefined}
      aria-live={status ? "polite" : undefined}
      className="min-h-0 flex-1 overflow-auto bg-background px-6 py-5"
    >
      <div className="max-w-[660px]">{children}</div>
    </div>
  );
}
