use crate::models::PoolState;
use crate::storage::{PostgresStorageTrait, RedisCacheTrait};
use crate::sui_client::SuiClientTrait;
use axum::{
    extract::{
        State,
        ws::{Message as WsMessage, WebSocket, WebSocketUpgrade},
    },
    response::IntoResponse,
};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::broadcast;

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq)]
pub struct ClientMessage {
    pub action: String,
    pub pool_id: String,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq)]
pub struct ServerMessage {
    #[serde(rename = "type")]
    pub message_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pool_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<PoolState>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

impl ServerMessage {
    pub fn subscribed(pool_id: String) -> Self {
        Self {
            message_type: "subscribed".to_string(),
            pool_id: Some(pool_id),
            state: None,
            message: None,
        }
    }

    pub fn unsubscribed(pool_id: String) -> Self {
        Self {
            message_type: "unsubscribed".to_string(),
            pool_id: Some(pool_id),
            state: None,
            message: None,
        }
    }

    pub fn pool_update(state: PoolState) -> Self {
        Self {
            message_type: "pool_update".to_string(),
            pool_id: None,
            state: Some(state),
            message: None,
        }
    }

    pub fn error(message: String) -> Self {
        Self {
            message_type: "error".to_string(),
            pool_id: None,
            state: None,
            message: Some(message),
        }
    }
}

#[derive(Clone)]
pub struct ServerAppState {
    pub broadcast_tx: broadcast::Sender<PoolState>,
    pub pg_db: Arc<dyn PostgresStorageTrait>,
    pub redis_cache: Arc<dyn RedisCacheTrait>,
    pub topology_ready: Arc<std::sync::atomic::AtomicBool>,
    pub sui_client: Arc<dyn SuiClientTrait>,
}

impl ServerAppState {
    pub fn new(
        broadcast_tx: broadcast::Sender<PoolState>,
        pg_db: Arc<dyn PostgresStorageTrait>,
        redis_cache: Arc<dyn RedisCacheTrait>,
        topology_ready: Arc<std::sync::atomic::AtomicBool>,
        sui_client: Arc<dyn SuiClientTrait>,
    ) -> Self {
        Self {
            broadcast_tx,
            pg_db,
            redis_cache,
            topology_ready,
            sui_client,
        }
    }
}

pub async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<ServerAppState>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

async fn handle_socket(socket: WebSocket, state: ServerAppState) {
    let (mut sender, mut receiver) = socket.split();
    let mut broadcast_rx = state.broadcast_tx.subscribe();
    let mut subscriptions = HashSet::<String>::new();

    loop {
        tokio::select! {
            // 1. Read message from WebSocket client
            msg_opt = receiver.next() => {
                match msg_opt {
                    Some(Ok(WsMessage::Text(text))) => {
                        match serde_json::from_str::<ClientMessage>(&text) {
                            Ok(msg) => {
                                if msg.action == "subscribe" {
                                    subscriptions.insert(msg.pool_id.clone());
                                    let response = ServerMessage::subscribed(msg.pool_id);
                                    if let Ok(serialized) = serde_json::to_string(&response) {
                                        let _ = sender.send(WsMessage::Text(serialized.into())).await;
                                    }
                                } else if msg.action == "unsubscribe" {
                                    subscriptions.remove(&msg.pool_id);
                                    let response = ServerMessage::unsubscribed(msg.pool_id);
                                    if let Ok(serialized) = serde_json::to_string(&response) {
                                        let _ = sender.send(WsMessage::Text(serialized.into())).await;
                                    }
                                } else {
                                    let response = ServerMessage::error(format!("Unknown action: {}", msg.action));
                                    if let Ok(serialized) = serde_json::to_string(&response) {
                                        let _ = sender.send(WsMessage::Text(serialized.into())).await;
                                    }
                                }
                            }
                            Err(e) => {
                                let response = ServerMessage::error(format!("Invalid message format: {:?}", e));
                                if let Ok(serialized) = serde_json::to_string(&response) {
                                    let _ = sender.send(WsMessage::Text(serialized.into())).await;
                                }
                            }
                        }
                    }
                    Some(Ok(WsMessage::Close(_))) | None => {
                        break;
                    }
                    Some(Err(_)) => {
                        break;
                    }
                    _ => {}
                }
            }
            // 2. Read update from dynamic workers broadcast channel
            broadcast_res = broadcast_rx.recv() => {
                match broadcast_res {
                    Ok(pool_state) => {
                        if subscriptions.contains(&pool_state.pool_id) {
                            let response = ServerMessage::pool_update(pool_state);
                            if let Ok(serialized) = serde_json::to_string(&response)
                                && sender.send(WsMessage::Text(serialized.into())).await.is_err() {
                                    break; // connection closed
                                }
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        continue;
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        break;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Router, routing::get};
    use tokio::net::TcpListener;
    use tokio_tungstenite::{connect_async, tungstenite::Message as TungsteniteMessage};

    #[tokio::test]
    async fn test_ws_server_subscription_and_broadcast() {
        let (broadcast_tx, _) = broadcast::channel::<PoolState>(10);
        let pg_db = Arc::new(crate::storage::postgres::tests::InMemoryPostgresStorage::new());
        let redis_cache = Arc::new(crate::storage::redis::tests::InMemoryRedisCache::new());
        let app_state = ServerAppState::new(
            broadcast_tx.clone(),
            pg_db,
            redis_cache,
            Arc::new(std::sync::atomic::AtomicBool::new(true)),
            Arc::new(crate::sui_client::tests::MockSuiClient::new()),
        );

        let app = Router::new()
            .route("/ws", get(ws_handler))
            .with_state(app_state);

        // Bind server to an ephemeral port
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        // Spawn server
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        // Connect client
        let ws_url = format!("ws://{}/ws", addr);
        let (ws_stream, _) = connect_async(ws_url).await.unwrap();
        let (mut client_tx, mut client_rx) = ws_stream.split();

        // 1. Subscribe to pool
        let sub_payload = serde_json::json!({
            "action": "subscribe",
            "pool_id": "0x_pool_1"
        });
        client_tx
            .send(TungsteniteMessage::Text(sub_payload.to_string().into()))
            .await
            .unwrap();

        // Expect subscribed response
        let resp = client_rx.next().await.unwrap().unwrap();
        let text = resp.to_text().unwrap();
        let parsed: ServerMessage = serde_json::from_str(text).unwrap();
        assert_eq!(parsed, ServerMessage::subscribed("0x_pool_1".to_string()));

        // 2. Broadcast pool update from dynamic worker side
        let update = PoolState {
            pool_id: "0x_pool_1".to_string(),
            dex_name: "Cetus".to_string(),
            coin_type_a: "A".to_string(),
            coin_type_b: "B".to_string(),
            sqrt_price: 100,
            liquidity: 200,
            fee_rate: 300,
            is_paused: false,
        };
        broadcast_tx.send(update.clone()).unwrap();

        // Expect PoolUpdate response
        let resp = client_rx.next().await.unwrap().unwrap();
        let text = resp.to_text().unwrap();
        let parsed: ServerMessage = serde_json::from_str(text).unwrap();
        assert_eq!(parsed, ServerMessage::pool_update(update));

        // 3. Broadcast update for non-subscribed pool
        let other_update = PoolState {
            pool_id: "0x_pool_2".to_string(),
            dex_name: "Cetus".to_string(),
            coin_type_a: "A".to_string(),
            coin_type_b: "B".to_string(),
            sqrt_price: 999,
            liquidity: 999,
            fee_rate: 300,
            is_paused: false,
        };
        broadcast_tx.send(other_update).unwrap();

        // Unsubscribe from 0x_pool_1
        let unsub_payload = serde_json::json!({
            "action": "unsubscribe",
            "pool_id": "0x_pool_1"
        });
        client_tx
            .send(TungsteniteMessage::Text(unsub_payload.to_string().into()))
            .await
            .unwrap();

        let resp = client_rx.next().await.unwrap().unwrap();
        let text = resp.to_text().unwrap();
        let parsed: ServerMessage = serde_json::from_str(text).unwrap();
        assert_eq!(parsed, ServerMessage::unsubscribed("0x_pool_1".to_string()));
    }
}
