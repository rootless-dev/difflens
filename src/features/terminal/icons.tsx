import type { TerminalKind } from "@/lib/api";
import { cn } from "@/lib/utils";

// The icons of design/terminal-panel.html: a shell's prompt in a window, and for Claude Code a plain four-point spark
// in the colour of what a model wrote, a generic mark rather than anyone's logo.

function ShellIcon() {
  return (
    <svg width="14" height="14" viewBox="0 0 14 14" aria-hidden="true">
      <rect x="1" y="2" width="12" height="10" rx="2" fill="none" stroke="currentColor" strokeWidth="1.2" />
      <path
        d="M3.8 5.2 5.8 7l-2 1.8M7.2 9h3"
        fill="none"
        stroke="currentColor"
        strokeWidth="1.2"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
    </svg>
  );
}

function SparkIcon() {
  return (
    <svg width="14" height="14" viewBox="0 0 14 14" aria-hidden="true">
      <path d="M7 1.2 8.1 5.9 12.8 7 8.1 8.1 7 12.8 5.9 8.1 1.2 7 5.9 5.9Z" fill="currentColor" />
    </svg>
  );
}

export function SessionIcon({ kind, className }: { kind: TerminalKind; className?: string }) {
  return (
    <span className={cn("inline-flex flex-none", kind === "claude" ? "text-ai" : "text-muted-foreground", className)}>
      {kind === "claude" ? <SparkIcon /> : <ShellIcon />}
    </span>
  );
}
