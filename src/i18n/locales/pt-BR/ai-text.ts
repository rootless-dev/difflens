import type { Catalog } from "@/i18n/catalog";

export const aiText: Catalog["aiText"] = {
  languageInSentence: {
    en: "inglês",
    "pt-BR": "português",
  },
  writtenIn: {
    findings_one: "Este achado foi escrito em {{language}}.",
    findings_other: "Estes achados foram escritos em {{language}}.",
    summary: "Este resumo foi escrito em {{language}}.",
    explanations_one: "A explicação de uma função foi escrita em {{language}}.",
    explanations_other: "{{count}} explicações de funções foram escritas em {{language}}.",
    diagnoses_one: "A explicação de uma falha foi escrita em {{language}}.",
    diagnoses_other: "{{count}} explicações de falhas foram escritas em {{language}}.",
  },
  label: "Idioma deste texto",
  translate: "Traduzir",
  analyzeAgain: "Analisar de novo",
  summarizeAgain: "Resumir de novo",
  planning: "Calculando o que enviar",
  model: "{{provider}}, modelo {{model}}",
  calls_one: "{{count}} chamada",
  calls_other: "{{count}} chamadas",
  confirm: {
    findings_one:
      "Traduzir envia o texto de {{count}} achado para {{model}}, em {{calls}}, cerca de {{tokens}} tokens de " +
      "entrada. Só o texto muda: o achado, o veredito e o local continuam como estão.",
    findings_other:
      "Traduzir envia o texto de {{count}} achados para {{model}}, em {{calls}}, cerca de {{tokens}} tokens de " +
      "entrada. Só o texto muda: os achados, os vereditos e os locais continuam como estão.",
    summary:
      "Traduzir envia o texto deste resumo para {{model}}, cerca de {{tokens}} tokens de entrada. Só o texto muda: " +
      "os vereditos e os locais continuam como estão.",
    explanations_one:
      "Traduzir envia o texto de {{count}} explicação para {{model}}, em {{calls}}, cerca de {{tokens}} tokens de " +
      "entrada. O código não é enviado de novo.",
    explanations_other:
      "Traduzir envia o texto de {{count}} explicações para {{model}}, em {{calls}}, cerca de {{tokens}} tokens de " +
      "entrada. O código não é enviado de novo.",
    diagnoses_one:
      "Traduzir envia o texto de {{count}} explicação de falha para {{model}}, em {{calls}}, cerca de {{tokens}} " +
      "tokens de entrada. O código e a saída do teste não são enviados de novo.",
    diagnoses_other:
      "Traduzir envia o texto de {{count}} explicações de falhas para {{model}}, em {{calls}}, cerca de {{tokens}} " +
      "tokens de entrada. O código e a saída do teste não são enviados de novo.",
  },
  notReady: "Nenhum modelo está configurado para traduzir. {{problem}}",
  chooseModel: "Escolha um em Preferências › Modelo.",
  translating: "Traduzindo com {{model}}…",
  stop: "Parar",
  stopping: "Parando",
  failed: "A tradução falhou: {{message}}",
  partly_one: "{{count}} achado não foi traduzido e mantém o texto original.",
  partly_other: "{{count}} achados não foram traduzidos e mantêm o texto original.",
};
