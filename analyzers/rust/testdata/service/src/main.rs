mod commands;
mod legacy;
mod routes;

use axum::{middleware, routing::get, Router};
use clap::{Parser, Subcommand};

#[derive(Parser)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    Serve {
        port: u16,
    },
    #[command(name = "migrate-db")]
    Migrate,
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    match cli.command {
        Command::Serve { port } => commands::serve(app(), port).await,
        Command::Migrate => commands::migrate(),
    }
}

pub fn app() -> Router {
    Router::new()
        .route("/health", get(health))
        .nest("/api", routes::router())
        .layer(middleware::from_fn(auth))
}

async fn health() -> &'static str {
    "ok"
}

async fn auth(request: axum::extract::Request, next: middleware::Next) -> axum::response::Response {
    next.run(request).await
}
