import { cn } from "@/lib/utils";

export interface CodeLine {
  line: number;
  text: string;
  /** Drawn on the deletion tint: the lines a finding cites. */
  marked?: boolean;
}

/** Lines of a file with their numbers, as read from the review's new side; plain text, never highlighted as code. */
export function CodeLines({ lines, label, className }: { lines: readonly CodeLine[]; label: string; className?: string }) {
  const width = String(lines.at(-1)?.line ?? 0).length;
  return (
    <pre
      aria-label={label}
      className={cn("overflow-auto rounded-[7px] border bg-card py-2 font-mono text-xs leading-[1.7]", className)}
    >
      {lines.map((line) => (
        <div key={line.line} data-cited={line.marked || undefined} className={cn("px-3", line.marked && "bg-del-bg")}>
          {`${String(line.line).padStart(width)}  ${line.text}` || " "}
        </div>
      ))}
    </pre>
  );
}
