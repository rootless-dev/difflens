use axum::{routing::get, Router};

pub fn router() -> Router {
    Router::new().route("/users", get(list_users).post(create_user))
}

pub async fn list_users() -> &'static str {
    "[]"
}

pub async fn create_user() -> &'static str {
    "{}"
}
