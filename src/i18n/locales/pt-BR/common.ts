import type { Catalog } from "@/i18n/catalog";

export const common: Catalog["common"] = {
  actions: {
    back: "← Voltar",
    cancel: "Cancelar",
    close: "Fechar",
    copy: "Copiar",
    copied: "Copiado",
    remove: "Remover",
    retry: "Tentar de novo",
    save: "Salvar",
    tryAgain: "Tentar de novo",
  },
  somethingWentWrong: "Algo deu errado.",
  analyzer: {
    preparing: "Preparando o analisador de {{language}}…",
    hint: "Só na primeira vez que um projeto em {{language}} precisa dele; depois continua sozinho.",
    step: {
      creating_environment: "Criando o ambiente",
      installing_dependencies: "Instalando as dependências",
      compiling: "Compilando",
    },
    unavailable: "O analisador de {{language}} não pôde ser preparado.",
  },
  time: {
    justNow: "agora mesmo",
    minutesAgo: "há {{count}} min",
    hoursAgo: "há {{count}} h",
    yesterday: "ontem",
    daysAgo_one: "há {{count}} dia",
    daysAgo_other: "há {{count}} dias",
  },
  bytes_one: "{{count}} byte",
  bytes_other: "{{count}} bytes",
  language: {
    label: "Idioma",
    title: "Idioma do DiffLens",
    description: "Achados e resumos são escritos no idioma que estiver na tela quando você os pedir.",
    systemResolved: "Sistema ({{language}})",
    names: {
      en: "English",
      "pt-BR": "Português",
    },
    saveFailed: "Não foi possível salvar o idioma: {{message}}",
  },
};
