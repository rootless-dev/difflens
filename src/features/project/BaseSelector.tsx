import { ChevronDownIcon, PinIcon } from "lucide-react";
import { useState } from "react";
import { Trans, useTranslation } from "react-i18next";

import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuGroup,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import type { BranchBase } from "@/lib/api";
import { cn } from "@/lib/utils";

interface BaseSelectorProps {
  branch: string;
  base: BranchBase;
  /** Integration branches that exist in the repository. */
  bases: string[];
  pending: boolean;
  /** A base name pins it; null goes back to the base found from history. */
  onChoose: (base: string | null) => void;
  className?: string;
}

/**
 * The "into origin/develop ▾" pill of a branch row: it names the ref the base is read from, the one the review
 * compares with; the menu offers the integration branches by name.
 */
export function BaseSelector({ branch, base, bases, pending, onChoose, className }: BaseSelectorProps) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const chosenByUser = base.source === "user";
  const names = { branch, base: base.refName };

  function choose(next: string | null) {
    setOpen(false);
    // Picking the base already in use changes nothing; null is only offered for a chosen base.
    if (next !== base.name) {
      onChoose(next);
    }
  }

  return (
    <DropdownMenu open={open} onOpenChange={setOpen}>
      <DropdownMenuTrigger
        disabled={pending}
        aria-busy={pending || undefined}
        aria-label={chosenByUser ? t("project.base.labelChosen", names) : t("project.base.label", names)}
        title={chosenByUser ? t("project.base.titleChosen", names) : t("project.base.titleHistory", names)}
        className={cn(
          "inline-flex max-w-full cursor-pointer items-center gap-[5px] justify-self-start rounded-[10px] border px-2 py-px text-xs whitespace-nowrap text-muted-foreground outline-none hover:border-faint focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring disabled:cursor-default disabled:opacity-60 data-popup-open:border-faint",
          chosenByUser && "border-primary/60",
          className,
        )}
      >
        <Trans
          i18nKey="project.base.into"
          values={{ base: base.refName }}
          components={{ code: <code className="truncate text-[11.5px] text-foreground" /> }}
        />
        {chosenByUser && <PinIcon aria-hidden="true" className="size-3 shrink-0 text-primary" />}
        <ChevronDownIcon aria-hidden="true" className="size-3 shrink-0 text-faint" />
      </DropdownMenuTrigger>
      <DropdownMenuContent className="min-w-48">
        <DropdownMenuGroup>
          <DropdownMenuLabel>{t("project.base.compareWith")}</DropdownMenuLabel>
          <DropdownMenuRadioGroup value={base.name} onValueChange={(value: string) => choose(value)}>
            {bases.map((name) => (
              <DropdownMenuRadioItem key={name} value={name} className="font-mono text-xs">
                {name}
              </DropdownMenuRadioItem>
            ))}
          </DropdownMenuRadioGroup>
        </DropdownMenuGroup>
        {chosenByUser && (
          <>
            <DropdownMenuSeparator />
            <DropdownMenuItem onClick={() => choose(null)}>{t("project.base.useHistory")}</DropdownMenuItem>
          </>
        )}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
