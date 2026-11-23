import { type FormEvent, useId, useState } from "react";
import { useTranslation } from "react-i18next";

import { ErrorNote, Hint } from "@/components/page";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { joinPath, repositoryName } from "@/features/projects/clone-destination";

interface CloneBoxProps {
  pending: boolean;
  error: string | null;
  /** Opens the folder picker; resolves to null when the user dismisses it. */
  onChooseFolder: () => Promise<string | null>;
  onSubmit: (url: string, destination: string) => void;
  onCancel: () => void;
}

export function CloneBox({ pending, error, onChooseFolder, onSubmit, onCancel }: CloneBoxProps) {
  const { t } = useTranslation();
  const urlId = useId();
  const destinationId = useId();
  const [url, setUrl] = useState("");
  const [parent, setParent] = useState<string | null>(null);
  const [typedDestination, setTypedDestination] = useState<string | null>(null);
  const [pickerFailed, setPickerFailed] = useState(false);

  const derivedDestination = parent === null ? "" : joinPath(parent, repositoryName(url));
  const destination = typedDestination ?? derivedDestination;
  const canSubmit = !pending && url.trim() !== "" && destination.trim() !== "";

  async function chooseFolder() {
    setPickerFailed(false);
    try {
      const picked = await onChooseFolder();
      if (picked !== null) {
        setParent(picked);
        setTypedDestination(null);
      }
    } catch {
      setPickerFailed(true);
    }
  }

  function submit(event: FormEvent) {
    event.preventDefault();
    if (canSubmit) {
      onSubmit(url.trim(), destination.trim());
    }
  }

  return (
    <form
      aria-label={t("projects.clone.label")}
      onSubmit={submit}
      className="mb-[22px] rounded-[10px] border bg-card px-[18px] py-4"
    >
      <div className="grid grid-cols-[150px_1fr] items-center gap-x-4 gap-y-2.5">
        <Label htmlFor={urlId}>{t("projects.clone.url")}</Label>
        <Input
          id={urlId}
          value={url}
          onChange={(event) => setUrl(event.target.value)}
          placeholder="git@host:team/repository.git"
          disabled={pending}
          autoFocus
          autoComplete="off"
          spellCheck={false}
        />
        <Label htmlFor={destinationId}>{t("projects.clone.destination")}</Label>
        <div className="flex items-center gap-2">
          <Input
            id={destinationId}
            value={destination}
            onChange={(event) => setTypedDestination(event.target.value)}
            placeholder={t("projects.clone.destinationPlaceholder")}
            disabled={pending}
            autoComplete="off"
            spellCheck={false}
            className="flex-1"
          />
          <Button variant="secondary" size="sm" onClick={chooseFolder} disabled={pending}>
            {t("projects.clone.chooseFolder")}
          </Button>
        </div>
      </div>
      <Hint>{t("projects.clone.hint")}</Hint>
      {pickerFailed && <ErrorNote className="mt-2.5">{t("projects.clone.pickerFailed")}</ErrorNote>}
      {error && <ErrorNote className="mt-2.5">{error}</ErrorNote>}
      <div className="mt-3.5 flex items-center gap-2.5">
        <Button type="submit" disabled={!canSubmit}>
          {pending ? t("projects.clone.cloning") : t("projects.clone.submit")}
        </Button>
        <Button variant="secondary" onClick={onCancel} disabled={pending}>
          {t("common.actions.cancel")}
        </Button>
        {pending && (
          <span role="status" className="text-muted-foreground">
            {t("projects.clone.progress")}
          </span>
        )}
      </div>
    </form>
  );
}
