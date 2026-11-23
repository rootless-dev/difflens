import { openUrl } from "@tauri-apps/plugin-opener";
import { type ReactNode, useEffect, useState } from "react";

import { getJiraConnection } from "@/lib/api";
import { errorMessage } from "@/lib/format";
import { cn } from "@/lib/utils";

/** The page of card `key` on a Jira site such as "acme.atlassian.net". */
export function cardUrl(site: string, key: string): string {
  return `https://${site}/browse/${encodeURIComponent(key)}`;
}

/**
 * The site of the Jira connection, read once: null until read, and when Jira is not connected or the connection
 * cannot be read (the screen works the same without it).
 */
export function useJiraSite(): string | null {
  const [site, setSite] = useState<string | null>(null);
  useEffect(() => {
    let cancelled = false;
    getJiraConnection().then(
      (connection) => {
        if (!cancelled) setSite(connection.account ? connection.site : null);
      },
      () => {
        // Without it, card keys stay plain text.
      },
    );
    return () => {
      cancelled = true;
    };
  }, []);
  return site;
}

/** The ↗ of a link that opens outside DiffLens. */
export function ExternalMark() {
  return (
    <svg width="10" height="10" viewBox="0 0 10 10" aria-hidden="true">
      <path d="M3.5 1.5h5v5M8.5 1.5l-7 7" fill="none" stroke="currentColor" strokeWidth="1.3" />
    </svg>
  );
}

interface ExternalLinkProps {
  href: string;
  /** Read on hover, and said by screen readers. */
  title: string;
  children: ReactNode;
  className?: string;
}

/**
 * A chip that opens `href` in the system browser (the prototype's `.ext`), as the pull request chip does. When the
 * browser cannot be opened, the title says why.
 */
export function ExternalLink({ href, title, children, className }: ExternalLinkProps) {
  const [openError, setOpenError] = useState<string | null>(null);
  return (
    <a
      href={href}
      onClick={(event) => {
        event.preventDefault();
        event.stopPropagation();
        setOpenError(null);
        openUrl(href).catch((cause: unknown) => setOpenError(errorMessage(cause)));
      }}
      title={openError ?? title}
      className={cn(
        "inline-flex flex-none items-center gap-1.5 rounded-sm border px-[9px] py-[3px] text-xs whitespace-nowrap text-foreground no-underline outline-none hover:border-primary hover:text-primary focus-visible:outline-2 focus-visible:outline-ring",
        className,
      )}
    >
      {children}
      <ExternalMark />
    </a>
  );
}
