// Which language version of a model's text to show: the one in the language shown when there is one (the original or
// a translation), otherwise the original, with a banner saying what language it is in.
import type {
  Explanations,
  Finding,
  FunctionBrief,
  FunctionExplanation,
  Language,
  OverviewSummary,
  TestDiagnosis,
} from "@/lib/api";

/** A finding with the text of the version shown, and the language that text is in. */
export interface ShownFinding extends Finding {
  /** The language of the text shown; differs from the one asked for when no version is in it. */
  shownLanguage: Language;
}

export function shownFinding(finding: Finding, language: Language): ShownFinding {
  if (finding.language === language) {
    return { ...finding, shownLanguage: language };
  }
  const translation = finding.translations.find((version) => version.language === language);
  if (!translation) {
    return { ...finding, shownLanguage: finding.language };
  }
  return {
    ...finding,
    title: translation.title,
    explanation: translation.explanation,
    suggestedDirection: translation.suggestedDirection,
    evidence: translation.evidence,
    related: translation.related,
    shownLanguage: language,
  };
}

export interface ShownSummary extends OverviewSummary {
  shownLanguage: Language;
}

export function shownSummary(summary: OverviewSummary, language: Language): ShownSummary {
  if (summary.language === language) {
    return { ...summary, shownLanguage: language };
  }
  const translation = summary.translations.find((version) => version.language === language);
  if (!translation) {
    return { ...summary, shownLanguage: summary.language };
  }
  return {
    ...summary,
    title: translation.title,
    lead: translation.lead,
    // One translated item for each item: a version without them (never expected) keeps the original's lists.
    points: translation.points.length === summary.points.length ? translation.points : summary.points,
    attention: translation.attention.length === summary.attention.length ? translation.attention : summary.attention,
    cardSummary: translation.cardSummary,
    // Same criteria, same order: only their text changes; verdicts and places stay.
    criteria: summary.criteria.map((criterion, index) => ({
      ...criterion,
      criterion: translation.criteria[index]?.criterion ?? criterion.criterion,
      reasoning: translation.criteria[index]?.reasoning ?? criterion.reasoning,
    })),
    shownLanguage: language,
  };
}

export interface ShownDetail extends FunctionExplanation {
  shownLanguage: Language;
}

export function shownDetail(explanation: FunctionExplanation, language: Language): ShownDetail {
  if (explanation.language === language) {
    return { ...explanation, shownLanguage: language };
  }
  const translation = explanation.translations.find((version) => version.language === language);
  if (!translation) {
    return { ...explanation, shownLanguage: explanation.language };
  }
  return {
    ...explanation,
    whatChanged: translation.whatChanged,
    why: translation.why,
    purpose: translation.purpose,
    shownLanguage: language,
  };
}

export interface ShownBrief extends FunctionBrief {
  shownLanguage: Language;
}

export function shownBrief(brief: FunctionBrief, language: Language): ShownBrief {
  if (brief.language === language) {
    return { ...brief, shownLanguage: language };
  }
  const translation = brief.translations.find((version) => version.language === language);
  if (!translation) {
    return { ...brief, shownLanguage: brief.language };
  }
  return { ...brief, text: translation.text, shownLanguage: language };
}

export interface ShownDiagnosis extends TestDiagnosis {
  shownLanguage: Language;
}

/** An explanation of a failed test in the version that reads in `language` if any; its code stays as written. */
export function shownDiagnosis(diagnosis: TestDiagnosis, language: Language): ShownDiagnosis {
  if (diagnosis.language === language) {
    return { ...diagnosis, shownLanguage: language };
  }
  const translation = diagnosis.translations.find((version) => version.language === language);
  if (!translation) {
    return { ...diagnosis, shownLanguage: diagnosis.language };
  }
  return { ...diagnosis, cause: translation.cause, fix: translation.fix, shownLanguage: language };
}

/** Both levels of a function's explanation, each in the version shown; at least one of them is there. */
export interface ShownExplanation {
  functionId: string;
  /** The short one. */
  brief: ShownBrief | null;
  /** The detailed one, in three parts (also every explanation written before short ones existed). */
  detail: ShownDetail | null;
}

/** The explanations of a review by function id, each level in the version that reads in `language` if any. */
export function shownExplanations(explanations: Explanations, language: Language): Map<string, ShownExplanation> {
  const shown = new Map<string, ShownExplanation>();
  const entry = (functionId: string) => {
    let found = shown.get(functionId);
    if (!found) {
      found = { functionId, brief: null, detail: null };
      shown.set(functionId, found);
    }
    return found;
  };
  for (const brief of explanations.briefs) entry(brief.functionId).brief = shownBrief(brief, language);
  for (const detail of explanations.functions) entry(detail.functionId).detail = shownDetail(detail, language);
  return shown;
}

/** The languages, other than `language`, that a function's explanation shows text in: what the banner counts. */
export function explanationLanguages(explanation: ShownExplanation, language: Language): Language[] {
  return [explanation.brief?.shownLanguage, explanation.detail?.shownLanguage].filter(
    (shown): shown is Language => shown !== undefined && shown !== language,
  );
}

/**
 * The languages the shown findings are written in other than `language`: what the banner names. Empty when every
 * finding reads in `language`.
 */
export function otherLanguages(findings: ShownFinding[], language: Language): Language[] {
  return [...new Set(findings.map((finding) => finding.shownLanguage).filter((shown) => shown !== language))];
}
