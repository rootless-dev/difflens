import { useTranslation } from "react-i18next";

import { ErrorNote, Page, PageTitle } from "@/components/page";
import { Button } from "@/components/ui/button";
import { Skeleton } from "@/components/ui/skeleton";
import { FullAnalysisSection } from "@/features/preferences/FullAnalysisSection";
import { HostConnection } from "@/features/preferences/HostConnection";
import { JiraConnectionForm } from "@/features/preferences/JiraConnectionForm";
import { LanguageSection } from "@/features/preferences/LanguageSection";
import { ModelSection } from "@/features/preferences/ModelSection";
import { PreferencesSection } from "@/features/preferences/PreferencesSection";
import { TerminalSection } from "@/features/preferences/TerminalSection";
import { useAsyncData } from "@/hooks/use-async-data";
import { type Connection, getConnections, getJiraConnection, type JiraConnection } from "@/lib/api";

interface PreferencesScreenProps {
  /** Back to the screen Preferences was opened from. */
  onBack: () => void;
}

/** Settings that apply to every project on this machine, one section each: Language, Model, Full analysis (how
 * many run at once), Terminal (what Claude Code sessions run), and Connections (Jira and hosts). */
export function PreferencesScreen({ onBack }: PreferencesScreenProps) {
  const { t } = useTranslation();
  return (
    <Page>
      <Button variant="link" size="inline" className="-mt-4 mb-3.5" onClick={onBack}>
        {t("common.actions.back")}
      </Button>
      <PageTitle>{t("preferences.title")}</PageTitle>
      <p className="mt-1 text-muted-foreground">{t("preferences.description")}</p>
      <LanguageSection />
      <ModelSection />
      <FullAnalysisSection />
      <TerminalSection />
      <ConnectionsSection />
    </Page>
  );
}

interface Connections {
  hosts: Connection[];
  jira: JiraConnection;
}

const loadConnections = async (): Promise<Connections> => {
  const [hosts, jira] = await Promise.all([getConnections(), getJiraConnection()]);
  return { hosts, jira };
};

function ConnectionsSection() {
  const { t } = useTranslation();
  const { state, reload, update } = useAsyncData(loadConnections);
  return (
    <PreferencesSection title={t("preferences.connections.title")}>
      {state.status === "loading" && (
        <div role="status" aria-label={t("preferences.connections.loading")} className="grid gap-2 py-3">
          <Skeleton className="h-3.5 w-1/3" />
          <Skeleton className="h-3.5 w-2/3" />
        </div>
      )}
      {state.status === "error" && (
        <div className="py-3">
          <ErrorNote>{state.message}</ErrorNote>
          <Button variant="secondary" size="sm" className="mt-2.5" onClick={reload}>
            {t("common.actions.tryAgain")}
          </Button>
        </div>
      )}
      {state.status === "ready" && (
        <>
          <JiraConnectionForm
            connection={state.data.jira}
            // Only Bitbucket's user name is an Atlassian account email; GitHub's is a login.
            suggestedEmail={state.data.hosts.find((host) => host.host === "bitbucket")?.username ?? null}
            onChange={(jira) => update((connections) => ({ ...connections, jira }))}
          />
          {state.data.hosts.map((connection) => (
            <HostConnection
              key={connection.host}
              connection={connection}
              onChange={(next) =>
                update((connections) => ({
                  ...connections,
                  hosts: connections.hosts.map((item) => (item.host === next.host ? next : item)),
                }))
              }
            />
          ))}
        </>
      )}
    </PreferencesSection>
  );
}
