resource_model_macro::resource_model_file!("specs/self.yaml");

use std::net::SocketAddr;

use axum::Router;
use tower_http::cors::{Any, CorsLayer};
use tower_http::services::ServeDir;
use utoipa_scalar::{Scalar, Servable};

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

    let (api, openapi) = resource_api::router().split_for_parts();

    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    let app = Router::new()
        .merge(api)
        .merge(Scalar::with_url("/api/docs", openapi))
        .layer(cors)
        .fallback_service(ServeDir::new(&serve_dir))
        .with_state(pool);

    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    eprintln!("listening on http://localhost:{port}");
    eprintln!("  api docs: http://localhost:{port}/api/docs");
    axum::serve(listener, app).await?;

    Ok(())
}
