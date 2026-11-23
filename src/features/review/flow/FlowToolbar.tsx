import { LoaderCircle } from "lucide-react";
import { useTranslation } from "react-i18next";

import { Segment, SegmentGroup } from "@/components/segment";
import { type Level, LEVELS } from "@/features/review/flow/flow-groups";
import {
  type LinkState,
  RELATION_LOOK,
  relationMeaning,
  RELATIONS,
  STATE_LOOK,
  stateMeaning,
} from "@/features/review/flow/flow-links";
import { NODE_STATE, nodeStateLabel } from "@/features/review/flow/flow-model";
import { SEVERITIES, severityName } from "@/features/review/insights/insights-text";
import {
  BugMark,
  CopyMark,
  GaugeMark,
  NoTestMark,
  SeverityMark,
  WrenchMark,
} from "@/features/review/signals/signal-marks";
import { laterStep } from "@/features/review/views";
import type { FlowGraph, FlowNodeState, FlowOptions } from "@/lib/api";
import { cn } from "@/lib/utils";

const CHANGED_STATES: FlowNodeState[] = ["new", "modified", "deleted"];
const LEGEND_LINK_STATES: LinkState[] = ["added", "preserved", "removed"];

/** The options of a graph that is on screen, what each would add to it, and the level it is drawn at. */
export interface GraphControls {
  hidden: FlowGraph["hidden"];
  options: FlowOptions;
  onChange: (options: FlowOptions) => void;
  /** A graph with the new options is being built; the one on screen stays until it arrives. */
  pending: boolean;
  level: Level;
  onLevelChange: (level: Level) => void;
  /** States drawn besides the changed ones, so the legend names them. */
  context: { untouched: boolean; external: boolean };
}

/**
 * Scope switch, level switch, graph options (once there is a graph) and the legends of the flow toolbar; the legend of
 * findings, limits and checks only while some box carries a badge (`signals`).
 */
export function FlowToolbar({ controls, signals = false }: { controls?: GraphControls; signals?: boolean }) {
  const { t } = useTranslation();
  const states = [
    ...CHANGED_STATES,
    ...(controls?.context.untouched ? (["untouched"] as const) : []),
    ...(controls?.context.external ? (["external"] as const) : []),
  ];
  return (
    <div className="flex flex-none flex-wrap items-center gap-x-3.5 gap-y-2 border-b px-4 py-2.5">
      <SegmentGroup label={t("flow.toolbar.scope")}>
        <Segment className="py-[3px]" pressed>
          {t("flow.toolbar.thisBranch")}
        </Segment>
        <Segment className="py-[3px]" pressed={false} disabled title={laterStep()}>
          {t("flow.toolbar.wholeApp")}
        </Segment>
      </SegmentGroup>
      {controls && <LevelSwitch level={controls.level} onChange={controls.onLevelChange} />}
      {controls && <GraphOptions {...controls} />}
      <ul
        aria-label={t("flow.toolbar.functionStates")}
        className="flex flex-wrap gap-x-3.5 gap-y-1 text-xs text-muted-foreground"
      >
        {states.map((state) => (
          <li key={state} className="inline-flex items-center gap-1.5">
            <i
              aria-hidden="true"
              className={cn(
                "size-2 rounded-[2px]",
                state === "external" ? "border border-dashed border-muted-foreground" : NODE_STATE[state].dot,
              )}
            />
            {nodeStateLabel(state)}
          </li>
        ))}
      </ul>
      <ul
        aria-label={t("flow.toolbar.reviewMarks")}
        className="flex flex-wrap gap-x-3.5 gap-y-1 text-xs text-muted-foreground"
      >
        <li className="inline-flex items-center gap-1.5">
          <b aria-hidden="true" className="font-semibold text-add">
            ✓
          </b>
          {t("flow.toolbar.reviewed")}
        </li>
        <li className="inline-flex items-center gap-1.5">
          <i aria-hidden="true" className="size-[11px] rounded-full border-[1.6px] border-dashed border-primary" />
          {t("flow.toolbar.changedSinceReview")}
        </li>
      </ul>
      {signals && (
        <ul
          aria-label={t("signals.legend.label")}
          className="flex flex-wrap gap-x-3.5 gap-y-1 border-l pl-3.5 text-xs text-muted-foreground"
        >
          <li className="inline-flex items-center gap-1.5 text-faint">{t("signals.legend.bySeverity")}</li>
          {SEVERITIES.map((severity) => (
            <li key={severity} className="inline-flex items-center gap-1.5">
              <SeverityMark severity={severity} />
              {severityName(severity)}
            </li>
          ))}
          <li className="inline-flex items-center gap-1.5">
            <GaugeMark />
            {t("signals.legend.limit")}
          </li>
          <li className="inline-flex items-center gap-1.5">
            <WrenchMark />
            {t("signals.legend.check")}
          </li>
          <li className="inline-flex items-center gap-1.5">
            <CopyMark />
            {t("signals.legend.duplicate")}
          </li>
          <li className="inline-flex items-center gap-1.5">
            <BugMark />
            {t("signals.legend.mutant")}
          </li>
          <li className="inline-flex items-center gap-1.5">
            <NoTestMark />
            {t("signals.legend.untested")}
          </li>
          <li className="inline-flex items-center gap-1.5 text-faint">{t("signals.legend.count")}</li>
        </ul>
      )}
      <ul
        aria-label={t("flow.toolbar.links")}
        className="flex basis-full flex-wrap gap-x-3.5 gap-y-1 text-xs text-muted-foreground"
      >
        {RELATIONS.map((relation) => (
          <li key={relation} className="inline-flex items-center gap-1.5">
            <LegendLine dash={RELATION_LOOK[relation].dash} round={relation !== "call"} />
            {relationMeaning(relation)}
          </li>
        ))}
        {LEGEND_LINK_STATES.map((state) => (
          <li key={state} className="inline-flex items-center gap-1.5">
            <LegendLine stroke={STATE_LOOK[state].stroke} width={STATE_LOOK[state].strokeWidth} />
            {state === "removed" && (
              <span aria-hidden="true" className="font-bold text-del">
                ×
              </span>
            )}
            {stateMeaning(state)}
          </li>
        ))}
      </ul>
    </div>
  );
}

function LevelSwitch({ level, onChange }: { level: Level; onChange: (level: Level) => void }) {
  const { t } = useTranslation();
  return (
    <SegmentGroup label={t("flow.toolbar.level")}>
      {LEVELS.map((option) => (
        <Segment
          key={option}
          className="py-[3px]"
          pressed={level === option}
          onClick={() => level !== option && onChange(option)}
        >
          {t(`flow.levels.${option}`)}
        </Segment>
      ))}
    </SegmentGroup>
  );
}

/**
 * Three toggles, each saying how much it would add. One that would add nothing is disabled with a title saying
 * why, unless it is on (after a Reload the review may have changed under it).
 */
function GraphOptions({ hidden, options, onChange, pending }: GraphControls) {
  const { t } = useTranslation();
  const tests = hidden.changedTests;
  const callers = hidden.callers;
  const callees = hidden.callees + hidden.externalPackages;
  return (
    <>
      <SegmentGroup label={t("flow.toolbar.context")}>
        <Segment
          className="py-[3px]"
          pressed={options.includeTests}
          disabled={tests === 0 && !options.includeTests}
          title={tests === 0 ? t("flow.toolbar.noTestChanged") : t("flow.toolbar.testsTitle", { count: tests })}
          onClick={() => onChange({ ...options, includeTests: !options.includeTests })}
        >
          {t("flow.toolbar.includeTests", { total: tests })}
        </Segment>
        <Segment
          className="py-[3px]"
          pressed={options.includeCallers}
          disabled={callers === 0 && !options.includeCallers}
          title={callers === 0 ? t("flow.toolbar.noCallers") : t("flow.toolbar.callersTitle", { count: callers })}
          onClick={() => onChange({ ...options, includeCallers: !options.includeCallers })}
        >
          {t("flow.toolbar.showCallers", { total: callers })}
        </Segment>
        <Segment
          className="py-[3px]"
          pressed={options.includeCallees}
          disabled={callees === 0 && !options.includeCallees}
          title={
            callees === 0
              ? t("flow.toolbar.noCallees")
              : t("flow.toolbar.calleesTitle", {
                  functions: t("flow.functions", { count: hidden.callees }),
                  packages: t("flow.toolbar.externalPackages", { count: hidden.externalPackages }),
                })
          }
          onClick={() => onChange({ ...options, includeCallees: !options.includeCallees })}
        >
          {t("flow.toolbar.showCallees", { total: callees })}
        </Segment>
      </SegmentGroup>
      {pending && (
        <p role="status" className="inline-flex items-center gap-1.5 text-xs text-muted-foreground">
          <LoaderCircle aria-hidden="true" className="size-3.5 animate-spin" />
          {t("flow.toolbar.updating")}
        </p>
      )}
    </>
  );
}

function LegendLine({
  dash,
  round = false,
  stroke = "stroke-muted-foreground",
  width = 1.6,
}: {
  dash?: string;
  round?: boolean;
  stroke?: string;
  width?: number;
}) {
  return (
    <svg aria-hidden="true" width="26" height="8" className="flex-none">
      <path
        d="M0 4h26"
        fill="none"
        strokeWidth={width}
        strokeLinecap={round ? "round" : undefined}
        strokeDasharray={dash}
        className={stroke}
      />
    </svg>
  );
}
