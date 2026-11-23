//! The old API, still on actix-web.

use actix_web::{get, web, App, HttpResponse};

#[get("/legacy/{id}")]
async fn show(path: web::Path<u32>) -> HttpResponse {
    HttpResponse::Ok().body(path.to_string())
}

async fn ping() -> HttpResponse {
    HttpResponse::Ok().finish()
}

pub fn mount() {
    let _app = App::new().service(web::scope("/v1").service(show).route("/ping", web::get().to(ping)));
}
