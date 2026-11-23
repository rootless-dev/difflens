import { i18n } from "@/i18n";
import type { FileStatus } from "@/lib/api";

interface StatusLook {
  /** One letter shown next to the dot, as git prints it. */
  letter: string;
  /** Background class of the status dot. */
  dot: string;
  /** Text colour class of the letter. */
  text: string;
}

export const FILE_STATUS: Record<FileStatus, StatusLook> = {
  added: { letter: "A", dot: "bg-add", text: "text-add" },
  modified: { letter: "M", dot: "bg-mod", text: "text-mod" },
  deleted: { letter: "D", dot: "bg-del", text: "text-del" },
  renamed: { letter: "R", dot: "bg-primary", text: "text-primary" },
  type_changed: { letter: "T", dot: "bg-mod", text: "text-mod" },
};

/** "Added", "Renamed": the status as a label, in the language shown. */
export const fileStatusLabel = (status: FileStatus): string => i18n.t(`review.fileStatus.${status}`);

/** "added", "renamed": the status as read inside a sentence, in the language shown. */
export const spokenFileStatus = (status: FileStatus): string => i18n.t(`review.fileList.spoken.status.${status}`);
