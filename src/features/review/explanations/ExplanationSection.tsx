import { useContext, useId } from "react";
import { useTranslation } from "react-i18next";

import { ErrorNote } from "@/components/page";
import { Button } from "@/components/ui/button";
import { WithCode } from "@/components/with-code";
import type { ShownBrief, ShownDetail } from "@/features/review/ai-text/versions";
import { explainerName } from "@/features/review/explanations/explanation-text";
import {
  canExplain,
  detailRunKey,
  ExplanationsContext,
  nodeRunKey,
} from "@/features/review/explanations/use-explanations";
import { Spinner } from "@/features/review/flow/FlowView";
import { modelName } from "@/features/review/insights/insights-text";
import { useNow } from "@/hooks/use-now";
import type { ExplanationLevel, FlowNode } from "@/lib/api";
import { formatRelativeTime } from "@/lib/format";

/**
 * The way to ask for an explanation in a function's popover, while it has none. Explaining is a model call, only on
 * click, with the model named in the button's tooltip. The explanation itself opens in a column of its own beside the
 * popover once it has arrived (ExplanationColumn), never empty while it is being written.
 */
export function ExplanationSection({ node }: { node: FlowNode }) {
  const { t } = useTranslation();
  const id = useId();
  const explanations = useContext(ExplanationsContext);
  if (!canExplain(node) || explanations.shown.has(node.id)) return null;
  return (
    <section aria-labelledby={id} className="mt-3.5 border-t pt-3">
      <h4 id={id} className="mb-1.5 text-xs font-normal text-faint">
        {t("flow.explain.title")}
      </h4>
      <ExplainControl node={node} level="brief" />
    </section>
  );
}

interface ExplanationColumnProps {
  node: FlowNode;
  /** The detailed explanation is shown below the short one. Each opening of the popover starts without it. */
  expanded: boolean;
  onExpandedChange: (expanded: boolean) => void;
}

/**
 * A function's explanation, in the popover's second column: the short one (one or two sentences), who wrote it and
 * when, and the ways to explain it again or in detail. The detailed one (what changed, why, said to be inferred when
 * nothing the model was shown says why, and what it is for) is shown below it only when asked for: Show details
 * when it is already kept, which asks no model, or Explain in detail, which writes it and shows it once it arrives.
 * An explanation written before short ones existed is a detailed one alone: a line says so, with Explain there to
 * write the short one.
 */
export function ExplanationColumn({ node, expanded, onExpandedChange }: ExplanationColumnProps) {
  const { t } = useTranslation();
  const id = useId();
  const detailId = useId();
  const now = useNow();
  const explanations = useContext(ExplanationsContext);
  const explanation = explanations.shown.get(node.id);
  if (!explanation) return null;
  const { brief, detail } = explanation;
  // The tag says the text read first is outdated: the short one, or the detailed one when there is no short one.
  const outdated = brief ? brief.outdated : (detail?.outdated ?? false);
  const detailShown = expanded && detail !== null;
  // Explaining in detail again stays while it runs, even with the detail hidden meanwhile, so it can be stopped.
  const detailControl = detail === null || detailShown || explanations.runByKey(detailRunKey(node.id)) !== null;
  return (
    <section aria-labelledby={id}>
      <h4 id={id} className="mb-2 flex items-center gap-2 text-xs font-normal text-faint">
        {t("flow.explain.title")}
        <em className="rounded-[9px] bg-ai-bg px-[7px] text-[11px] text-ai not-italic">{t("overview.tag.model")}</em>
        {outdated && (
          <em className="rounded-[9px] bg-primary-soft px-[7px] text-[11px] text-primary not-italic">
            {t("flow.explain.outdatedTag")}
          </em>
        )}
      </h4>
      {brief ? (
        <div className={brief.outdated ? "text-muted-foreground" : undefined}>
          <p className="text-[13px] leading-[1.55]">
            <WithCode text={brief.text} />
          </p>
          <Byline written={brief} now={now} />
        </div>
      ) : (
        <p className="text-xs text-muted-foreground">{t("flow.explain.onlyDetailed")}</p>
      )}
      <div className="mt-1 flex flex-wrap items-start gap-x-2">
        {detail && (
          <Button
            variant={detailShown ? "outline" : "secondary"}
            size="sm"
            className="mt-2"
            aria-expanded={detailShown}
            aria-controls={detailShown ? detailId : undefined}
            onClick={() => onExpandedChange(!detailShown)}
          >
            {detailShown ? t("flow.explain.hideDetails") : t("flow.explain.showDetails")}
          </Button>
        )}
        {detailControl && <ExplainControl node={node} level="detail" onExplain={() => onExpandedChange(true)} />}
        <ExplainControl node={node} level="brief" />
      </div>
      {detailShown && <Detail id={detailId} detail={detail} now={now} />}
    </section>
  );
}

/** Who wrote a level of the explanation and when, and that its code changed since when it did. */
function Byline({ written, now }: { written: ShownBrief | ShownDetail; now: Date }) {
  const { t } = useTranslation();
  return (
    <>
      <p className="mt-2 text-xs text-faint">
        {t("flow.explain.byline", { model: modelName(written), when: formatRelativeTime(written.createdAt, now) })}
      </p>
      {written.outdated && <p className="mt-1.5 text-xs text-primary">{t("flow.explain.outdated")}</p>}
    </>
  );
}

/** The detailed explanation in its three parts (only what it is for, for a function of the context). */
function Detail({ id, detail, now }: { id: string; detail: ShownDetail; now: Date }) {
  const { t } = useTranslation();
  const labelId = useId();
  return (
    <section
      id={id}
      aria-labelledby={labelId}
      className={["mt-3 border-t pt-3", detail.outdated ? "text-muted-foreground" : ""].join(" ")}
    >
      <h5 id={labelId} className="mb-2 text-xs font-normal text-faint">
        {t("flow.explain.detailLabel")}
      </h5>
      <dl className="grid gap-2.5 text-[13px] leading-[1.55]">
        {detail.kind === "changed" && (
          <>
            <div>
              <dt className="text-xs text-muted-foreground">{t("flow.explain.whatChanged")}</dt>
              <dd>
                <WithCode text={detail.whatChanged} />
              </dd>
            </div>
            <div>
              <dt className="flex items-center gap-1.5 text-xs text-muted-foreground">
                {t("flow.explain.why")}
                {detail.whyInferred && (
                  <span title={t("flow.explain.inferredTitle")} className="rounded-[4px] border px-1 text-[10px] text-faint">
                    {t("flow.explain.inferred")}
                  </span>
                )}
              </dt>
              <dd>
                <WithCode text={detail.why} />
              </dd>
            </div>
          </>
        )}
        <div>
          <dt className="text-xs text-muted-foreground">{t("flow.explain.purpose")}</dt>
          <dd>
            <WithCode text={detail.purpose} />
          </dd>
        </div>
      </dl>
      <Byline written={detail} now={now} />
    </section>
  );
}

/**
 * Explain (again), which writes the short explanation, or Explain in detail (again), which asks for the detailed one
 * of this function alone; its spinner and a way to stop it while the model writes, and why the last try failed.
 * `onExplain` is told of each click that asks the model.
 */
function ExplainControl({ node, level, onExplain }: { node: FlowNode; level: ExplanationLevel; onExplain?: () => void }) {
  const { t } = useTranslation();
  const explanations = useContext(ExplanationsContext);
  const brief = level === "brief";
  const key = brief ? nodeRunKey(node.id) : detailRunKey(node.id);
  const shown = explanations.shown.get(node.id);
  const explained = brief ? Boolean(shown?.brief) : Boolean(shown?.detail);
  const run = brief ? explanations.runOf(node.id) : explanations.runByKey(key);
  const error = explanations.errorOf(key);
  const model = explainerName(explanations.models[level]);
  const values = { name: node.name, model };
  const label = brief
    ? explained
      ? t("flow.explain.again")
      : t("flow.explain.button")
    : explained
      ? t("flow.explain.detailAgain")
      : t("flow.explain.detail");
  const title = brief
    ? explained
      ? t("flow.explain.againTitle", values)
      : t("flow.explain.buttonTitle", values)
    : explained
      ? t("flow.explain.detailAgainTitle", values)
      : t("flow.explain.detailTitle", values);
  const working = run?.stopping
    ? t("flow.explain.stopping")
    : brief
      ? t("flow.explain.explaining", { name: node.name })
      : t("flow.explain.detailing", { name: node.name });
  // Explain in detail leads; once the short one is there, explaining it again is the quieter choice.
  const variant = explained || (brief && shown !== undefined) ? "outline" : "secondary";
  return (
    <div>
      {run ? (
        <div className="mt-2 flex items-center gap-2">
          <Button variant={variant} size="sm" disabled>
            <Spinner />
            {run.stopping ? t("flow.explain.stopping") : label}
          </Button>
          <span role="status" className="sr-only">
            {working}
          </span>
          <Button variant="outline" size="sm" disabled={run.stopping} onClick={() => explanations.cancel(run.key)}>
            {t("common.actions.cancel")}
          </Button>
        </div>
      ) : (
        <Button
          variant={variant}
          size="sm"
          className="mt-2"
          title={title}
          onClick={() => {
            onExplain?.();
            explanations.explain(key, [node.id], level);
          }}
        >
          {label}
        </Button>
      )}
      {error && <ErrorNote className="mt-2">{t("flow.explain.failed", { message: error })}</ErrorNote>}
    </div>
  );
}
