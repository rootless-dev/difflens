import { type ReactNode, useId } from "react";
import { useTranslation } from "react-i18next";

import type { Focus } from "@/features/review/flow/flow-groups";
import { NODE_STATE } from "@/features/review/flow/flow-model";
import { entryRows, packageRows } from "@/features/review/flow/flow-panel";
import type { FlowGraph } from "@/lib/api";
import { cn } from "@/lib/utils";

interface FlowSidebarProps {
  graph: FlowGraph;
  focus: Focus | null;
  onEntry: (nodeId: string) => void;
  onPackage: (container: string) => void;
}

/** Left panel of the Flow tab: entry points that reach a change and packages with changed functions. */
export function FlowSidebar({ graph, focus, onEntry, onPackage }: FlowSidebarProps) {
  const { t } = useTranslation();
  const entries = entryRows(graph);
  const packages = packageRows(graph);
  const noEntry = graph.options.includeCallers ? t("flow.sidebar.noEntryWithCallers") : t("flow.sidebar.noEntry");
  return (
    <aside data-flow-sidebar className="min-h-0 overflow-auto border-r bg-card px-3 pt-3.5 pb-4">
      <Section title={t("flow.sidebar.entryPoints")} empty={entries.length === 0 ? noEntry : null}>
        {entries.map((row) => (
          <Row
            key={row.key}
            active={focus?.kind === "node" && focus.id === row.nodeId}
            onClick={() => onEntry(row.nodeId)}
            dot={NODE_STATE[row.state].dot}
            name={row.label}
            meta={row.meta}
          />
        ))}
      </Section>
      <Section title={t("flow.sidebar.packages")} empty={packages.length === 0 ? t("flow.sidebar.noPackage") : null}>
        {packages.map((row) => (
          <Row
            key={row.container}
            active={focus?.kind === "package" && focus.container === row.container}
            onClick={() => onPackage(row.container)}
            name={row.container}
            meta={t("flow.functions", { count: row.changed })}
          />
        ))}
      </Section>
    </aside>
  );
}

function Section({ title, empty, children }: { title: string; empty: string | null; children: ReactNode }) {
  const id = useId();
  return (
    <section aria-labelledby={id} className="mb-4">
      <h2 id={id} className="mx-2 mb-1.5 text-xs font-normal text-faint">
        {title}
      </h2>
      {empty ? <p className="mx-2 text-xs text-muted-foreground">{empty}</p> : <ul>{children}</ul>}
    </section>
  );
}

interface RowProps {
  active: boolean;
  onClick: () => void;
  dot?: string;
  name: string;
  meta: string;
}

function Row({ active, onClick, dot, name, meta }: RowProps) {
  return (
    <li>
      <button
        type="button"
        aria-pressed={active}
        aria-label={meta ? `${name}, ${meta}` : name}
        title={name}
        onClick={onClick}
        className={cn(
          "flex w-full cursor-pointer items-center gap-2 rounded-sm px-2 py-1.5 text-left outline-none hover:bg-secondary focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-ring",
          active && "bg-primary-soft hover:bg-primary-soft",
        )}
      >
        {dot && <span aria-hidden="true" className={cn("size-2 flex-none rounded-[2px]", dot)} />}
        <span className="min-w-0 flex-1 truncate font-mono text-xs">{name}</span>
        {meta && <span className="flex-none text-xs text-faint">{meta}</span>}
      </button>
    </li>
  );
}
