//! Language of a single file guessed from its name, for syntax highlighting.
//! Project detection (which languages a repository uses, and which of them
//! get analysis) lives in `languages.rs`; this table only names files.

/// Whole file names, compared exactly, that say more than their extension.
const FILE_NAMES: &[(&str, &str)] = &[
    ("Dockerfile", "dockerfile"),
    ("Containerfile", "dockerfile"),
    ("Makefile", "makefile"),
    ("makefile", "makefile"),
    ("GNUmakefile", "makefile"),
    ("Gemfile", "ruby"),
    ("Rakefile", "ruby"),
    ("Jenkinsfile", "groovy"),
    ("CMakeLists.txt", "cmake"),
    (".bashrc", "shell"),
    (".bash_profile", "shell"),
    (".profile", "shell"),
    (".zshrc", "shell"),
];

/// Extensions in lowercase, without the dot.
const EXTENSIONS: &[(&str, &str)] = &[
    ("go", "go"),
    ("ts", "typescript"),
    ("mts", "typescript"),
    ("cts", "typescript"),
    ("tsx", "tsx"),
    ("js", "javascript"),
    ("mjs", "javascript"),
    ("cjs", "javascript"),
    ("jsx", "jsx"),
    ("rs", "rust"),
    ("py", "python"),
    ("pyi", "python"),
    ("java", "java"),
    ("kt", "kotlin"),
    ("kts", "kotlin"),
    ("scala", "scala"),
    ("groovy", "groovy"),
    ("gradle", "groovy"),
    ("rb", "ruby"),
    ("php", "php"),
    ("c", "c"),
    ("h", "c"),
    ("cc", "cpp"),
    ("cpp", "cpp"),
    ("cxx", "cpp"),
    ("hh", "cpp"),
    ("hpp", "cpp"),
    ("hxx", "cpp"),
    ("cs", "csharp"),
    ("swift", "swift"),
    ("m", "objective-c"),
    ("dart", "dart"),
    ("lua", "lua"),
    ("ex", "elixir"),
    ("exs", "elixir"),
    ("erl", "erlang"),
    ("hs", "haskell"),
    ("clj", "clojure"),
    ("r", "r"),
    ("pl", "perl"),
    ("sql", "sql"),
    ("graphql", "graphql"),
    ("gql", "graphql"),
    ("proto", "proto"),
    ("yaml", "yaml"),
    ("yml", "yaml"),
    ("json", "json"),
    ("jsonc", "json"),
    ("toml", "toml"),
    ("xml", "xml"),
    ("ini", "ini"),
    ("md", "markdown"),
    ("markdown", "markdown"),
    ("html", "html"),
    ("htm", "html"),
    ("vue", "vue"),
    ("svelte", "svelte"),
    ("css", "css"),
    ("scss", "scss"),
    ("less", "less"),
    ("sh", "shell"),
    ("bash", "shell"),
    ("zsh", "shell"),
    ("ps1", "powershell"),
    ("dockerfile", "dockerfile"),
    ("mk", "makefile"),
    ("tf", "hcl"),
    ("hcl", "hcl"),
];

/// Language id of the file at `path` (a repository path with `/`
/// separators), or `None` when its name says nothing known.
pub fn of_path(path: &str) -> Option<&'static str> {
    let name = path.rsplit('/').next().unwrap_or(path);
    if let Some((_, language)) = FILE_NAMES.iter().find(|(known, _)| *known == name) {
        return Some(language);
    }
    // Dockerfile.dev, Dockerfile.prod and the like.
    if name.starts_with("Dockerfile.") {
        return Some("dockerfile");
    }
    let (stem, extension) = name.rsplit_once('.')?;
    // A dotfile such as `.env` has no extension, only a name.
    if stem.is_empty() {
        return None;
    }
    let extension = extension.to_ascii_lowercase();
    EXTENSIONS
        .iter()
        .find(|(known, _)| *known == extension)
        .map(|(_, language)| *language)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extensions_name_the_language_whatever_their_case() {
        let cases = [
            ("main.go", Some("go")),
            ("src/lib/api.ts", Some("typescript")),
            ("src/App.tsx", Some("tsx")),
            ("index.mjs", Some("javascript")),
            ("src-tauri/src/git.rs", Some("rust")),
            ("tool.py", Some("python")),
            ("Main.java", Some("java")),
            ("build.gradle.kts", Some("kotlin")),
            ("app.rb", Some("ruby")),
            ("index.php", Some("php")),
            ("x.c", Some("c")),
            ("x.hpp", Some("cpp")),
            ("Program.cs", Some("csharp")),
            ("View.swift", Some("swift")),
            ("db/001 init.sql", Some("sql")),
            (".github/workflows/ci.YML", Some("yaml")),
            ("package.json", Some("json")),
            ("Cargo.toml", Some("toml")),
            ("README.md", Some("markdown")),
            ("index.html", Some("html")),
            ("style.css", Some("css")),
            ("scripts/run.sh", Some("shell")),
            ("api/v1/service.proto", Some("proto")),
        ];
        for (path, expected) in cases {
            assert_eq!(of_path(path), expected, "{path}");
        }
    }

    #[test]
    fn some_files_are_known_by_their_whole_name() {
        assert_eq!(of_path("Dockerfile"), Some("dockerfile"));
        assert_eq!(of_path("deploy/Dockerfile.prod"), Some("dockerfile"));
        assert_eq!(of_path("build/Makefile"), Some("makefile"));
        assert_eq!(of_path("rules.mk"), Some("makefile"));
        assert_eq!(of_path("Gemfile"), Some("ruby"));
    }

    #[test]
    fn unknown_names_have_no_language() {
        assert_eq!(of_path("LICENSE"), None);
        assert_eq!(of_path(".env"), None);
        assert_eq!(of_path(".gitignore"), None);
        assert_eq!(of_path("image.png"), None);
        assert_eq!(of_path("dir.go/notes"), None);
        assert_eq!(of_path("trailing."), None);
        assert_eq!(of_path(""), None);
    }
}
