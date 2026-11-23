//! The DiffLens analyzer for Rust source trees: one command per run, one
//! JSON document on stdout, logs on stderr (see README.md and `cli`).

mod analyze;
mod cache;
mod checks;
mod cli;
mod complexity;
mod coverage;
mod dupes;
mod engine;
mod entrypoints;
mod index;
mod log;
mod manifest;
mod mutation;
mod process;
mod protocol;
mod sandbox;
mod source;
#[cfg(test)]
mod testing;
mod testrun;
mod walk;
mod workspace;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = cli::run(
        &args,
        &mut std::io::stdout().lock(),
        &mut std::io::stderr().lock(),
    );
    std::process::exit(code);
}
