import { type FormEvent, useId, useState } from "react";
import { Trans, useTranslation } from "react-i18next";

import { Hint } from "@/components/page";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { ScopeList } from "@/features/preferences/ScopeList";
import { i18n } from "@/i18n";
import {
  disconnectJira,
  type JiraConnection,
  saveJiraConnection,
  testJiraConnection,
} from "@/lib/api";
import { errorMessage } from "@/lib/format";
import { cn } from "@/lib/utils";

type Pending = "test" | "save" | "disconnect";

/** What the last action found, under the fields. */
type Outcome = { tone: "ok" | "error"; text: string };

interface JiraConnectionFormProps {
  connection: JiraConnection;
  /** The email of the Bitbucket connection, offered until Jira has one of its own. The token is never shared. */
  suggestedEmail: string | null;
  onChange: (connection: JiraConnection) => void;
}

/**
 * The Jira Cloud connection (the prototype's Connections › Jira row): the site, who DiffLens is connected as, and the
 * fields to connect, test or disconnect. The saved token never comes back from the core, so its field starts empty.
 */
export function JiraConnectionForm({ connection, suggestedEmail, onChange }: JiraConnectionFormProps) {
  const { t } = useTranslation();
  const siteId = useId();
  const emailId = useId();
  const tokenId = useId();
  const [site, setSite] = useState(connection.site ?? "");
  const [email, setEmail] = useState(connection.email ?? suggestedEmail ?? "");
  const [token, setToken] = useState("");
  const [pending, setPending] = useState<Pending | null>(null);
  const [outcome, setOutcome] = useState<Outcome | null>(null);

  const connected = connection.account !== null;
  const typedSite = site.trim();
  const typedEmail = email.trim();
  const typedToken = token.trim();
  // Without a new token, the saved one can be tested, but only for the site and email it was saved for.
  const savedFor =
    connected && sameSite(typedSite, connection.site ?? "") && typedEmail === connection.email;
  const canTest = typedSite !== "" && typedEmail !== "" && (typedToken !== "" || savedFor);
  const canSave = typedSite !== "" && typedEmail !== "" && typedToken !== "";

  async function act(kind: Pending, action: () => Promise<Outcome>) {
    setPending(kind);
    setOutcome(null);
    try {
      setOutcome(await action());
    } catch (cause: unknown) {
      setOutcome({ tone: "error", text: errorMessage(cause) });
    } finally {
      setPending(null);
    }
  }

  const test = () =>
    act("test", async () => {
      const account = await testJiraConnection(typedSite, typedEmail, typedToken === "" ? null : typedToken);
      return { tone: "ok", text: i18n.t("preferences.connection.works", { name: account.name }) };
    });

  const save = (event: FormEvent) => {
    event.preventDefault();
    if (!canSave || pending !== null) return;
    void act("save", async () => {
      const saved = await saveJiraConnection(typedSite, typedEmail, typedToken);
      setToken("");
      setSite(saved.site ?? typedSite);
      onChange(saved);
      return {
        tone: "ok",
        text: i18n.t("preferences.jira.saved", { name: saved.account?.name ?? typedEmail, site: saved.site }),
      };
    });
  };

  const forget = () =>
    act("disconnect", async () => {
      await disconnectJira();
      onChange({ ...connection, site: null, email: null, account: null, tokenKind: null, connectedAt: null });
      setToken("");
      return { tone: "ok", text: i18n.t("preferences.connection.disconnected") };
    });

  return (
    <form aria-label="Jira Cloud" onSubmit={save} className="grid gap-3 border-b px-2 py-3">
      <div className="grid grid-cols-[1fr_auto] items-center gap-4">
        <div>
          <strong className="block font-medium">Jira</strong>
          <span role="status" className="text-muted-foreground">
            {connected ? (
              <Trans
                i18nKey="preferences.jira.connected"
                values={{ site: connection.site, name: connection.account?.name, email: connection.email }}
                components={{ code: <code /> }}
              />
            ) : (
              <Trans i18nKey="preferences.jira.notConnected" components={{ code: <code /> }} />
            )}
          </span>
        </div>
        <span
          className={cn(
            "rounded-[9px] border px-[9px] py-px text-[11px] whitespace-nowrap",
            connected ? "border-add text-add" : "text-muted-foreground",
          )}
        >
          {connected ? t("preferences.connection.connected") : t("preferences.connection.notConnected")}
        </span>
      </div>
      <div className="grid grid-cols-[150px_1fr] items-center gap-x-4 gap-y-2.5">
        <Label htmlFor={siteId}>{t("preferences.jira.site")}</Label>
        <Input
          id={siteId}
          value={site}
          onChange={(event) => setSite(event.target.value)}
          placeholder="acme.atlassian.net"
          disabled={pending !== null}
          autoComplete="off"
          spellCheck={false}
        />
        <Label htmlFor={emailId}>{t("preferences.connection.email")}</Label>
        <Input
          id={emailId}
          value={email}
          onChange={(event) => setEmail(event.target.value)}
          placeholder={t("preferences.connection.emailPlaceholder")}
          disabled={pending !== null}
          autoComplete="off"
          spellCheck={false}
        />
        <Label htmlFor={tokenId}>{t("preferences.connection.apiToken")}</Label>
        <Input
          id={tokenId}
          type="password"
          value={token}
          onChange={(event) => setToken(event.target.value)}
          placeholder={
            connected ? t("preferences.connection.tokenPlaceholderSaved") : t("preferences.connection.tokenPlaceholder")
          }
          disabled={pending !== null}
          autoComplete="off"
          spellCheck={false}
        />
      </div>
      <Hint className="mt-0 ml-[166px]">
        <Trans i18nKey="preferences.jira.hint" components={{ scopes: <ScopeList scopes={connection.scopes} /> }} />
        {connected && connection.tokenKind && (
          <>
            {" "}
            {t("preferences.jira.savedAs", { kind: t(`preferences.jira.tokenKind.${connection.tokenKind}`) })}
          </>
        )}
      </Hint>
      <div className="ml-[166px] flex flex-wrap items-center gap-2.5">
        <Button type="submit" size="sm" disabled={!canSave || pending !== null}>
          {pending === "save" ? t("preferences.saving") : t("common.actions.save")}
        </Button>
        <Button variant="secondary" size="sm" disabled={!canTest || pending !== null} onClick={() => void test()}>
          {pending === "test" ? t("preferences.testing") : t("preferences.connection.test")}
        </Button>
        {connected && (
          <Button variant="destructive" size="sm" disabled={pending !== null} onClick={() => void forget()}>
            {pending === "disconnect" ? t("preferences.connection.disconnecting") : t("preferences.connection.disconnect")}
          </Button>
        )}
        {outcome && (
          <span
            role={outcome.tone === "error" ? "alert" : "status"}
            className={cn("text-xs", outcome.tone === "error" ? "text-destructive" : "text-add")}
          >
            {outcome.text}
          </span>
        )}
      </div>
    </form>
  );
}

/** The site as typed and the one saved name the same site: "acme" and "acme.atlassian.net" do. */
function sameSite(typed: string, saved: string): boolean {
  const bare = (site: string) =>
    site
      .toLowerCase()
      .replace(/^https?:\/\//, "")
      .replace(/[/?#].*$/, "")
      .replace(/\.$/, "")
      .replace(/\.atlassian\.net$/, "");
  return saved !== "" && bare(typed) === bare(saved);
}
