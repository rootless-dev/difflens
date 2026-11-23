import { i18n } from "@/i18n";
import type { FileSide, FileVersions } from "@/lib/api";
import { formatBytes } from "@/lib/format";

/** What the diff view can show for one file. */
export type DiffContent =
  | { kind: "text"; oldText: string; newText: string }
  /** Both sides are the same text: a pure rename, a mode change, an empty file. */
  | { kind: "same"; message: string }
  /** At least one side cannot be shown. */
  | { kind: "unavailable"; message: string };

type Unshowable = "binary" | "too_large" | "not_utf8";

/** What a message is about: the whole file, or only one of its versions. */
type Subject = "file" | "old" | "new";

const isUnshowable = (side: FileSide): side is FileSide & { reason: Unshowable } =>
  side.reason === "binary" || side.reason === "too_large" || side.reason === "not_utf8";

function describe(reason: Unshowable, subject: Subject, size: number): string {
  switch (reason) {
    case "binary":
      return i18n.t(`review.content.binary.${subject}`);
    case "too_large":
      return i18n.t(`review.content.tooLarge.${subject}`, { size: formatBytes(size) });
    case "not_utf8":
      return i18n.t(`review.content.notUtf8.${subject}`);
  }
}

/**
 * Decides how to show a file from its two sides. An absent side is empty text, so an added file
 * reads as all added lines and a deleted one as all removed lines. Messages are in the language shown when called.
 */
export function diffContent({ old, new: current }: FileVersions): DiffContent {
  // One subject for the whole file when both sides agree, or when the other side does not exist.
  if (isUnshowable(old) && isUnshowable(current)) {
    const message =
      old.reason === current.reason
        ? describe(current.reason, "file", Math.max(old.size, current.size))
        : `${describe(old.reason, "old", old.size)} ${describe(current.reason, "new", current.size)}`;
    return { kind: "unavailable", message };
  }
  if (isUnshowable(old)) {
    const subject = current.reason === "absent" ? "file" : "old";
    return { kind: "unavailable", message: describe(old.reason, subject, old.size) };
  }
  if (isUnshowable(current)) {
    const subject = old.reason === "absent" ? "file" : "new";
    return { kind: "unavailable", message: describe(current.reason, subject, current.size) };
  }

  const oldText = old.text ?? "";
  const newText = current.text ?? "";
  if (oldText === newText) {
    return {
      kind: "same",
      message: newText === "" ? i18n.t("review.content.empty") : i18n.t("review.content.unchanged"),
    };
  }
  return { kind: "text", oldText, newText };
}
