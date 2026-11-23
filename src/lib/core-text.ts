// Words for what the core reports as codes with parameters (`CoreText`, `ApiError` in `src/lib/api.ts`): a rejected
// command (`errorMessage` in `src/lib/format.ts`), what keeps the model from being asked, why publishing or a verdict
// is not possible now, why a comment was not published, and the `CoreMessage` fields (why the flow analysis or a test
// run failed, the tests' and checks' diagnostics, shown with `coreText`). Call these instead of reading the fields, so
// the text follows the language shown.
import { i18n } from "@/i18n";
import type {
  CoreParam,
  CoreText,
  ModelInUse,
  PublishFailure,
  PublishReadiness,
  ReviewComment,
} from "@/lib/api";

/**
 * A sentence of the core in the language shown: `errors.reasons.<reason>` filled with its params (a param that is a
 * sentence of its own is worded the same way) and its detail, which stays as the host or tool wrote it. Without a
 * reason (text from elsewhere, a row stored before reasons existed), or with one the catalog does not know, it is
 * the core's English `message`, as it is.
 */
export function coreText(text: string | (CoreText & { detail?: string })): string {
  if (typeof text === "string") {
    return text;
  }
  if (!text.reason) {
    return text.message;
  }
  const key = `errors.reasons.${text.reason}`;
  if (!i18n.exists(key)) {
    return text.message;
  }
  const values: Record<string, string | number> = {};
  for (const [name, value] of Object.entries(text.params ?? {})) {
    values[name] = paramText(value);
  }
  if (values.detail === undefined && text.detail !== undefined) {
    values.detail = text.detail;
  }
  // The key is built from the core's reason, so it is checked with `exists` above rather than by the types. Values
  // go in `replace` so a param named like an i18next option (`key`, `lng`…) is still only a value; a `count` also
  // picks the plural form, as the core's catalog check does.
  const count = typeof values.count === "number" ? { count: values.count } : {};
  return i18n.t(key as "errors.reasons.stopped", { replace: values, ...count });
}

function paramText(value: CoreParam): string | number {
  return typeof value === "object" && value !== null ? coreText(value) : value;
}

/** What to do in Preferences › Model when the model in use is not ready; null when it is. */
export function modelProblem(model: ModelInUse): string | null {
  return model.problem ? coreText(model.problem) : null;
}

/** Why publishing (or a verdict) is not possible now; null when it is allowed or no reason is known. */
export function readinessReason(readiness: PublishReadiness): string | null {
  return readiness.reason ? coreText(readiness.reason) : null;
}

/** Why the last attempt to publish a comment failed; null when it did not. */
export function commentPublishError(comment: ReviewComment): string | null {
  return comment.publishError ? coreText(comment.publishError) : null;
}

/** Why the host refused one change of a publish. */
export function publishFailureMessage(failure: PublishFailure): string {
  return coreText(failure);
}
