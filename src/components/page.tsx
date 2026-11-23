import type { ComponentProps, ReactNode } from "react";

import { cn } from "@/lib/utils";

export function Page({ className, ...props }: ComponentProps<"main">) {
  return (
    <main
      className={cn("flex-1 overflow-auto px-[max(24px,calc(50%-440px))] py-10", className)}
      {...props}
    />
  );
}

export function PageTitle({ children }: { children: ReactNode }) {
  return <h1 className="text-[22px] leading-[1.45] font-semibold tracking-[-0.02em]">{children}</h1>;
}

export function SectionTitle({ children, note }: { children: ReactNode; note?: ReactNode }) {
  return (
    <h2 className="mt-[30px] mb-2.5 flex items-baseline gap-2 text-sm font-medium text-muted-foreground">
      {children}
      {note && <small className="text-xs font-normal text-faint">{note}</small>}
    </h2>
  );
}

/** Bordered list container shared by the project list and the branch lists. */
export function ListPanel({ className, ...props }: ComponentProps<"ul">) {
  return (
    <ul
      className={cn(
        "overflow-hidden rounded-[10px] border bg-card [&>li+li]:border-t",
        className,
      )}
      {...props}
    />
  );
}

export function PanelNote({ children }: { children: ReactNode }) {
  return (
    <p className="rounded-[10px] border bg-card px-4 py-[13px] text-muted-foreground">{children}</p>
  );
}

export function ErrorNote({ children, className }: { children: ReactNode; className?: string }) {
  return (
    <p role="alert" className={cn("text-destructive", className)}>
      {children}
    </p>
  );
}

export function Hint({ children, className }: { children: ReactNode; className?: string }) {
  return <p className={cn("mt-2 text-xs text-faint", className)}>{children}</p>;
}
