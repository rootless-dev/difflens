import type { Catalog } from "@/i18n/catalog";

export const insights: Catalog["insights"] = {
  severity: { high: "Alta", medium: "Média", low: "Baixa" },
  category: {
    correctness: "Corretude",
    concurrency: "Concorrência",
    security: "Segurança",
    data_loss: "Perda de dados",
    error_handling: "Tratamento de erros",
    tests: "Testes",
    observability: "Observabilidade",
  },
  verdict: { pending: "Pendente", confirmed: "Confirmado", dismissed: "Descartado" },
  modelName: "{{provider}}, modelo {{model}}",
  plan: {
    files_one: "{{count}} arquivo",
    files_other: "{{count}} arquivos",
    calls_one: "{{count}} chamada",
    calls_other: "{{count}} chamadas",
    shape: "{{files}} em {{calls}}, cerca de {{tokens}} tokens de entrada",
    skip: {
      deleted_one: "{{count}} excluído",
      deleted_other: "{{count}} excluídos",
      binary_one: "{{count}} binário",
      binary_other: "{{count}} binários",
      too_large_one: "{{count}} grande demais para ler",
      too_large_other: "{{count}} grandes demais para ler",
      not_text_one: "{{count}} que não é texto",
      not_text_other: "{{count}} que não são texto",
    },
    skipped_one: "{{count}} arquivo é listado para o modelo, mas não é enviado: {{parts}}.",
    skipped_other: "{{count}} arquivos são listados para o modelo, mas não são enviados: {{parts}}.",
    working: "Calculando o que enviar",
    noModel: "Nenhum modelo está configurado para ler a mudança. {{problem}}",
    chooseModel: "Escolha um em Preferências › Modelo.",
    openPreferences: "Abrir Preferências",
    nothingToSend:
      "Nada nesta mudança pode ser enviado: todo arquivo alterado foi excluído, é binário ou é grande demais para ler.",
    sendsLabel: "O que Analisar envia",
    sends: "<b>{{shape}}</b>, para leitura de {{model}}.",
    truncated_one: "{{count}} arquivo grande demais para uma chamada é enviado só com as linhas alteradas.",
    truncated_other: "{{count}} arquivos grandes demais para uma chamada são enviados só com as linhas alteradas.",
    sentTo: "O código dos arquivos alterados é enviado para {{provider}}. Troque o modelo em Preferências › Modelo.",
  },
  calls: {
    one: "Chamada {{number}} de {{total}}",
    many: "Chamadas {{numbers}} de {{total}}",
    listLabel: "Chamadas que não responderam",
    failed: "A chamada {{number}} de {{total}} falhou.",
    didNotRun: "A chamada {{number}} de {{total}} não foi executada.",
  },
  writtenBy: {
    atHead: "Analisado por {{model}} em {{sha}}",
    workingTree: "Analisado por {{model}} na working tree",
  },
  location: {
    not_changed:
      "Local não encontrado: {{path}} não é um arquivo alterado nesta revisão, então não há código para mostrar.",
    not_sent: "Local não encontrado: o modelo não recebeu {{path}} na chamada que escreveu isto.",
    outside_file: "Local não encontrado: {{path}} não tem a linha {{line}}.",
    code_changed: "Local não encontrado: as linhas citadas mudaram desde a análise e não foram encontradas de novo.",
  },
  recheck: {
    confirmed: "Você confirmou isto.",
    confirmedOn: "Em {{date}}, você confirmou isto.",
    dismissed: "Você descartou isto.",
    dismissedOn: "Em {{date}}, você descartou isto.",
    codeChanged: "O código ao redor mudou desde então, então reveja o achado.",
    codeGone: "O código a que ele se refere não é mais encontrado, então reveja o achado.",
  },
  prompt: {
    copy: "Copiar como prompt para o agente",
    copyFailed: "Não foi possível copiar",
    fixAt: "Corrija um problema que uma revisão de código encontrou em {{where}}.",
    fixGone:
      "Corrija um problema que uma revisão de código encontrou em {{path}}. Ela citou a linha {{line}}, mas esse " +
      "código não foi mais encontrado: primeiro encontre o código que ela descreve.",
    problem: "Problema:",
    evidence: "O que mostra isso (lido do código, não reproduzido):",
    related: "Relacionado, mas não é prova:",
    codeNow: "O código agora ({{where}}):",
    suggestedDirection: "Direção sugerida:",
    closing:
      "Confirme que o problema é real antes de mudar qualquer coisa. Depois corrija com a menor mudança que resolva " +
      "e, onde couber, adicione um teste que falhe sem a correção e passe com ela.",
  },
  loading: "Lendo os achados",
  noFindingYet: "Nenhum achado ainda.",
  nothingFound: "O modelo não encontrou nada a apontar nesta mudança.",
  intro: {
    title: "Peça ao modelo para ler esta mudança",
    body:
      "Ele lê o diff de cada arquivo alterado e aponta defeitos que um revisor não pode deixar passar: resultados " +
      "errados, condições de corrida, falhas de segurança, perda de dados, erros não tratados, caminhos arriscados " +
      "sem testes e falhas que não deixam rastro. Ele não executa nada, e nada é enviado até você clicar em Analisar.",
  },
  analyze: "Analisar",
  analyzeAgain: "Analisar de novo",
  replaceNote:
    "A nova análise confere de novo os achados abertos com o código atual: os que o modelo der como corrigidos vão " +
    "para Corrigidos. Nada é apagado, e os achados descartados não são enviados de novo.",
  fixed: {
    group: "Corrigidos ({{count}})",
    listLabel: "Achados corrigidos",
    tag: "Corrigido",
    checkedBy: "Conferido por {{model}} {{when}}",
    markedByYou: "Marcado por você {{when}}",
    fileLeft: "Corrigido {{when}}: o arquivo não faz mais parte da mudança",
    allFixed: "Todos os achados foram corrigidos.",
    markFixed: "Marcar como corrigido",
    reopen: "Reabrir",
  },
  findingCount_one: "{{count}} achado",
  findingCount_other: "{{count}} achados",
  stopping: "Parando",
  starting: "Iniciando",
  asking: "Perguntando a {{model}}. Os achados aparecem conforme cada chamada responde.",
  findings: {
    listLabel: "Achados",
    severityLabel: "Gravidade",
    all: "Todos {{total}}",
    caveat: "Lido pelo modelo, não medido. Confira antes de confiar.",
    none: {
      all: "Nenhum achado.",
      high: "Nenhum achado de gravidade alta.",
      medium: "Nenhum achado de gravidade média.",
      low: "Nenhum achado de gravidade baixa.",
    },
  },
  detail: {
    moved: "Citado na linha {{line}}; as mesmas linhas estão aqui agora",
    earlier: "Da análise anterior de {{date}}, por {{model}}.",
    suggestedDirection: "Direção sugerida",
    evidence: "O que mostra isso",
    evidenceNote: "lido do código, não reproduzido",
    noEvidence: "O modelo não deu evidência.",
    related: "Relacionado, mas não é prova",
    openInDiff: "Abrir no Diff",
    verdictLabel: "Veredito",
    // No closing period: a date in Portuguese already ends with one ("12 de nov.").
    by: "Por {{model}}, {{when}}",
    codeAt: "Código em {{where}}",
  },
};
