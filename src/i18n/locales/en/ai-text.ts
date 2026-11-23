// Text the model wrote (Insights findings, the Overview summary, explanations of functions): the banner over a version in another language and
// the Translate flow.
export const aiText = {
  /** A language as a sentence names it: "written in English". */
  languageInSentence: {
    en: "English",
    "pt-BR": "Portuguese",
  },
  writtenIn: {
    findings_one: "This finding was written in {{language}}.",
    findings_other: "These findings were written in {{language}}.",
    summary: "This summary was written in {{language}}.",
    explanations_one: "An explanation of a function was written in {{language}}.",
    explanations_other: "{{count}} explanations of functions were written in {{language}}.",
    diagnoses_one: "An explanation of a failure was written in {{language}}.",
    diagnoses_other: "{{count}} explanations of failures were written in {{language}}.",
  },
  label: "Language of this text",
  translate: "Translate",
  analyzeAgain: "Analyze again",
  summarizeAgain: "Summarize again",
  planning: "Working out what to send",
  model: "{{provider}}, model {{model}}",
  calls_one: "{{count}} call",
  calls_other: "{{count}} calls",
  confirm: {
    findings_one:
      "Translate sends the text of {{count}} finding to {{model}}, in {{calls}}, about {{tokens}} tokens in. Only its " +
      "text changes: the finding, its verdict and its place stay as they are.",
    findings_other:
      "Translate sends the text of {{count}} findings to {{model}}, in {{calls}}, about {{tokens}} tokens in. Only " +
      "their text changes: the findings, their verdicts and their places stay as they are.",
    summary:
      "Translate sends the text of this summary to {{model}}, about {{tokens}} tokens in. Only its text changes: the " +
      "verdicts and their places stay as they are.",
    explanations_one:
      "Translate sends the text of {{count}} explanation to {{model}}, in {{calls}}, about {{tokens}} tokens in. The " +
      "code is not sent again.",
    explanations_other:
      "Translate sends the text of {{count}} explanations to {{model}}, in {{calls}}, about {{tokens}} tokens in. The " +
      "code is not sent again.",
    diagnoses_one:
      "Translate sends the text of {{count}} explanation of a failure to {{model}}, in {{calls}}, about {{tokens}} " +
      "tokens in. The code and the test output are not sent again.",
    diagnoses_other:
      "Translate sends the text of {{count}} explanations of failures to {{model}}, in {{calls}}, about {{tokens}} " +
      "tokens in. The code and the test output are not sent again.",
  },
  notReady: "No model is set up to translate. {{problem}}",
  chooseModel: "Choose one in Preferences › Model.",
  translating: "Translating with {{model}}…",
  stop: "Stop",
  stopping: "Stopping",
  failed: "The translation failed: {{message}}",
  partly_one: "{{count}} finding was not translated and keeps its original text.",
  partly_other: "{{count}} findings were not translated and keep their original text.",
};
