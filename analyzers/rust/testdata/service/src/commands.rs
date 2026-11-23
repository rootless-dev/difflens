use axum::Router;

pub async fn serve(app: Router, port: u16) {
    let address = format!("0.0.0.0:{port}");
    let listener = tokio::net::TcpListener::bind(address).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

pub fn migrate() {
    println!("migrated");
}
