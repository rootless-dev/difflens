import { type ComponentProps, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";

import { Button } from "@/components/ui/button";

interface CopyPromptButtonProps {
  /** The prompt, worked out on click, in the language shown then. */
  text: () => string;
  label: string;
  /** Shown for a moment when the clipboard refused it. */
  failedLabel: string;
  title?: string;
  variant?: ComponentProps<typeof Button>["variant"];
  size?: ComponentProps<typeof Button>["size"];
  className?: string;
}

/** Copies a prompt for a coding agent, then says for two seconds that it did, or that it could not. */
export function CopyPromptButton({ text, label, failedLabel, title, variant, size, className }: CopyPromptButtonProps) {
  const { t } = useTranslation();
  const [copied, setCopied] = useState<"copied" | "failed" | null>(null);
  useEffect(() => {
    if (copied === null) return;
    const timer = setTimeout(() => setCopied(null), 2000);
    return () => clearTimeout(timer);
  }, [copied]);
  const copy = () => {
    navigator.clipboard.writeText(text()).then(
      () => setCopied("copied"),
      () => setCopied("failed"),
    );
  };
  return (
    <Button variant={variant} size={size} className={className} title={title} onClick={copy} aria-live="polite">
      {copied === "copied" ? t("common.actions.copied") : copied === "failed" ? failedLabel : label}
    </Button>
  );
}
