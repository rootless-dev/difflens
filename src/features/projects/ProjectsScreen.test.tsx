import { open } from "@tauri-apps/plugin-dialog";
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { ProjectsScreen } from "@/features/projects/ProjectsScreen";
import { resetProjectFilters } from "@/features/projects/project-filters";
import { addLocalProject, cloneProject, listProjects, type Project } from "@/lib/api";
import { apiError, deferred, makeProject } from "@/test/fixtures";

vi.mock("@/lib/api", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/lib/api")>()),
  listProjects: vi.fn(),
  addLocalProject: vi.fn(),
  cloneProject: vi.fn(),
}));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));

const onOpenProject = vi.fn();

function renderScreen() {
  return render(<ProjectsScreen onOpenProject={onOpenProject} />);
}

const DAY = 24 * 60 * 60 * 1000;

function daysAgo(days: number): string {
  return new Date(Date.now() - days * DAY).toISOString();
}

/** Names of the listed projects, top to bottom. */
function listedNames(): string[] {
  return within(screen.getByRole("list", { name: "Projects" }))
    .getAllByRole("button")
    .map((row) => row.firstElementChild?.firstElementChild?.textContent ?? "");
}

async function openCloneBox() {
  const user = userEvent.setup();
  renderScreen();
  await screen.findByText("No projects yet");
  await user.click(screen.getByRole("button", { name: "Clone repository" }));
  return user;
}

beforeEach(() => {
  vi.resetAllMocks();
  resetProjectFilters();
  vi.mocked(listProjects).mockResolvedValue([]);
});

describe("project list", () => {
  it("shows the empty state when there are no projects", async () => {
    renderScreen();

    expect(screen.getByRole("status", { name: "Loading projects" })).toBeInTheDocument();
    expect(await screen.findByText("No projects yet")).toBeInTheDocument();
    expect(screen.queryByRole("list", { name: "Projects" })).not.toBeInTheDocument();
  });

  it("shows the status of each project", async () => {
    vi.mocked(listProjects).mockResolvedValue([
      makeProject({ id: "a", name: "payments-api", branchesAhead: 2 }),
      makeProject({ id: "b", name: "ledger-worker", branchesAhead: 1 }),
      makeProject({ id: "c", name: "notifier" }),
      makeProject({
        id: "d",
        name: "backoffice-web",
        languages: ["typescript"],
        analysisSupport: "diff_only",
        branchesAhead: 3,
      }),
      makeProject({ id: "e", name: "gone", path: "/work/gone", available: false }),
    ]);
    renderScreen();

    const row = async (name: string) =>
      within(await screen.findByRole("button", { name: new RegExp(`^${name}`) }));

    expect((await row("payments-api")).getByText("2 branches to review")).toBeInTheDocument();
    expect((await row("payments-api")).getByText("Go")).toBeInTheDocument();
    expect((await row("ledger-worker")).getByText("1 branch to review")).toBeInTheDocument();
    expect((await row("notifier")).getByText("Nothing to review")).toBeInTheDocument();
    const diffOnly = await row("backoffice-web");
    expect(diffOnly.getByText("TypeScript")).toBeInTheDocument();
    expect(diffOnly.getByText("Language not supported yet")).toBeInTheDocument();
    expect(diffOnly.getByText("Diff only")).toBeInTheDocument();
    expect((await row("gone")).getByText("Repository unavailable")).toBeInTheDocument();
  });

  it("opens a project when its row is clicked", async () => {
    const project = makeProject({ name: "notifier" });
    vi.mocked(listProjects).mockResolvedValue([project]);
    renderScreen();

    await userEvent.click(await screen.findByRole("button", { name: /notifier/ }));

    expect(onOpenProject).toHaveBeenCalledWith(project);
  });

  it("shows the error and retries when the list cannot be loaded", async () => {
    vi.mocked(listProjects).mockRejectedValueOnce(apiError("internal", "Could not read the project list."));
    renderScreen();

    expect(await screen.findByRole("alert")).toHaveTextContent("Could not read the project list.");

    await userEvent.click(screen.getByRole("button", { name: "Try again" }));

    expect(await screen.findByText("No projects yet")).toBeInTheDocument();
  });
});

describe("search, language filter and sort", () => {
  function projects(): Project[] {
    return [
      makeProject({ id: "a", name: "notifier", languages: ["python", "typescript"], lastModifiedAt: daysAgo(10) }),
      makeProject({ id: "b", name: "empty-repo", languages: [], lastModifiedAt: null }),
      makeProject({ id: "c", name: "payments-api", languages: ["go"], lastModifiedAt: daysAgo(1) }),
      makeProject({ id: "d", name: "gone", languages: [], available: false, lastModifiedAt: null }),
      makeProject({ id: "e", name: "ação-relatórios", languages: ["go"], lastModifiedAt: daysAgo(3) }),
    ];
  }

  async function renderProjects() {
    vi.mocked(listProjects).mockResolvedValue(projects());
    const user = userEvent.setup();
    const view = renderScreen();
    await screen.findByRole("list", { name: "Projects" });
    return { user, view };
  }

  const search = () => screen.getByRole("searchbox", { name: "Search projects" });
  const languageTrigger = (summary: string) =>
    screen.getByRole("button", { name: `Language filter: ${summary}` });

  it("sorts by last modification, most recent first, with unknown dates last by name", async () => {
    const { user } = await renderProjects();

    expect(search()).toHaveAttribute("placeholder", "Search projects…");
    expect(screen.getByRole("combobox", { name: "Sort projects" })).toHaveTextContent("Recently modified");
    expect(listedNames()).toEqual(["payments-api", "ação-relatórios", "notifier", "empty-repo", "gone"]);
    const row = within(screen.getByRole("button", { name: /^payments-api/ }));
    expect(row.getByText("modified yesterday")).toBeInTheDocument();
    expect(within(screen.getByRole("button", { name: /^ação/ })).getByText("modified 3 days ago")).toBeInTheDocument();
    expect(within(screen.getByRole("button", { name: /^gone/ })).queryByText(/^modified/)).not.toBeInTheDocument();

    await user.click(screen.getByRole("combobox", { name: "Sort projects" }));
    await user.click(await screen.findByRole("option", { name: "Least recently modified" }));

    expect(screen.getByRole("combobox", { name: "Sort projects" })).toHaveTextContent("Least recently modified");
    expect(listedNames()).toEqual(["notifier", "ação-relatórios", "payments-api", "empty-repo", "gone"]);
  });

  it("searches names ignoring case and accents, and Escape clears the search", async () => {
    const { user } = await renderProjects();

    await user.type(search(), "ACAO");
    expect(listedNames()).toEqual(["ação-relatórios"]);

    await user.clear(search());
    await user.type(search(), "relatorios");
    expect(listedNames()).toEqual(["ação-relatórios"]);

    await user.clear(search());
    await user.type(search(), "Pay");
    expect(listedNames()).toEqual(["payments-api"]);

    await user.keyboard("{Escape}");
    expect(search()).toHaveValue("");
    expect(listedNames()).toHaveLength(5);
  });

  it("filters by any of the chosen languages, with a count for each and an option for none", async () => {
    const { user } = await renderProjects();

    await user.click(languageTrigger("All languages"));
    const menu = within(await screen.findByRole("menu"));
    expect(menu.getAllByRole("menuitemcheckbox").map((item) => item.getAttribute("aria-label"))).toEqual([
      "Go, 2 projects",
      "Python, 1 project",
      "TypeScript, 1 project",
      "No language, 2 projects",
    ]);
    expect(menu.getByRole("menuitem", { name: "Show all languages" })).toHaveAttribute("aria-disabled", "true");

    await user.click(menu.getByRole("menuitemcheckbox", { name: "Python, 1 project" }));
    expect(menu.getByRole("menuitemcheckbox", { name: "Python, 1 project" })).toHaveAttribute("aria-checked", "true");
    await user.keyboard("{Escape}");
    expect(languageTrigger("Python")).toBeInTheDocument();
    expect(listedNames()).toEqual(["notifier"]);

    await user.click(languageTrigger("Python"));
    await user.click(await screen.findByRole("menuitemcheckbox", { name: "Go, 2 projects" }));
    await user.keyboard("{Escape}");
    expect(languageTrigger("2 languages")).toBeInTheDocument();
    expect(listedNames()).toEqual(["payments-api", "ação-relatórios", "notifier"]);

    await user.click(languageTrigger("2 languages"));
    await user.click(await screen.findByRole("menuitemcheckbox", { name: "Go, 2 projects" }));
    await user.click(screen.getByRole("menuitemcheckbox", { name: "Python, 1 project" }));
    await user.click(screen.getByRole("menuitemcheckbox", { name: "No language, 2 projects" }));
    await user.keyboard("{Escape}");
    expect(languageTrigger("No language")).toBeInTheDocument();
    expect(listedNames()).toEqual(["empty-repo", "gone"]);

    await user.click(languageTrigger("No language"));
    await user.click(await screen.findByRole("menuitem", { name: "Show all languages" }));
    expect(languageTrigger("All languages")).toBeInTheDocument();
    expect(listedNames()).toHaveLength(5);
  });

  it("says when nothing matches and clears the search and the languages", async () => {
    const { user } = await renderProjects();
    await user.click(languageTrigger("All languages"));
    await user.click(await screen.findByRole("menuitemcheckbox", { name: "Go, 2 projects" }));
    await user.keyboard("{Escape}");

    await user.type(search(), "notifier");

    expect(screen.getByText("No projects found")).toBeInTheDocument();
    expect(screen.queryByRole("list", { name: "Projects" })).not.toBeInTheDocument();
    expect(screen.queryByText("No projects yet")).not.toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Clear filters" }));

    expect(search()).toHaveValue("");
    expect(search()).toHaveFocus();
    expect(languageTrigger("All languages")).toBeInTheDocument();
    expect(listedNames()).toHaveLength(5);
  });

  it("keeps the sort and the languages after going to a project and back, not the search", async () => {
    const { user, view } = await renderProjects();
    await user.click(screen.getByRole("combobox", { name: "Sort projects" }));
    await user.click(await screen.findByRole("option", { name: "Least recently modified" }));
    await user.click(languageTrigger("All languages"));
    await user.click(await screen.findByRole("menuitemcheckbox", { name: "Go, 2 projects" }));
    await user.keyboard("{Escape}");
    await user.type(search(), "pay");

    view.unmount();
    renderScreen();
    await screen.findByRole("list", { name: "Projects" });

    expect(search()).toHaveValue("");
    expect(screen.getByRole("combobox", { name: "Sort projects" })).toHaveTextContent("Least recently modified");
    expect(languageTrigger("Go")).toBeInTheDocument();
    expect(listedNames()).toEqual(["ação-relatórios", "payments-api"]);
  });

  it("moves to the search on / or ⌘F, but not while typing in another field", async () => {
    const { user } = await renderProjects();

    await user.keyboard("/");
    expect(search()).toHaveFocus();
    expect(search()).toHaveValue("");

    await user.type(search(), "pay/");
    expect(search()).toHaveValue("pay/");

    search().blur();
    await user.keyboard("{Meta>}f{/Meta}");
    expect(search()).toHaveFocus();

    await user.click(screen.getByRole("button", { name: "Clone repository" }));
    const url = screen.getByLabelText("Repository URL");
    await user.type(url, "acme/notifier");
    expect(url).toHaveValue("acme/notifier");
    expect(url).toHaveFocus();
    await user.keyboard("{Meta>}f{/Meta}");
    expect(url).toHaveFocus();
  });
});

describe("add local folder", () => {
  it("adds the picked folder to the list, in the order of the sort", async () => {
    vi.mocked(listProjects).mockResolvedValue([
      makeProject({ id: "a", name: "notifier", lastModifiedAt: daysAgo(2) }),
    ]);
    vi.mocked(open).mockResolvedValue("/work/ledger-worker");
    vi.mocked(addLocalProject).mockResolvedValue(
      makeProject({ id: "b", name: "ledger-worker", lastModifiedAt: daysAgo(1) }),
    );
    renderScreen();
    await screen.findByRole("button", { name: /notifier/ });

    await userEvent.click(screen.getByRole("button", { name: "Add local folder" }));

    expect(open).toHaveBeenCalledWith({ directory: true });
    expect(addLocalProject).toHaveBeenCalledWith("/work/ledger-worker");
    await screen.findByRole("button", { name: /ledger-worker/ });
    expect(listedNames()).toEqual(["ledger-worker", "notifier"]);
  });

  it("does nothing when the picker is dismissed", async () => {
    vi.mocked(open).mockResolvedValue(null);
    renderScreen();
    await screen.findByText("No projects yet");

    await userEvent.click(screen.getByRole("button", { name: "Add local folder" }));

    expect(addLocalProject).not.toHaveBeenCalled();
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });

  it("shows the API error next to the action", async () => {
    vi.mocked(open).mockResolvedValue("/tmp/plain-folder");
    vi.mocked(addLocalProject).mockRejectedValue(
      apiError("not_a_git_repository", "/tmp/plain-folder is not a git repository."),
    );
    renderScreen();
    await screen.findByText("No projects yet");

    await userEvent.click(screen.getByRole("button", { name: "Add local folder" }));

    expect(await screen.findByRole("alert")).toHaveTextContent(
      "/tmp/plain-folder is not a git repository.",
    );
    expect(screen.getByRole("button", { name: "Add local folder" })).toBeEnabled();
  });
});

describe("clone repository", () => {
  it("derives the destination from the picked folder and the URL, and keeps it editable", async () => {
    vi.mocked(open).mockResolvedValue("/work");
    const user = await openCloneBox();
    const destination = screen.getByLabelText("Clone into");

    await user.type(screen.getByLabelText("Repository URL"), "git@bitbucket.org:acme/ledger-worker.git");
    expect(destination).toHaveValue("");
    expect(screen.getByRole("button", { name: "Clone and add" })).toBeDisabled();

    await user.click(screen.getByRole("button", { name: "Choose folder" }));
    expect(destination).toHaveValue("/work/ledger-worker");

    await user.clear(screen.getByLabelText("Repository URL"));
    await user.type(screen.getByLabelText("Repository URL"), "https://example.com/acme/notifier");
    expect(destination).toHaveValue("/work/notifier");

    await user.clear(destination);
    await user.type(destination, "/elsewhere/custom");
    expect(destination).toHaveValue("/elsewhere/custom");
  });

  it("shows progress while cloning, then adds the project and closes the box", async () => {
    vi.mocked(open).mockResolvedValue("/work");
    const cloning = deferred<Project>();
    vi.mocked(cloneProject).mockReturnValue(cloning.promise);
    const user = await openCloneBox();

    await user.type(screen.getByLabelText("Repository URL"), "git@bitbucket.org:acme/ledger-worker.git");
    await user.click(screen.getByRole("button", { name: "Choose folder" }));
    await user.click(screen.getByRole("button", { name: "Clone and add" }));

    expect(cloneProject).toHaveBeenCalledWith(
      "git@bitbucket.org:acme/ledger-worker.git",
      "/work/ledger-worker",
    );
    expect(screen.getByRole("button", { name: "Cloning…" })).toBeDisabled();
    expect(screen.getByRole("status")).toHaveTextContent("Cloning the repository");
    expect(screen.getByRole("button", { name: "Cancel" })).toBeDisabled();
    expect(screen.getByLabelText("Repository URL")).toBeDisabled();

    cloning.resolve(makeProject({ id: "b", name: "ledger-worker", path: "/work/ledger-worker" }));

    expect(await screen.findByRole("button", { name: /ledger-worker/ })).toBeInTheDocument();
    expect(screen.queryByRole("form", { name: "Clone repository" })).not.toBeInTheDocument();
  });

  it("shows the API error inside the box and keeps what was typed", async () => {
    vi.mocked(open).mockResolvedValue("/work");
    vi.mocked(cloneProject).mockRejectedValue(
      apiError("destination_exists", "/work/ledger-worker already exists."),
    );
    const user = await openCloneBox();

    await user.type(screen.getByLabelText("Repository URL"), "git@bitbucket.org:acme/ledger-worker.git");
    await user.click(screen.getByRole("button", { name: "Choose folder" }));
    await user.click(screen.getByRole("button", { name: "Clone and add" }));

    const box = within(screen.getByRole("form", { name: "Clone repository" }));
    expect(await box.findByRole("alert")).toHaveTextContent("/work/ledger-worker already exists.");
    expect(box.getByLabelText("Clone into")).toHaveValue("/work/ledger-worker");
    expect(box.getByRole("button", { name: "Clone and add" })).toBeEnabled();
  });

  it("closes the box on cancel", async () => {
    const user = await openCloneBox();

    await user.click(screen.getByRole("button", { name: "Cancel" }));

    expect(screen.queryByRole("form", { name: "Clone repository" })).not.toBeInTheDocument();
    expect(cloneProject).not.toHaveBeenCalled();
  });
});
