use crate::api::websocket::ServerAppState;
use crate::models::Token;
use axum::{Json, extract::State, http::StatusCode, response::IntoResponse};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Serialize, Deserialize)]
pub struct HealthResponse {
    pub status: String,
    pub service: String,
}

pub async fn handle_health() -> impl IntoResponse {
    (
        StatusCode::OK,
        Json(HealthResponse {
            status: "ok".to_string(),
            service: "devilray-sui".to_string(),
        }),
    )
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ReadinessResponse {
    pub ready: bool,
    pub topology_ready: bool,
}

pub async fn handle_readyz(State(state): State<ServerAppState>) -> impl IntoResponse {
    let topology_ready = state
        .topology_ready
        .load(std::sync::atomic::Ordering::SeqCst);
    let pool_count = state.pg_db.list_pools().await.unwrap_or_default().len();
    let ready = topology_ready && pool_count > 0;
    let status = if ready {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    (
        status,
        Json(ReadinessResponse {
            ready,
            topology_ready,
        }),
    )
}

pub async fn load_all_tokens(state: &ServerAppState) -> Vec<Token> {
    if let Ok(Some(tokens)) = state.redis_cache.get_all_tokens().await {
        return tokens;
    }
    match state.pg_db.list_tokens().await {
        Ok(tokens) => {
            let _ = state.redis_cache.set_all_tokens(&tokens).await;
            tokens
        }
        Err(_) => vec![],
    }
}

pub async fn load_tokens_map(state: &ServerAppState) -> HashMap<String, u8> {
    load_all_tokens(state)
        .await
        .into_iter()
        .map(|t| (t.address, t.decimals))
        .collect()
}

pub async fn handle_list_tokens(State(state): State<ServerAppState>) -> impl IntoResponse {
    let tokens = load_all_tokens(&state).await;
    (StatusCode::OK, Json(tokens))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Token;
    use crate::storage::PostgresStorageTrait;
    use crate::storage::postgres::tests::InMemoryPostgresStorage;
    use crate::storage::redis::tests::InMemoryRedisCache;
    use axum::{Router, routing::get};
    use std::sync::Arc;
    use tokio::net::TcpListener;
    use tokio::sync::broadcast;

    #[tokio::test]
    async fn test_health_endpoint() {
        let (broadcast_tx, _) = broadcast::channel(10);
        let pg_db = Arc::new(InMemoryPostgresStorage::new());
        let redis_cache = Arc::new(InMemoryRedisCache::new());
        let app_state = ServerAppState::new(
            broadcast_tx,
            pg_db,
            redis_cache,
            Arc::new(std::sync::atomic::AtomicBool::new(true)),
            Arc::new(crate::sui_client::tests::MockSuiClient::new()),
        );
        let app = Router::new()
            .route("/health", get(handle_health))
            .with_state(app_state);

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let client = reqwest::Client::new();
        let res = client
            .get(format!("http://{}/health", addr))
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body: HealthResponse = res.json().await.unwrap();
        assert_eq!(body.status, "ok");
        assert_eq!(body.service, "devilray-sui");
    }

    #[tokio::test]
    async fn test_tokens_endpoint() {
        let (broadcast_tx, _) = broadcast::channel(10);
        let pg_db = Arc::new(InMemoryPostgresStorage::new());
        let redis_cache = Arc::new(InMemoryRedisCache::new());

        pg_db
            .insert_token(&Token {
                address: "SUI".to_string(),
                symbol: "SUI".to_string(),
                name: "Sui".to_string(),
                decimals: 9,
            })
            .await
            .unwrap();
        pg_db
            .insert_token(&Token {
                address: "USDC".to_string(),
                symbol: "USDC".to_string(),
                name: "USDC".to_string(),
                decimals: 6,
            })
            .await
            .unwrap();

        let app_state = ServerAppState::new(
            broadcast_tx,
            pg_db,
            redis_cache,
            Arc::new(std::sync::atomic::AtomicBool::new(true)),
            Arc::new(crate::sui_client::tests::MockSuiClient::new()),
        );
        let app = Router::new()
            .route("/api/v1/tokens", get(handle_list_tokens))
            .with_state(app_state);

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let client = reqwest::Client::new();
        let url = format!("http://{}/api/v1/tokens", addr);

        let res = client.get(&url).send().await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let tokens: Vec<Token> = res.json().await.unwrap();
        assert_eq!(tokens.len(), 2);

        let res2 = client.get(&url).send().await.unwrap();
        let tokens2: Vec<Token> = res2.json().await.unwrap();
        assert_eq!(tokens2.len(), 2);
    }
}
