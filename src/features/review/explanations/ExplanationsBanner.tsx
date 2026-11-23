import { useContext } from "react";

import { TranslationBanner } from "@/features/review/ai-text/TranslationBanner";
import { explanationLanguages } from "@/features/review/ai-text/versions";
import { ExplanationsContext } from "@/features/review/explanations/use-explanations";
import { useAppLanguage } from "@/i18n/language";

/**
 * Over the graph when some explanations (short or detailed) were written in another language than the one shown and
 * have no version in it: Translate sends their text, both levels (a model call, only on click, after showing who would
 * be asked). Switching the language never asks a model.
 */
export function ExplanationsBanner() {
  const explanations = useContext(ExplanationsContext);
  const language = useAppLanguage();
  const others = [...explanations.shown.values()]
    .map((explanation) => explanationLanguages(explanation, language))
    .filter((languages) => languages.length > 0);
  if (others.length === 0) return null;
  return (
    <TranslationBanner
      className="mx-4 my-2 flex-none"
      subject={{ kind: "explanations", count: others.length }}
      writtenIn={others[0][0]}
      plan={() => explanations.planTranslation(language)}
      runCall={(call, runId) => explanations.translateCall(language, call.ids, runId)}
    />
  );
}
