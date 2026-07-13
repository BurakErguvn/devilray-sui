use anyhow::{Result, anyhow};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use crate::transaction_builder::{ObjectMeta, OwnerKind, ResolvedCoin};

#[async_trait]
pub trait SuiClientTrait: Send + Sync {
    async fn get_object(&self, object_id: &str) -> Result<Value>;
    async fn query_graphql(&self, query: &str) -> Result<Value>;
    async fn query_graphql_with_variables(&self, query: &str, variables: Value) -> Result<Value>;
    async fn get_reference_gas_price(&self) -> Result<u64>;
    async fn get_coin_metadata(&self, coin_type: &str) -> Result<Value>;
    /// Lists dynamic fields on a parent object (`suix_getDynamicFields`).
    async fn get_dynamic_fields(
        &self,
        parent_id: &str,
        cursor: Option<&str>,
        limit: Option<u32>,
    ) -> Result<Value>;
    /// Fetches a single dynamic field child object (`suix_getDynamicFieldObject`).
    async fn get_dynamic_field_object(&self, parent_id: &str, name: &Value) -> Result<Value>;
    /// Returns normalized Move modules for a package (`sui_getNormalizedMoveModulesByPackage`).
    async fn get_normalized_move_modules(&self, package_id: &str) -> Result<Value>;
    /// Typed object metadata (version/digest/owner) for transaction inputs.
    async fn get_object_meta(&self, object_id: &str) -> Result<ObjectMeta>;
    /// Paginated owned coins (`suix_getCoins`).
    async fn get_coins(
        &self,
        owner: &str,
        coin_type: Option<&str>,
        cursor: Option<&str>,
        limit: Option<u32>,
    ) -> Result<Vec<ResolvedCoin>>;
    /// Execute a signed transaction block (`sui_executeTransactionBlock`).
    async fn execute_transaction_block(
        &self,
        tx_bytes_base64: &str,
        signatures: &[String],
    ) -> Result<Value>;
}

#[derive(Clone)]
pub struct SuiClient {
    rpc_urls: Vec<String>,
    graphql_urls: Vec<String>,
    http_client: reqwest::Client,
}

impl SuiClient {
    pub fn new(rpc_url: String, graphql_url: String) -> Self {
        let rpc_urls: Vec<String> = rpc_url
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        let graphql_urls: Vec<String> = graphql_url
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        Self {
            rpc_urls,
            graphql_urls,
            http_client: reqwest::Client::new(),
        }
    }

    async fn post_rpc(&self, payload: Value) -> Result<Value> {
        if self.rpc_urls.is_empty() {
            return Err(anyhow!("No Sui RPC URLs configured"));
        }

        let mut last_error = None;
        for (index, rpc_url) in self.rpc_urls.iter().enumerate() {
            let request = self
                .http_client
                .post(rpc_url)
                .timeout(std::time::Duration::from_secs(5))
                .json(&payload);

            match request.send().await {
                Ok(response) => {
                    if response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
                        let retry_after = response
                            .headers()
                            .get(reqwest::header::RETRY_AFTER)
                            .and_then(|h| h.to_str().ok())
                            .and_then(|s| s.parse::<u64>().ok())
                            .unwrap_or(500);
                        tracing::warn!(
                            "RPC endpoint {} rate limited (429). Retrying next node after {}ms.",
                            rpc_url,
                            retry_after
                        );
                        last_error = Some(anyhow!("RPC endpoint {} rate limited (429)", rpc_url));
                        tokio::time::sleep(tokio::time::Duration::from_millis(retry_after)).await;
                        continue;
                    }

                    if !response.status().is_success() {
                        let err_msg = format!(
                            "RPC endpoint {} failed with status: {}",
                            rpc_url,
                            response.status()
                        );
                        last_error = Some(anyhow!(err_msg));
                        continue;
                    }

                    match response.json::<Value>().await {
                        Ok(body) => {
                            if let Some(err) = body.get("error") {
                                let err_msg =
                                    format!("RPC endpoint {} returned error: {:?}", rpc_url, err);
                                last_error = Some(anyhow!(err_msg));
                                continue;
                            }

                            if let Some(result) = body.get("result") {
                                return Ok(result.clone());
                            } else {
                                last_error = Some(anyhow!(
                                    "RPC endpoint {} response missing 'result' field",
                                    rpc_url
                                ));
                                continue;
                            }
                        }
                        Err(e) => {
                            last_error = Some(anyhow!(
                                "Failed to parse JSON response from RPC endpoint {}: {:?}",
                                rpc_url,
                                e
                            ));
                            continue;
                        }
                    }
                }
                Err(e) => {
                    last_error = Some(anyhow!(
                        "Connection failed to RPC endpoint {}: {:?}",
                        rpc_url,
                        e
                    ));
                    if index + 1 < self.rpc_urls.len() {
                        tokio::time::sleep(tokio::time::Duration::from_millis(
                            100 * (index + 1) as u64,
                        ))
                        .await;
                    }
                }
            }
        }

        Err(last_error.unwrap_or_else(|| anyhow!("All RPC failover attempts failed")))
    }
}

#[async_trait]
impl SuiClientTrait for SuiClient {
    async fn get_object(&self, object_id: &str) -> Result<Value> {
        let payload = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "sui_getObject",
            "params": [
                object_id,
                {
                    "showContent": true,
                    "showType": true,
                    "showOwner": false,
                    "showPreviousTransaction": false,
                    "showStorageRebate": false,
                    "showDisplay": false
                }
            ]
        });

        self.post_rpc(payload).await
    }

    async fn query_graphql(&self, query: &str) -> Result<Value> {
        self.query_graphql_with_variables(query, json!({})).await
    }

    async fn query_graphql_with_variables(&self, query: &str, variables: Value) -> Result<Value> {
        let payload = json!({
            "query": query,
            "variables": variables
        });

        if self.graphql_urls.is_empty() {
            return Err(anyhow!("No Sui GraphQL URLs configured"));
        }

        let mut last_error = None;
        for (index, graphql_url) in self.graphql_urls.iter().enumerate() {
            let request = self
                .http_client
                .post(graphql_url)
                .timeout(std::time::Duration::from_secs(5))
                .json(&payload);

            match request.send().await {
                Ok(response) => {
                    if response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
                        let retry_after = response
                            .headers()
                            .get(reqwest::header::RETRY_AFTER)
                            .and_then(|h| h.to_str().ok())
                            .and_then(|s| s.parse::<u64>().ok())
                            .unwrap_or(500);
                        tracing::warn!(
                            "GraphQL endpoint {} rate limited (429). Retrying next node after {}ms.",
                            graphql_url,
                            retry_after
                        );
                        last_error = Some(anyhow!(
                            "GraphQL endpoint {} rate limited (429)",
                            graphql_url
                        ));
                        tokio::time::sleep(tokio::time::Duration::from_millis(retry_after)).await;
                        continue;
                    }

                    if !response.status().is_success() {
                        let err_msg = format!(
                            "GraphQL endpoint {} failed with status: {}",
                            graphql_url,
                            response.status()
                        );
                        last_error = Some(anyhow!(err_msg));
                        continue;
                    }

                    match response.json::<Value>().await {
                        Ok(body) => {
                            if let Some(errors) = body.get("errors") {
                                let err_msg = format!(
                                    "GraphQL endpoint {} returned errors: {:?}",
                                    graphql_url, errors
                                );
                                last_error = Some(anyhow!(err_msg));
                                continue;
                            }

                            if let Some(data) = body.get("data") {
                                return Ok(data.clone());
                            } else {
                                last_error = Some(anyhow!(
                                    "GraphQL endpoint {} response missing 'data' field",
                                    graphql_url
                                ));
                                continue;
                            }
                        }
                        Err(e) => {
                            last_error = Some(anyhow!(
                                "Failed to parse JSON response from GraphQL endpoint {}: {:?}",
                                graphql_url,
                                e
                            ));
                            continue;
                        }
                    }
                }
                Err(e) => {
                    last_error = Some(anyhow!(
                        "Connection failed to GraphQL endpoint {}: {:?}",
                        graphql_url,
                        e
                    ));
                    if index + 1 < self.graphql_urls.len() {
                        tokio::time::sleep(tokio::time::Duration::from_millis(
                            100 * (index + 1) as u64,
                        ))
                        .await;
                    }
                }
            }
        }

        Err(last_error.unwrap_or_else(|| anyhow!("All GraphQL failover attempts failed")))
    }

    async fn get_reference_gas_price(&self) -> Result<u64> {
        let payload = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "suix_getReferenceGasPrice",
            "params": []
        });

        let result = self.post_rpc(payload).await?;
        let price_str = result
            .as_str()
            .ok_or_else(|| anyhow!("Reference gas price result is not a string"))?;
        let price = price_str.parse::<u64>()?;
        Ok(price)
    }

    async fn get_coin_metadata(&self, coin_type: &str) -> Result<Value> {
        let payload = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "suix_getCoinMetadata",
            "params": [coin_type]
        });

        self.post_rpc(payload).await
    }

    async fn get_dynamic_fields(
        &self,
        parent_id: &str,
        cursor: Option<&str>,
        limit: Option<u32>,
    ) -> Result<Value> {
        let mut params = json!([parent_id, cursor, limit.unwrap_or(50)]);
        // Sui expects null cursor when not paginating
        if cursor.is_none() {
            params = json!([parent_id, null, limit.unwrap_or(50)]);
        }
        let payload = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "suix_getDynamicFields",
            "params": params
        });
        self.post_rpc(payload).await
    }

    async fn get_dynamic_field_object(&self, parent_id: &str, name: &Value) -> Result<Value> {
        let payload = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "suix_getDynamicFieldObject",
            "params": [parent_id, name, { "showContent": true }]
        });
        self.post_rpc(payload).await
    }

    async fn get_normalized_move_modules(&self, package_id: &str) -> Result<Value> {
        let payload = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "sui_getNormalizedMoveModulesByPackage",
            "params": [package_id]
        });
        self.post_rpc(payload).await
    }

    async fn get_object_meta(&self, object_id: &str) -> Result<ObjectMeta> {
        let payload = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "sui_getObject",
            "params": [
                object_id,
                {
                    "showContent": false,
                    "showType": true,
                    "showOwner": true,
                    "showPreviousTransaction": false,
                    "showStorageRebate": false,
                    "showDisplay": false
                }
            ]
        });
        let result = self.post_rpc(payload).await?;
        parse_object_meta(object_id, &result)
    }

    async fn get_coins(
        &self,
        owner: &str,
        coin_type: Option<&str>,
        cursor: Option<&str>,
        limit: Option<u32>,
    ) -> Result<Vec<ResolvedCoin>> {
        let mut out = Vec::new();
        let mut cursor = cursor.map(|s| s.to_string());
        loop {
            let payload = json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "suix_getCoins",
                "params": [
                    owner,
                    coin_type,
                    cursor,
                    limit.unwrap_or(50)
                ]
            });
            let result = self.post_rpc(payload).await?;
            let page = result
                .get("data")
                .and_then(|d| d.as_array())
                .ok_or_else(|| anyhow!("suix_getCoins missing data array"))?;
            for coin in page {
                out.push(parse_resolved_coin(coin)?);
            }
            let has_next = result
                .get("hasNextPage")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            if !has_next {
                break;
            }
            cursor = result
                .get("nextCursor")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            if cursor.is_none() {
                break;
            }
        }
        Ok(out)
    }

    async fn execute_transaction_block(
        &self,
        tx_bytes_base64: &str,
        signatures: &[String],
    ) -> Result<Value> {
        let payload = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "sui_executeTransactionBlock",
            "params": [
                tx_bytes_base64,
                signatures,
                {
                    "showInput": false,
                    "showRawInput": false,
                    "showEffects": true,
                    "showEvents": true,
                    "showObjectChanges": true,
                    "showBalanceChanges": true,
                    "showRawEffects": false
                },
                "WaitForLocalExecution"
            ]
        });
        self.post_rpc(payload).await
    }
}

fn parse_u64_flexible(value: &Value) -> Result<u64> {
    if let Some(n) = value.as_u64() {
        return Ok(n);
    }
    if let Some(s) = value.as_str() {
        return s
            .parse::<u64>()
            .map_err(|e| anyhow!("invalid u64 string `{s}`: {e}"));
    }
    Err(anyhow!("expected u64-compatible value, got {value}"))
}

fn parse_object_meta(object_id: &str, result: &Value) -> Result<ObjectMeta> {
    let data = result
        .get("data")
        .ok_or_else(|| anyhow!("sui_getObject missing data for {object_id}"))?;
    let version = data
        .get("version")
        .ok_or_else(|| anyhow!("object {object_id} missing version"))
        .and_then(parse_u64_flexible)?;
    let digest = data
        .get("digest")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("object {object_id} missing digest"))?
        .to_string();
    let owner = data
        .get("owner")
        .ok_or_else(|| anyhow!("object {object_id} missing owner"))?;

    let (owner_kind, initial_shared_version, mutable) =
        if owner.get("AddressOwner").is_some() || owner.get("ObjectOwner").is_some() {
            (OwnerKind::Owned, None, false)
        } else if let Some(shared) = owner.get("Shared") {
            let initial = shared
                .get("initial_shared_version")
                .ok_or_else(|| anyhow!("shared object {object_id} missing initial_shared_version"))
                .and_then(parse_u64_flexible)?;
            let mutable = crate::discovery::registry::shared_object_is_mutable(object_id);
            (OwnerKind::Shared, Some(initial), mutable)
        } else if owner.as_str() == Some("Immutable")
            || owner.get("Immutable").is_some()
            || owner
                .as_object()
                .is_some_and(|m| m.contains_key("Immutable"))
        {
            (OwnerKind::Immutable, None, false)
        } else {
            return Err(anyhow!("unsupported owner shape for {object_id}: {owner}"));
        };

    Ok(ObjectMeta {
        object_id: data
            .get("objectId")
            .and_then(|v| v.as_str())
            .unwrap_or(object_id)
            .to_string(),
        version,
        digest,
        owner_kind,
        initial_shared_version,
        mutable,
    })
}

fn parse_resolved_coin(coin: &Value) -> Result<ResolvedCoin> {
    let object_id = coin
        .get("coinObjectId")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("coin missing coinObjectId"))?
        .to_string();
    let version = coin
        .get("version")
        .ok_or_else(|| anyhow!("coin {object_id} missing version"))
        .and_then(parse_u64_flexible)?;
    let digest = coin
        .get("digest")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("coin {object_id} missing digest"))?
        .to_string();
    let balance = coin
        .get("balance")
        .ok_or_else(|| anyhow!("coin {object_id} missing balance"))
        .and_then(parse_u64_flexible)?;
    let coin_type = coin
        .get("coinType")
        .and_then(|v| v.as_str())
        .unwrap_or("0x2::sui::SUI")
        .to_string();
    Ok(ResolvedCoin {
        object_id,
        version,
        digest,
        balance,
        coin_type,
    })
}

/// Deterministic, external-infrastructure-free Sui client for canonical transaction tests.
///
/// It deliberately does not emulate transaction execution. Its scope is resolving the
/// object and coin metadata required to build, serialize and locally sign a transaction.
#[derive(Clone)]
pub struct InMemorySuiClient {
    object_meta: Arc<RwLock<HashMap<String, ObjectMeta>>>,
    coins: Arc<RwLock<Vec<ResolvedCoin>>>,
    reference_gas_price: u64,
}

impl InMemorySuiClient {
    pub fn new(reference_gas_price: u64) -> Self {
        Self {
            object_meta: Arc::new(RwLock::new(HashMap::new())),
            coins: Arc::new(RwLock::new(Vec::new())),
            reference_gas_price,
        }
    }

    pub fn insert_object_meta(&self, meta: ObjectMeta) {
        self.object_meta
            .write()
            .expect("in-memory Sui object lock poisoned")
            .insert(meta.object_id.clone(), meta);
    }

    pub fn insert_coin(&self, coin: ResolvedCoin) {
        self.coins
            .write()
            .expect("in-memory Sui coin lock poisoned")
            .push(coin);
    }
}

#[async_trait]
impl SuiClientTrait for InMemorySuiClient {
    async fn get_object(&self, object_id: &str) -> Result<Value> {
        let meta = self.get_object_meta(object_id).await?;
        Ok(json!({
            "data": {
                "objectId": meta.object_id,
                "version": meta.version,
                "digest": meta.digest
            }
        }))
    }

    async fn query_graphql(&self, _query: &str) -> Result<Value> {
        Err(anyhow!("InMemorySuiClient does not emulate GraphQL"))
    }

    async fn query_graphql_with_variables(&self, _query: &str, _variables: Value) -> Result<Value> {
        Err(anyhow!("InMemorySuiClient does not emulate GraphQL"))
    }

    async fn get_reference_gas_price(&self) -> Result<u64> {
        Ok(self.reference_gas_price)
    }

    async fn get_coin_metadata(&self, _coin_type: &str) -> Result<Value> {
        Err(anyhow!("InMemorySuiClient does not emulate coin metadata"))
    }

    async fn get_dynamic_fields(
        &self,
        _parent_id: &str,
        _cursor: Option<&str>,
        _limit: Option<u32>,
    ) -> Result<Value> {
        Err(anyhow!("InMemorySuiClient does not emulate dynamic fields"))
    }

    async fn get_dynamic_field_object(&self, _parent_id: &str, _name: &Value) -> Result<Value> {
        Err(anyhow!(
            "InMemorySuiClient does not emulate dynamic field objects"
        ))
    }

    async fn get_normalized_move_modules(&self, _package_id: &str) -> Result<Value> {
        Err(anyhow!(
            "InMemorySuiClient does not emulate normalized Move modules"
        ))
    }

    async fn get_object_meta(&self, object_id: &str) -> Result<ObjectMeta> {
        self.object_meta
            .read()
            .expect("in-memory Sui object lock poisoned")
            .get(object_id)
            .cloned()
            .ok_or_else(|| anyhow!("in-memory object metadata missing for {object_id}"))
    }

    async fn get_coins(
        &self,
        _owner: &str,
        coin_type: Option<&str>,
        _cursor: Option<&str>,
        limit: Option<u32>,
    ) -> Result<Vec<ResolvedCoin>> {
        let mut coins: Vec<_> = self
            .coins
            .read()
            .expect("in-memory Sui coin lock poisoned")
            .iter()
            .filter(|coin| coin_type.is_none_or(|expected| coin.coin_type == expected))
            .cloned()
            .collect();
        if let Some(limit) = limit {
            coins.truncate(limit as usize);
        }
        Ok(coins)
    }

    async fn execute_transaction_block(
        &self,
        _tx_bytes_base64: &str,
        _signatures: &[String],
    ) -> Result<Value> {
        Err(anyhow!(
            "InMemorySuiClient cannot execute transactions; dummy canonical mode only"
        ))
    }
}

// Unit Tests for SuiClient and SuiClientTrait
#[cfg(test)]
pub mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::Mutex;

    // A manual mock client for testing collectors and other code dependent on SuiClientTrait
    #[allow(clippy::type_complexity)]
    pub struct MockSuiClient {
        pub get_object_mock: Arc<Mutex<Box<dyn Fn(String) -> Result<Value> + Send + Sync>>>,
        pub query_graphql_mock: Arc<Mutex<Box<dyn Fn(String) -> Result<Value> + Send + Sync>>>,
        pub query_graphql_with_variables_mock:
            Arc<Mutex<Box<dyn Fn(String, Value) -> Result<Value> + Send + Sync>>>,
        pub get_reference_gas_price_mock: Arc<Mutex<Box<dyn Fn() -> Result<u64> + Send + Sync>>>,
        pub get_coin_metadata_mock: Arc<Mutex<Box<dyn Fn(String) -> Result<Value> + Send + Sync>>>,
        pub get_dynamic_fields_mock: Arc<
            Mutex<Box<dyn Fn(String, Option<String>, Option<u32>) -> Result<Value> + Send + Sync>>,
        >,
        pub get_dynamic_field_object_mock:
            Arc<Mutex<Box<dyn Fn(String, Value) -> Result<Value> + Send + Sync>>>,
        pub get_normalized_move_modules_mock:
            Arc<Mutex<Box<dyn Fn(String) -> Result<Value> + Send + Sync>>>,
        pub get_object_meta_mock:
            Arc<Mutex<Box<dyn Fn(String) -> Result<ObjectMeta> + Send + Sync>>>,
        pub get_coins_mock: Arc<
            Mutex<
                Box<
                    dyn Fn(
                            String,
                            Option<String>,
                            Option<String>,
                            Option<u32>,
                        ) -> Result<Vec<ResolvedCoin>>
                        + Send
                        + Sync,
                >,
            >,
        >,
        pub execute_transaction_block_mock:
            Arc<Mutex<Box<dyn Fn(String, Vec<String>) -> Result<Value> + Send + Sync>>>,
    }

    impl Default for MockSuiClient {
        fn default() -> Self {
            Self::new()
        }
    }

    impl MockSuiClient {
        pub fn new() -> Self {
            Self {
                get_object_mock: Arc::new(Mutex::new(Box::new(|_| {
                    Err(anyhow!("get_object not mocked"))
                }))),
                query_graphql_mock: Arc::new(Mutex::new(Box::new(|_| {
                    Err(anyhow!("query_graphql not mocked"))
                }))),
                query_graphql_with_variables_mock: Arc::new(Mutex::new(Box::new(|_, _| {
                    Err(anyhow!("query_graphql_with_variables not mocked"))
                }))),
                get_reference_gas_price_mock: Arc::new(Mutex::new(Box::new(|| Ok(1000)))),
                get_coin_metadata_mock: Arc::new(Mutex::new(Box::new(|_| {
                    Ok(json!({
                        "symbol": "MOCK",
                        "name": "Mock Coin",
                        "decimals": 9
                    }))
                }))),
                get_dynamic_fields_mock: Arc::new(Mutex::new(Box::new(|_, _, _| {
                    Err(anyhow!("get_dynamic_fields not mocked"))
                }))),
                get_dynamic_field_object_mock: Arc::new(Mutex::new(Box::new(|_, _| {
                    Err(anyhow!("get_dynamic_field_object not mocked"))
                }))),
                get_normalized_move_modules_mock: Arc::new(Mutex::new(Box::new(|_| {
                    Err(anyhow!("get_normalized_move_modules not mocked"))
                }))),
                get_object_meta_mock: Arc::new(Mutex::new(Box::new(|_| {
                    Err(anyhow!("get_object_meta not mocked"))
                }))),
                get_coins_mock: Arc::new(Mutex::new(Box::new(|_, _, _, _| {
                    Err(anyhow!("get_coins not mocked"))
                }))),
                execute_transaction_block_mock: Arc::new(Mutex::new(Box::new(|_, _| {
                    Err(anyhow!("execute_transaction_block not mocked"))
                }))),
            }
        }
    }

    #[async_trait]
    impl SuiClientTrait for MockSuiClient {
        async fn get_object(&self, object_id: &str) -> Result<Value> {
            let mock = self.get_object_mock.lock().unwrap();
            mock(object_id.to_string())
        }

        async fn query_graphql(&self, query: &str) -> Result<Value> {
            let mock = self.query_graphql_mock.lock().unwrap();
            mock(query.to_string())
        }

        async fn query_graphql_with_variables(
            &self,
            query: &str,
            variables: Value,
        ) -> Result<Value> {
            let mock = self.query_graphql_with_variables_mock.lock().unwrap();
            mock(query.to_string(), variables)
        }

        async fn get_reference_gas_price(&self) -> Result<u64> {
            let mock = self.get_reference_gas_price_mock.lock().unwrap();
            mock()
        }

        async fn get_coin_metadata(&self, coin_type: &str) -> Result<Value> {
            let mock = self.get_coin_metadata_mock.lock().unwrap();
            mock(coin_type.to_string())
        }

        async fn get_dynamic_fields(
            &self,
            parent_id: &str,
            cursor: Option<&str>,
            limit: Option<u32>,
        ) -> Result<Value> {
            let mock = self.get_dynamic_fields_mock.lock().unwrap();
            mock(parent_id.to_string(), cursor.map(|s| s.to_string()), limit)
        }

        async fn get_dynamic_field_object(&self, parent_id: &str, name: &Value) -> Result<Value> {
            let mock = self.get_dynamic_field_object_mock.lock().unwrap();
            mock(parent_id.to_string(), name.clone())
        }

        async fn get_normalized_move_modules(&self, package_id: &str) -> Result<Value> {
            let mock = self.get_normalized_move_modules_mock.lock().unwrap();
            mock(package_id.to_string())
        }

        async fn get_object_meta(&self, object_id: &str) -> Result<ObjectMeta> {
            let mock = self.get_object_meta_mock.lock().unwrap();
            mock(object_id.to_string())
        }

        async fn get_coins(
            &self,
            owner: &str,
            coin_type: Option<&str>,
            cursor: Option<&str>,
            limit: Option<u32>,
        ) -> Result<Vec<ResolvedCoin>> {
            let mock = self.get_coins_mock.lock().unwrap();
            mock(
                owner.to_string(),
                coin_type.map(|s| s.to_string()),
                cursor.map(|s| s.to_string()),
                limit,
            )
        }

        async fn execute_transaction_block(
            &self,
            tx_bytes_base64: &str,
            signatures: &[String],
        ) -> Result<Value> {
            let mock = self.execute_transaction_block_mock.lock().unwrap();
            mock(tx_bytes_base64.to_string(), signatures.to_vec())
        }
    }

    #[tokio::test]
    async fn test_mock_client() {
        let mock_client = MockSuiClient::new();

        *mock_client.get_object_mock.lock().unwrap() = Box::new(|obj_id| {
            assert_eq!(obj_id, "0x123");
            Ok(json!({ "id": "0x123", "value": "test" }))
        });

        let res = mock_client.get_object("0x123").await.unwrap();
        assert_eq!(res["value"], "test");
    }

    #[tokio::test]
    async fn test_sui_client_failover_mechanism() {
        let rpc_urls = "http://127.0.0.1:11111,http://127.0.0.1:22222".to_string();
        let client = SuiClient::new(rpc_urls, "http://127.0.0.1:33333".to_string());

        assert_eq!(client.rpc_urls.len(), 2);
        assert_eq!(client.rpc_urls[0], "http://127.0.0.1:11111");
        assert_eq!(client.rpc_urls[1], "http://127.0.0.1:22222");

        let res = client.get_object("0x1").await;
        assert!(res.is_err());
        let err_str = res.unwrap_err().to_string();
        assert!(err_str.contains("http://127.0.0.1:22222"));
    }

    #[tokio::test]
    async fn test_sui_client_graphql_failover_mechanism() {
        let graphql_urls = "http://127.0.0.1:44444,http://127.0.0.1:55555".to_string();
        let client = SuiClient::new("http://127.0.0.1:33333".to_string(), graphql_urls);

        let res = client.query_graphql("query { version }").await;
        assert!(res.is_err());
        let err_str = res.unwrap_err().to_string();
        assert!(err_str.contains("http://127.0.0.1:55555"));
    }

    #[tokio::test]
    async fn in_memory_client_resolves_deterministic_inputs() {
        let client = InMemorySuiClient::new(1_234);
        client.insert_object_meta(ObjectMeta {
            object_id: "0x42".to_string(),
            version: 9,
            digest: sui_sdk_types::Digest::ZERO.to_base58(),
            owner_kind: OwnerKind::Shared,
            initial_shared_version: Some(3),
            mutable: true,
        });
        client.insert_coin(ResolvedCoin {
            object_id: "0x99".to_string(),
            version: 7,
            digest: sui_sdk_types::Digest::ZERO.to_base58(),
            balance: 5_000,
            coin_type: "0x2::sui::SUI".to_string(),
        });

        assert_eq!(client.get_reference_gas_price().await.unwrap(), 1_234);
        assert_eq!(client.get_object_meta("0x42").await.unwrap().version, 9);
        assert_eq!(
            client
                .get_coins("0x1", Some("0x2::sui::SUI"), None, None)
                .await
                .unwrap()[0]
                .balance,
            5_000
        );
        assert!(client.get_object_meta("0x404").await.is_err());
        assert!(
            client
                .execute_transaction_block("dummy", &[])
                .await
                .unwrap_err()
                .to_string()
                .contains("dummy canonical mode")
        );
    }
}
