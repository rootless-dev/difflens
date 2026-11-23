// Text shared by every screen: generic actions and the formatting helpers of `src/lib/format.ts`.
export const common = {
  actions: {
    back: "← Back",
    cancel: "Cancel",
    close: "Close",
    copy: "Copy",
    copied: "Copied",
    remove: "Remove",
    retry: "Retry",
    save: "Save",
    tryAgain: "Try again",
  },
  somethingWentWrong: "Something went wrong.",
  // An analyzer shipped as source, prepared the first time a project of its language needs it.
  analyzer: {
    preparing: "Preparing the {{language}} analyzer…",
    hint: "Only the first time a {{language}} project needs it; it goes on by itself.",
    step: {
      creating_environment: "Creating its environment",
      installing_dependencies: "Installing its dependencies",
      compiling: "Compiling it",
    },
    unavailable: "The {{language}} analyzer could not be prepared.",
  },
  time: {
    justNow: "just now",
    minutesAgo: "{{count}} min ago",
    hoursAgo: "{{count}} h ago",
    yesterday: "yesterday",
    daysAgo_one: "{{count}} day ago",
    daysAgo_other: "{{count}} days ago",
  },
  bytes_one: "{{count}} byte",
  bytes_other: "{{count}} bytes",
  language: {
    label: "Language",
    title: "Language of DiffLens",
    description: "Findings and summaries are written in the language shown when you ask for them.",
    systemResolved: "System ({{language}})",
    names: {
      en: "English",
      "pt-BR": "Português",
    },
    saveFailed: "The language could not be saved: {{message}}",
  },
};
