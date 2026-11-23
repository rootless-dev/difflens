import { type FormEvent, useId, useState } from "react";
import { Trans, useTranslation } from "react-i18next";

import { Hint } from "@/components/page";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { ScopeList } from "@/features/preferences/ScopeList";
import { i18n } from "@/i18n";
import { type Connection, disconnect, saveConnection, testConnection } from "@/lib/api";
import { errorMessage } from "@/lib/format";
import { cn } from "@/lib/utils";

/** How a host names its credential, for the fields, as catalog keys. Hosts not listed get the generic words. */
interface HostWords {
  /** Shown only when the host needs a user name besides the token. */
  username: "preferences.connection.email" | "preferences.host.username";
  usernamePlaceholder: "preferences.connection.emailPlaceholder" | null;
  token: "preferences.connection.apiToken" | "preferences.host.githubToken";
  tokenPlaceholder: "preferences.connection.tokenPlaceholder" | "preferences.host.githubTokenPlaceholder";
  /** Where the user gets a token, for the generic hint. */
  tokenSource: "preferences.host.bitbucketTokenSource" | "preferences.host.tokenSource";
  /** What the token needs, with <scopes/> for the host's scopes. */
  hint: "preferences.host.hint" | "preferences.host.githubHint";
}

const GENERIC: HostWords = {
  username: "preferences.host.username",
  usernamePlaceholder: null,
  token: "preferences.connection.apiToken",
  tokenPlaceholder: "preferences.connection.tokenPlaceholder",
  tokenSource: "preferences.host.tokenSource",
  hint: "preferences.host.hint",
};

const WORDS: Record<string, HostWords> = {
  bitbucket: {
    ...GENERIC,
    username: "preferences.connection.email",
    usernamePlaceholder: "preferences.connection.emailPlaceholder",
    tokenSource: "preferences.host.bitbucketTokenSource",
  },
  // A classic token has scopes, a fine-grained one permissions: the hint names both.
  github: {
    ...GENERIC,
    token: "preferences.host.githubToken",
    tokenPlaceholder: "preferences.host.githubTokenPlaceholder",
    hint: "preferences.host.githubHint",
  },
};

type Pending = "test" | "save" | "disconnect";

/** What the last action found, under the fields. */
type Outcome = { tone: "ok" | "error"; text: string };

interface HostConnectionProps {
  connection: Connection;
  onChange: (connection: Connection) => void;
}

/**
 * One pull request host: who DiffLens is connected as, and the fields to connect, test or disconnect. The saved token
 * never comes back from the core, so the token field starts empty. A host whose token alone says who it is (GitHub)
 * has no user name field.
 */
export function HostConnection({ connection, onChange }: HostConnectionProps) {
  const { t } = useTranslation();
  const words = WORDS[connection.host] ?? GENERIC;
  const usernameId = useId();
  const tokenId = useId();
  const [username, setUsername] = useState(connection.username ?? "");
  const [token, setToken] = useState("");
  const [pending, setPending] = useState<Pending | null>(null);
  const [outcome, setOutcome] = useState<Outcome | null>(null);

  const connected = connection.account !== null;
  const { needsUsername } = connection;
  const typedUsername = needsUsername ? username.trim() : "";
  const typedToken = token.trim();
  const usernameReady = !needsUsername || typedUsername !== "";
  // Without a new token, the saved one can be tested, but only for the account it was saved for.
  const canTest =
    usernameReady &&
    (typedToken !== "" || (connected && (!needsUsername || typedUsername === connection.username)));
  const canSave = usernameReady && typedToken !== "";

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
      const account = await testConnection(connection.host, typedUsername, typedToken === "" ? null : typedToken);
      return { tone: "ok", text: i18n.t("preferences.connection.works", { name: account.name }) };
    });

  const save = (event: FormEvent) => {
    event.preventDefault();
    if (!canSave || pending !== null) return;
    void act("save", async () => {
      const saved = await saveConnection(connection.host, typedUsername, typedToken);
      setToken("");
      onChange(saved);
      return { tone: "ok", text: i18n.t("preferences.host.saved", { name: saved.account?.name ?? typedUsername }) };
    });
  };

  const forget = () =>
    act("disconnect", async () => {
      await disconnect(connection.host);
      onChange({ ...connection, username: null, account: null, connectedAt: null });
      setToken("");
      return { tone: "ok", text: i18n.t("preferences.connection.disconnected") };
    });

  return (
    <form
      aria-label={connection.hostName}
      onSubmit={save}
      className="grid gap-3 border-b px-2 py-3"
    >
      <div className="grid grid-cols-[1fr_auto] items-center gap-4">
        <div>
          <strong className="block font-medium">{t("preferences.host.title", { host: connection.hostName })}</strong>
          <span role="status" className="text-muted-foreground">
            {connected
              ? needsUsername
                ? t("preferences.host.connectedAs", {
                    name: connection.account?.name,
                    username: connection.username,
                  })
                : t("preferences.host.connectedAsAccount", { name: connection.account?.name })
              : t("preferences.host.notConnected")}
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
        {needsUsername && (
          <>
            <Label htmlFor={usernameId}>{t(words.username)}</Label>
            <Input
              id={usernameId}
              value={username}
              onChange={(event) => setUsername(event.target.value)}
              placeholder={words.usernamePlaceholder ? t(words.usernamePlaceholder) : ""}
              disabled={pending !== null}
              autoComplete="off"
              spellCheck={false}
            />
          </>
        )}
        <Label htmlFor={tokenId}>{t(words.token)}</Label>
        <Input
          id={tokenId}
          type="password"
          value={token}
          onChange={(event) => setToken(event.target.value)}
          placeholder={connected ? t("preferences.connection.tokenPlaceholderSaved") : t(words.tokenPlaceholder)}
          disabled={pending !== null}
          autoComplete="off"
          spellCheck={false}
        />
      </div>
      <Hint className="mt-0 ml-[166px]">
        <Trans
          i18nKey={words.hint}
          values={{ tokenSource: t(words.tokenSource) }}
          components={{ scopes: <ScopeList scopes={connection.scopes} /> }}
        />
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
