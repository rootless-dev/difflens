import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { PreferencesScreen } from "@/features/preferences/PreferencesScreen";
import {
  disconnect,
  disconnectJira,
  getClaudeCommand,
  getConnections,
  getJiraConnection,
  getModelSettings,
  saveConnection,
  saveJiraConnection,
  testConnection,
  testJiraConnection,
} from "@/lib/api";
import { apiError } from "@/test/fixtures";
import { JIRA_CONNECTED, makeJiraConnection } from "@/test/jira-fixtures";
import { makeModelSettings } from "@/test/model-fixtures";
import { makeConnection, makeGithubConnection } from "@/test/pull-request-fixtures";

vi.mock("@/lib/api", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/lib/api")>()),
  getConnections: vi.fn(),
  testConnection: vi.fn(),
  saveConnection: vi.fn(),
  disconnect: vi.fn(),
  getModelSettings: vi.fn(),
  getJiraConnection: vi.fn(),
  testJiraConnection: vi.fn(),
  saveJiraConnection: vi.fn(),
  disconnectJira: vi.fn(),
  getClaudeCommand: vi.fn(),
}));

const CONNECTED = makeConnection({
  username: "dana@example.com",
  account: { id: "{d4}", name: "Dana" },
  connectedAt: "2026-11-19T10:00:00Z",
});

beforeEach(() => {
  vi.resetAllMocks();
  vi.mocked(getConnections).mockResolvedValue([makeConnection()]);
  vi.mocked(getModelSettings).mockResolvedValue(makeModelSettings());
  vi.mocked(getJiraConnection).mockResolvedValue(makeJiraConnection());
  vi.mocked(getClaudeCommand).mockResolvedValue({ mode: "default", custom: null });
});

async function renderBitbucket(onBack = vi.fn()) {
  render(<PreferencesScreen onBack={onBack} />);
  const form = within(await screen.findByRole("form", { name: "Bitbucket Cloud" }));
  return {
    form,
    onBack,
    email: form.getByLabelText("Atlassian account email"),
    token: form.getByLabelText("API token"),
  };
}

describe("Preferences", () => {
  it("has Model, Terminal and Connections sections and goes back", async () => {
    const user = userEvent.setup();
    const { onBack } = await renderBitbucket();
    expect(screen.getByRole("heading", { level: 1, name: "Preferences" })).toBeInTheDocument();
    expect(screen.getByRole("region", { name: "Model" })).toBeInTheDocument();
    expect(screen.getByRole("region", { name: "Terminal" })).toBeInTheDocument();
    expect(screen.getByRole("region", { name: "Connections" })).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "← Back" }));
    expect(onBack).toHaveBeenCalledOnce();
  });
});

describe("the Bitbucket connection", () => {
  it("says it is not connected and lists the scopes the token needs", async () => {
    const { form } = await renderBitbucket();
    expect(form.getByText("Not connected. Review comments stay in DiffLens until you connect.")).toBeInTheDocument();
    expect(form.getByText("read:user:bitbucket")).toBeInTheDocument();
    expect(form.getByText("read:pullrequest:bitbucket")).toBeInTheDocument();
    expect(form.getByText("write:pullrequest:bitbucket")).toBeInTheDocument();
    expect(form.getByText(/It needs the scopes/)).toHaveTextContent(
      "It needs the scopes read:user:bitbucket, read:pullrequest:bitbucket and write:pullrequest:bitbucket.",
    );
    expect(form.getByRole("button", { name: "Save" })).toBeDisabled();
    expect(form.getByRole("button", { name: "Test connection" })).toBeDisabled();
    expect(form.queryByRole("button", { name: "Disconnect" })).not.toBeInTheDocument();
  });

  it("tests a token without saving it, and shows who it belongs to", async () => {
    const user = userEvent.setup();
    vi.mocked(testConnection).mockResolvedValue({ id: "{d4}", name: "Dana" });
    const { form, email, token } = await renderBitbucket();

    await user.type(email, "dana@example.com");
    await user.type(token, "ATATT-secret");
    await user.click(form.getByRole("button", { name: "Test connection" }));

    expect(testConnection).toHaveBeenCalledWith("bitbucket", "dana@example.com", "ATATT-secret");
    expect(await form.findByText("The connection works: this token belongs to Dana.")).toBeInTheDocument();
    expect(saveConnection).not.toHaveBeenCalled();
  });

  it("shows why a test failed", async () => {
    const user = userEvent.setup();
    vi.mocked(testConnection).mockRejectedValue(
      apiError("auth_failed", "Bitbucket did not accept the email and API token. The token may have expired or been revoked."),
    );
    const { form, email, token } = await renderBitbucket();

    await user.type(email, "dana@example.com");
    await user.type(token, "expired");
    await user.click(form.getByRole("button", { name: "Test connection" }));

    expect(await form.findByRole("alert")).toHaveTextContent("The token may have expired or been revoked.");
  });

  it("saves, then shows the account and empties the token field", async () => {
    const user = userEvent.setup();
    vi.mocked(saveConnection).mockResolvedValue(CONNECTED);
    const { form, email, token } = await renderBitbucket();

    await user.type(email, " dana@example.com ");
    await user.type(token, "ATATT-secret");
    await user.click(form.getByRole("button", { name: "Save" }));

    expect(saveConnection).toHaveBeenCalledWith("bitbucket", "dana@example.com", "ATATT-secret");
    expect(await form.findByText("Saved. Connected as Dana.")).toBeInTheDocument();
    expect(form.getAllByText(/Connected as Dana \(dana@example\.com\)/)).toHaveLength(1);
    expect(token).toHaveValue("");
    expect(form.getByRole("button", { name: "Disconnect" })).toBeInTheDocument();
  });

  it("never shows a saved token, tests the saved one, and disconnects", async () => {
    const user = userEvent.setup();
    vi.mocked(getConnections).mockResolvedValue([CONNECTED]);
    vi.mocked(testConnection).mockResolvedValue({ id: "{d4}", name: "Dana" });
    vi.mocked(disconnect).mockResolvedValue(undefined);
    const { form, email, token } = await renderBitbucket();

    expect(email).toHaveValue("dana@example.com");
    expect(token).toHaveValue("");
    expect(token).toHaveAttribute("type", "password");
    expect(form.getByText("Connected")).toBeInTheDocument();
    // Saving needs a new token; testing can use the saved one.
    expect(form.getByRole("button", { name: "Save" })).toBeDisabled();
    await user.click(form.getByRole("button", { name: "Test connection" }));
    expect(testConnection).toHaveBeenCalledWith("bitbucket", "dana@example.com", null);

    await user.click(form.getByRole("button", { name: "Disconnect" }));
    expect(disconnect).toHaveBeenCalledWith("bitbucket");
    expect(await form.findByText("Disconnected. The token was deleted from the Keychain.")).toBeInTheDocument();
    expect(form.getByText("Not connected")).toBeInTheDocument();
  });
});

describe("the GitHub connection", () => {
  const GITHUB_CONNECTED = makeGithubConnection({
    username: "dana",
    account: { id: "4", name: "dana" },
    connectedAt: "2026-11-19T10:00:00Z",
  });

  async function renderGithub() {
    render(<PreferencesScreen onBack={vi.fn()} />);
    const form = within(await screen.findByRole("form", { name: "GitHub" }));
    return { form, token: form.getByLabelText("Personal access token") };
  }

  beforeEach(() => {
    vi.mocked(getConnections).mockResolvedValue([makeConnection(), makeGithubConnection()]);
  });

  it("is listed beside Bitbucket, asks for the token alone and says what a classic or fine-grained one needs", async () => {
    const { form, token } = await renderGithub();
    expect(screen.getByRole("form", { name: "Bitbucket Cloud" })).toBeInTheDocument();
    expect(form.getByText("Pull requests on GitHub")).toBeInTheDocument();
    expect(form.getByText("Not connected. Review comments stay in DiffLens until you connect.")).toBeInTheDocument();
    expect(form.queryByLabelText("Atlassian account email")).not.toBeInTheDocument();
    expect(form.queryByLabelText("User name")).not.toBeInTheDocument();
    expect(token).toHaveAttribute("type", "password");
    expect(token).toHaveAttribute("placeholder", "Paste the personal access token");
    expect(form.getByText(/A classic token needs/)).toHaveTextContent(
      "A classic token needs the repo scope; a fine-grained token needs read and write access to Pull requests and read access to Contents and Metadata, on the repositories you review.",
    );
    expect(form.getByText("repo").tagName).toBe("CODE");
    expect(form.getByRole("button", { name: "Save" })).toBeDisabled();
    expect(form.getByRole("button", { name: "Test connection" })).toBeDisabled();
  });

  it("tests and saves the token without a user name, then shows the account", async () => {
    const user = userEvent.setup();
    vi.mocked(testConnection).mockResolvedValue({ id: "4", name: "dana" });
    vi.mocked(saveConnection).mockResolvedValue(GITHUB_CONNECTED);
    const { form, token } = await renderGithub();

    await user.type(token, "ghp_secret");
    await user.click(form.getByRole("button", { name: "Test connection" }));
    expect(testConnection).toHaveBeenCalledWith("github", "", "ghp_secret");
    expect(await form.findByText("The connection works: this token belongs to dana.")).toBeInTheDocument();

    await user.click(form.getByRole("button", { name: "Save" }));
    expect(saveConnection).toHaveBeenCalledWith("github", "", "ghp_secret");
    expect(await form.findByText("Saved. Connected as dana.")).toBeInTheDocument();
    expect(form.getByText(/^Connected as dana\. Comments are read on open and Reload/)).toBeInTheDocument();
    expect(token).toHaveValue("");
  });

  it("tests the saved token, and its login is not offered to Jira as an email", async () => {
    const user = userEvent.setup();
    vi.mocked(getConnections).mockResolvedValue([makeConnection(), GITHUB_CONNECTED]);
    vi.mocked(testConnection).mockResolvedValue({ id: "4", name: "dana" });
    const { form } = await renderGithub();

    expect(form.getByText("Connected")).toBeInTheDocument();
    await user.click(form.getByRole("button", { name: "Test connection" }));
    expect(testConnection).toHaveBeenCalledWith("github", "", null);
    const jira = within(screen.getByRole("form", { name: "Jira Cloud" }));
    expect(jira.getByLabelText("Atlassian account email")).toHaveValue("");
  });

  it("says why GitHub refused the token", async () => {
    const user = userEvent.setup();
    vi.mocked(saveConnection).mockRejectedValue(
      apiError("auth_failed", "GitHub did not accept the token. It may have expired or been revoked."),
    );
    const { form, token } = await renderGithub();
    await user.type(token, "ghp_expired");
    await user.click(form.getByRole("button", { name: "Save" }));
    expect(await form.findByRole("alert")).toHaveTextContent("GitHub did not accept the token.");
  });
});

describe("the Jira connection", () => {
  async function renderJira() {
    render(<PreferencesScreen onBack={vi.fn()} />);
    const form = within(await screen.findByRole("form", { name: "Jira Cloud" }));
    return {
      form,
      site: form.getByLabelText("Site"),
      email: form.getByLabelText("Atlassian account email"),
      token: form.getByLabelText("API token"),
    };
  }

  it("says it is not connected, lists the scopes and offers the Bitbucket email, not its token", async () => {
    vi.mocked(getConnections).mockResolvedValue([CONNECTED]);
    const { form, site, email, token } = await renderJira();

    expect(form.getByText(/Not connected\. Cards are found from the branch name/)).toBeInTheDocument();
    expect(form.getByText(/a token with scopes needs/)).toHaveTextContent(
      "a token with scopes needs read:jira-work and read:jira-user.",
    );
    expect(site).toHaveValue("");
    expect(email).toHaveValue("dana@example.com");
    expect(token).toHaveValue("");
    expect(form.getByRole("button", { name: "Save" })).toBeDisabled();
    expect(form.queryByRole("button", { name: "Disconnect" })).not.toBeInTheDocument();
  });

  it("tests and saves the site, email and token, then shows the account and empties the token field", async () => {
    const user = userEvent.setup();
    vi.mocked(testJiraConnection).mockResolvedValue({ id: "5b10a28", name: "Dana" });
    vi.mocked(saveJiraConnection).mockResolvedValue(JIRA_CONNECTED);
    const { form, site, email, token } = await renderJira();

    await user.type(site, "acme");
    await user.type(email, "dana@example.com");
    await user.type(token, "ATATT-jira");
    await user.click(form.getByRole("button", { name: "Test connection" }));
    expect(testJiraConnection).toHaveBeenCalledWith("acme", "dana@example.com", "ATATT-jira");
    expect(await form.findByText("The connection works: this token belongs to Dana.")).toBeInTheDocument();
    expect(saveJiraConnection).not.toHaveBeenCalled();

    await user.click(form.getByRole("button", { name: "Save" }));
    expect(saveJiraConnection).toHaveBeenCalledWith("acme", "dana@example.com", "ATATT-jira");
    expect(await form.findByText("Saved. Connected as Dana on acme.atlassian.net.")).toBeInTheDocument();
    expect(site).toHaveValue("acme.atlassian.net");
    expect(token).toHaveValue("");
    expect(form.getByText(/Saved as a classic API token/)).toBeInTheDocument();
  });

  it("shows why Jira refused the token", async () => {
    const user = userEvent.setup();
    vi.mocked(saveJiraConnection).mockRejectedValue(
      apiError("missing_scope", "Jira refused this: the API token is missing a scope."),
    );
    const { form, site, email, token } = await renderJira();
    await user.type(site, "acme.atlassian.net");
    await user.type(email, "dana@example.com");
    await user.type(token, "ATATT-jira");
    await user.click(form.getByRole("button", { name: "Save" }));
    expect(await form.findByRole("alert")).toHaveTextContent("the API token is missing a scope");
  });

  it("tests the saved token without showing it, and disconnects", async () => {
    const user = userEvent.setup();
    vi.mocked(getJiraConnection).mockResolvedValue(JIRA_CONNECTED);
    vi.mocked(testJiraConnection).mockResolvedValue({ id: "5b10a28", name: "Dana" });
    vi.mocked(disconnectJira).mockResolvedValue(undefined);
    const { form, site, token } = await renderJira();

    expect(site).toHaveValue("acme.atlassian.net");
    expect(token).toHaveValue("");
    expect(form.getByText(/acme\.atlassian\.net, as Dana \(dana@example\.com\)/)).toBeInTheDocument();
    await user.click(form.getByRole("button", { name: "Test connection" }));
    expect(testJiraConnection).toHaveBeenCalledWith("acme.atlassian.net", "dana@example.com", null);

    await user.click(form.getByRole("button", { name: "Disconnect" }));
    expect(disconnectJira).toHaveBeenCalledOnce();
    expect(await form.findByText("Disconnected. The token was deleted from the Keychain.")).toBeInTheDocument();
    expect(form.getByText("Not connected")).toBeInTheDocument();
  });
});
