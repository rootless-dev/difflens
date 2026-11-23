import type { Card, JiraConnection } from "@/lib/api";

export function makeJiraConnection(overrides: Partial<JiraConnection> = {}): JiraConnection {
  return {
    site: null,
    email: null,
    account: null,
    tokenKind: null,
    connectedAt: null,
    scopes: ["read:jira-work", "read:jira-user"],
    ...overrides,
  };
}

export const JIRA_CONNECTED = makeJiraConnection({
  site: "acme.atlassian.net",
  email: "dana@example.com",
  account: { id: "5b10a28", name: "Dana" },
  tokenKind: "classic",
  connectedAt: "2026-11-19T10:00:00Z",
});

export function makeCard(overrides: Partial<Card> = {}): Card {
  return {
    key: "PAY-482",
    url: "https://acme.atlassian.net/browse/PAY-482",
    summary: "Allow refunds for settled payments",
    status: "In Progress",
    issueType: "Story",
    description: "Support wants to refund a settled payment.\n\n- in full\n- in part",
    comments: [{ author: "Ana", createdAt: "2026-11-12T09:30:00.000+0000", text: "Finance must be told." }],
    ...overrides,
  };
}
