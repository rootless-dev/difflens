import { useState } from "react";
import { useTranslation } from "react-i18next";

import { ErrorNote } from "@/components/page";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
  AlertDialogTrigger,
} from "@/components/ui/alert-dialog";
import { Button } from "@/components/ui/button";
import { errorMessage } from "@/lib/format";

interface RemoveProjectProps {
  projectName: string;
  /** Resolves once the project is removed; the caller navigates away afterwards. */
  onRemove: () => Promise<void>;
}

export function RemoveProject({ projectName, onRemove }: RemoveProjectProps) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function confirm() {
    setError(null);
    setPending(true);
    try {
      await onRemove();
    } catch (cause) {
      setError(errorMessage(cause));
      setPending(false);
    }
  }

  function changeOpen(next: boolean) {
    if (pending) {
      return;
    }
    setOpen(next);
    setError(null);
  }

  return (
    <AlertDialog open={open} onOpenChange={changeOpen}>
      <AlertDialogTrigger render={<Button variant="link" size="inline" />}>
        {t("project.remove.trigger")}
      </AlertDialogTrigger>
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle>{t("project.remove.title", { name: projectName })}</AlertDialogTitle>
          <AlertDialogDescription>{t("project.remove.description")}</AlertDialogDescription>
        </AlertDialogHeader>
        {error && <ErrorNote>{error}</ErrorNote>}
        <AlertDialogFooter>
          <AlertDialogCancel disabled={pending}>{t("common.actions.cancel")}</AlertDialogCancel>
          <AlertDialogAction variant="destructive" onClick={confirm} disabled={pending}>
            {pending ? t("project.remove.removing") : t("project.remove.confirm")}
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
