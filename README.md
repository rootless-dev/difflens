# DiffLens

A desktop app for reviewing code that an AI agent wrote.

An agent can change dozens of files in minutes, and reading that change as a flat
diff is slow and easy to get wrong. DiffLens reads the branch the way a reviewer
needs it:
- which functions changed and how they connect;
- whether the tests cover the new lines;
- where complexity and duplication went up;
- what a model thinks deserves a second look.

Your comments then go back to the pull request.

It runs locally on macOS, against the git clones you already have. Models are
called only when you click; nothing runs on its own.

## What it does

- **Projects.** It lists the git repositories in the folders you choose, with
  search, a language filter and sorting by recent activity. Each project shows
  its branches with how far they are ahead and behind the base.
- **Reviews.** A review compares a branch with the base its pull request
  targets, or with `origin/<base>` when there is no pull request.
- **Pull requests.** It works with Bitbucket Cloud and GitHub. Comments stay
  drafts until you publish them all at once. You can approve or request changes
  from the app.
- **Models.** It works with the Claude Code CLI, or with API keys for Anthropic,
  OpenAI, Google or OpenRouter. You choose the model per task in Preferences.
- **Jira.** It finds the card from the branch name and checks the change
  against its acceptance criteria.
- **Languages.** It supports Go, Python, JavaScript/TypeScript and Rust. Each
  language has an analyzer written in that language. An analyzer's environment
  is set up only when a project in that language first shows up.
- **Interface.** It is available in English and Brazilian Portuguese. Tokens
  and API keys are kept in the macOS Keychain.

## Install

Download the disk image for your Mac from the
[latest release](https://github.com/rootless-dev/difflens/releases/latest):

- **Apple Silicon** (M1 and later): `DiffLens_<version>_aarch64.dmg`
- **Intel**: `DiffLens_<version>_x64.dmg`

Open it and drag **DiffLens** to **Applications**.

### Opening it the first time

DiffLens is not signed with an Apple Developer ID and is not notarized, so macOS
blocks it the first time you open it. Use either of these:

1. Open DiffLens once and dismiss the warning. Then go to **System Settings >
   Privacy & Security**, scroll to **Security**, and click **Open Anyway** next
   to the message about DiffLens.
2. Or remove the quarantine flag in Terminal. Use this one too if macOS says
   the app "is damaged and can't be opened":

   ```sh
   xattr -dr com.apple.quarantine /Applications/DiffLens.app
   ```

There is no automatic update yet. To get a new version, download it from the
releases page.

## Development

### Requirements

- macOS
- [Rust](https://rustup.rs) (stable)
- Node.js 26 and pnpm 10
- Go 1.26, to build the Go analyzer that ships inside the app
- Python 3.14 and [uv](https://docs.astral.sh/uv/). These are needed only to
  work on the Python analyzer.

### Run

```sh
pnpm install
pnpm analyzers:build   # builds the Go analyzer into src-tauri/binaries/
pnpm tauri dev
```

The app keeps its data in `~/Library/Application Support/dev.difflens.app/`.

### Test

```sh
pnpm test                                   # frontend (vitest)
(cd src-tauri && cargo test)                # Rust core
(cd analyzers/go && go test ./...)          # Go analyzer
(cd analyzers/jsts && npm ci && npm test)   # JS/TS analyzer
(cd analyzers/rust && cargo test)           # Rust analyzer
(cd analyzers/python && python -m unittest discover -s tests -t .)
```

CI runs these suites on every pull request. It also runs the linters, checks
for at least 70% line coverage per component, audits dependencies and builds
the `.dmg` files. See [`.github/workflows`](.github/workflows).

### Layout

```text
src/                 React frontend
src-tauri/           Rust core (git, reviews, tests, quality, models, pull requests)
analyzers/protocol/  JSON schemas shared by the analyzers and the core
analyzers/go/        Go analyzer (built as a sidecar binary)
analyzers/python/    Python analyzer
analyzers/jsts/      JavaScript/TypeScript analyzer (Node)
analyzers/rust/      Rust analyzer
design/              HTML mockups
```

## License

MIT. See [LICENSE](LICENSE).
