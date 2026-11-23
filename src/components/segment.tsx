import type { ReactNode } from "react";

import { cn } from "@/lib/utils";

/** Bordered group of toggle buttons, one of them pressed (the prototype's `.seg`). */
export function SegmentGroup({ label, children, className }: { label: string; children: ReactNode; className?: string }) {
  return (
    <div role="group" aria-label={label} className={cn("inline-flex rounded-[7px] border bg-card p-0.5", className)}>
      {children}
    </div>
  );
}

interface SegmentProps {
  pressed: boolean;
  onClick?: () => void;
  /** Shown but not usable yet; it stays hoverable so its title can say why. */
  disabled?: boolean;
  title?: string;
  children: ReactNode;
  className?: string;
}

export function Segment({ pressed, onClick, disabled = false, title, children, className }: SegmentProps) {
  return (
    <button
      type="button"
      aria-pressed={pressed}
      aria-disabled={disabled || undefined}
      title={title}
      onClick={disabled ? undefined : onClick}
      className={cn(
        "cursor-pointer rounded-[5px] px-[11px] py-[5px] whitespace-nowrap text-muted-foreground outline-none hover:text-foreground focus-visible:outline-2 focus-visible:outline-offset-1 focus-visible:outline-ring",
        pressed && "bg-secondary text-foreground shadow-[0_0_0_1px_var(--border)]",
        disabled && "cursor-default text-faint hover:text-faint",
        className,
      )}
    >
      {children}
    </button>
  );
}
