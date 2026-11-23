import { useTranslation } from "react-i18next";

/** The scopes a token needs, as code joined like a sentence: "a, b and c". */
export function ScopeList({ scopes }: { scopes: string[] }) {
  const { t } = useTranslation();
  return (
    <>
      {scopes.map((scope, index) => (
        <span key={scope}>
          {index > 0 &&
            (index === scopes.length - 1
              ? t("preferences.connection.scopeLastSeparator")
              : t("preferences.connection.scopeSeparator"))}
          <code>{scope}</code>
        </span>
      ))}
    </>
  );
}
