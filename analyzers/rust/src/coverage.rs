//! Coverage with cargo-llvm-cov, in its "external tests" mode: `cargo
//! llvm-cov show-env` gives the environment that instruments the build and
//! names the profile files, the test binaries run with it, and `cargo
//! llvm-cov report --lcov` merges what they wrote. The line counts of the
//! LCOV report become the protocol's blocks: one block per run of
//! consecutive lines with the same count.
//!
//! cargo-llvm-cov installs the llvm-tools component when it is missing; the
//! analyzer never lets it: it finds llvm-cov and llvm-profdata in the
//! toolchain's sysroot first and passes them (LLVM_COV, LLVM_PROFDATA), or
//! reports coverage as unavailable.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use crate::process;
use crate::protocol::CoverageBlock;

/// The tools coverage needs, found.
#[derive(Debug, Clone)]
pub struct Tools {
    pub llvm_cov: PathBuf,
    pub llvm_profdata: PathBuf,
}

/// Finds cargo-llvm-cov and the llvm tools of the toolchain used in `dir`,
/// or says why coverage cannot be measured.
pub fn tools(dir: &Path, deadline: Instant) -> Result<Tools, String> {
    let mut version = Command::new("cargo");
    version.args(["llvm-cov", "--version"]).current_dir(dir);
    match process::capture(version, deadline) {
        Ok((_, finished)) if finished.success() => {}
        _ => {
            return Err(
                "cargo-llvm-cov is not installed (cargo install cargo-llvm-cov)".to_string(),
            )
        }
    }
    if let (Some(llvm_cov), Some(llvm_profdata)) = (
        std::env::var_os("LLVM_COV"),
        std::env::var_os("LLVM_PROFDATA"),
    ) {
        return Ok(Tools {
            llvm_cov: llvm_cov.into(),
            llvm_profdata: llvm_profdata.into(),
        });
    }
    let rustc = |args: &[&str]| -> Option<String> {
        let mut command = Command::new("rustc");
        command.args(args).current_dir(dir);
        match process::capture(command, deadline) {
            Ok((stdout, finished)) if finished.success() => Some(stdout),
            _ => None,
        }
    };
    let sysroot = rustc(&["--print", "sysroot"]).map(|text| text.trim().to_string());
    let host = rustc(&["-vV"]).and_then(|text| {
        text.lines().find_map(|line| {
            line.strip_prefix("host: ")
                .map(|host| host.trim().to_string())
        })
    });
    let (Some(sysroot), Some(host)) = (sysroot, host) else {
        return Err("rustc could not say where its tools are".to_string());
    };
    let bin = Path::new(&sysroot)
        .join("lib/rustlib")
        .join(host)
        .join("bin");
    let tools = Tools {
        llvm_cov: bin.join("llvm-cov"),
        llvm_profdata: bin.join("llvm-profdata"),
    };
    if !tools.llvm_cov.is_file() || !tools.llvm_profdata.is_file() {
        return Err(
            "the toolchain has no llvm-tools component (rustup component add llvm-tools)"
                .to_string(),
        );
    }
    Ok(tools)
}

fn base_command(dir: &Path, target_dir: &Path, tools: &Tools) -> Command {
    let mut command = Command::new("cargo");
    command
        .current_dir(dir)
        .env("CARGO_TARGET_DIR", target_dir)
        .env("CARGO_TERM_COLOR", "never")
        .env("LLVM_COV", &tools.llvm_cov)
        .env("LLVM_PROFDATA", &tools.llvm_profdata);
    command
}

/// The environment that instruments a build of the workspace at `dir`.
pub fn show_env(
    dir: &Path,
    target_dir: &Path,
    tools: &Tools,
    deadline: Instant,
) -> Result<Vec<(String, String)>, String> {
    let mut command = base_command(dir, target_dir, tools);
    command.args(["llvm-cov", "show-env"]);
    let (stdout, finished) =
        process::capture(command, deadline).map_err(|error| error.to_string())?;
    if !finished.success() {
        return Err(format!(
            "cargo llvm-cov show-env failed: {}",
            process::first_line(&finished.stderr)
        ));
    }
    let mut vars = parse_env(&stdout);
    vars.push(("LLVM_COV".to_string(), tools.llvm_cov.display().to_string()));
    vars.push((
        "LLVM_PROFDATA".to_string(),
        tools.llvm_profdata.display().to_string(),
    ));
    Ok(vars)
}

/// `KEY='value'` lines, as show-env prints them (single quotes, `'\''`
/// for a quote inside).
fn parse_env(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter_map(|line| {
            let (key, value) = line.trim().split_once('=')?;
            if key.is_empty() || !key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                return None;
            }
            let value = match value
                .strip_prefix('\'')
                .and_then(|rest| rest.strip_suffix('\''))
            {
                Some(inner) => inner.replace("'\\''", "'"),
                None => value.to_string(),
            };
            Some((key.to_string(), value))
        })
        .collect()
}

/// Removes the profiles an earlier run left in `target_dir`, where
/// show-env names them: `report` merges every `*.profraw` there, and falls
/// back on the last merged `*.profdata` when there is none, so a shared
/// target folder would add other runs' counts to this one's.
pub fn clean_profiles(target_dir: &Path) -> std::io::Result<()> {
    let entries = match std::fs::read_dir(target_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if entry.file_type()?.is_file()
            && (name.ends_with(".profraw")
                || name.ends_with(".profdata")
                || name.ends_with("-profraw-list"))
        {
            std::fs::remove_file(entry.path())?;
        }
    }
    Ok(())
}

/// The LCOV report of what the instrumented binaries wrote.
pub fn report(
    dir: &Path,
    target_dir: &Path,
    tools: &Tools,
    env: &[(String, String)],
    output: &Path,
    deadline: Instant,
) -> Result<String, String> {
    let mut command = base_command(dir, target_dir, tools);
    command.envs(env.iter().map(|(key, value)| (key, value)));
    command
        .args(["llvm-cov", "report", "--lcov", "--output-path"])
        .arg(output);
    let (_, finished) = process::capture(command, deadline).map_err(|error| error.to_string())?;
    if !finished.success() {
        return Err(format!(
            "cargo llvm-cov report failed: {}",
            process::first_line(&finished.stderr)
        ));
    }
    std::fs::read_to_string(output).map_err(|error| error.to_string())
}

/// The line counts of an LCOV report, per absolute file.
pub fn parse_lcov(text: &str) -> BTreeMap<PathBuf, BTreeMap<u32, u64>> {
    let mut files: BTreeMap<PathBuf, BTreeMap<u32, u64>> = BTreeMap::new();
    let mut current: Option<PathBuf> = None;
    for line in text.lines() {
        if let Some(file) = line.strip_prefix("SF:") {
            current = Some(PathBuf::from(file.trim()));
        } else if line == "end_of_record" {
            current = None;
        } else if let (Some(file), Some(data)) = (&current, line.strip_prefix("DA:")) {
            let mut parts = data.split(',');
            let line_number = parts
                .next()
                .and_then(|part| part.trim().parse::<u32>().ok());
            let count = parts
                .next()
                .and_then(|part| part.trim().parse::<u64>().ok());
            if let (Some(line_number), Some(count)) = (line_number, count) {
                if line_number > 0 {
                    let entry = files
                        .entry(file.clone())
                        .or_default()
                        .entry(line_number)
                        .or_insert(0);
                    *entry += count;
                }
            }
        }
    }
    files
}

/// Blocks of one file's line counts: consecutive lines with the same
/// count are one block, each line a statement.
pub fn blocks(file: &str, lines: &BTreeMap<u32, u64>) -> Vec<CoverageBlock> {
    let mut out: Vec<CoverageBlock> = Vec::new();
    for (line, count) in lines {
        if let Some(last) = out.last_mut() {
            if last.end_line + 1 == *line && last.count == *count {
                last.end_line = *line;
                last.statements += 1;
                continue;
            }
        }
        out.push(CoverageBlock {
            file: file.to_string(),
            start_line: *line,
            end_line: *line,
            statements: 1,
            count: *count,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn show_env_output_is_read_as_variables() {
        let vars = parse_env("LLVM_PROFILE_FILE='/t/tree-%p-%11m.profraw'\nRUSTC_WRAPPER=/bin/x\nQUOTED='it'\\''s'\ninfo: something\n");
        assert_eq!(
            vars,
            [
                (
                    "LLVM_PROFILE_FILE".to_string(),
                    "/t/tree-%p-%11m.profraw".to_string()
                ),
                ("RUSTC_WRAPPER".to_string(), "/bin/x".to_string()),
                ("QUOTED".to_string(), "it's".to_string()),
            ]
        );
    }

    #[test]
    fn profiles_of_earlier_runs_are_removed_and_nothing_else() {
        let folder = crate::testing::TempDir::new();
        let target = folder.0.join("target");
        std::fs::create_dir_all(target.join("debug")).unwrap();
        for name in [
            "tree-1-123.profraw",
            "tree.profdata",
            "tree-profraw-list",
            "CACHEDIR.TAG",
            "debug/kept.profraw",
        ] {
            std::fs::write(target.join(name), "x").unwrap();
        }
        clean_profiles(&target).unwrap();
        let mut left: Vec<String> = std::fs::read_dir(&target)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        assert_eq!(left, ["CACHEDIR.TAG", "debug"]);
        assert!(target.join("debug/kept.profraw").exists());
        clean_profiles(&folder.0.join("missing")).unwrap();
    }

    #[test]
    fn lcov_lines_become_blocks() {
        let report = "SF:/t/src/lib.rs\nFN:3,x\nDA:3,2\nDA:4,2\nDA:5,0\nDA:7,1\nDA:3,1\nend_of_record\nSF:/t/src/b.rs\nDA:1,0\nend_of_record\n";
        let files = parse_lcov(report);
        let lib = &files[Path::new("/t/src/lib.rs")];
        assert_eq!(lib[&3], 3);
        let blocks = blocks("src/lib.rs", lib);
        let shape: Vec<(u32, u32, u32, u64)> = blocks
            .iter()
            .map(|block| {
                (
                    block.start_line,
                    block.end_line,
                    block.statements,
                    block.count,
                )
            })
            .collect();
        assert_eq!(
            shape,
            [(3, 3, 1, 3), (4, 4, 1, 2), (5, 5, 1, 0), (7, 7, 1, 1)]
        );
        assert_eq!(files.len(), 2);
    }
}
