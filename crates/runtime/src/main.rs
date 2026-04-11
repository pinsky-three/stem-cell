resource_model_macro::resource_model_file!("specs/self.yaml");

use std::net::SocketAddr;

use axum::Router;
use tower_http::services::ServeDir;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let db_url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");
    let pool = sqlx::PgPool::connect(&db_url).await?;

    migrate(&pool).await?;
    eprintln!("migrations applied");

    let port: u16 = std::env::var("PORT")
        .unwrap_or_else(|_| "4200".into())
        .parse()
        .expect("PORT must be a valid u16");

    let serve_dir = std::env::var("SERVE_DIR").unwrap_or_else(|_| "public".into());

    let app = Router::new().fallback_service(ServeDir::new(&serve_dir));

    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    eprintln!("listening on http://localhost:{port}");

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
