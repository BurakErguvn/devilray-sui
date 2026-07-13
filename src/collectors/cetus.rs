use crate::collectors::{DexDataCollector, MAX_TICK_FETCH_WINDOW};
use crate::models::{PoolState, PoolTickData, TickInfo};
use crate::sui_client::SuiClientTrait;
use crate::sui_int::{
    decode_sui_i32, decode_sui_i128, parse_i32_bits_from_json, parse_i128_bits_from_json,
};
use anyhow::{Result, anyhow};
use async_trait::async_trait;
use serde_json::Value;

pub struct CetusCollector;

impl Default for CetusCollector {
    fn default() -> Self {
        Self::new()
    }
}

impl CetusCollector {
    pub fn new() -> Self {
        Self
    }

    /// Parses coin types from the pool generic type string (e.g. `0x...::pool::Pool<CoinA, CoinB>`)
    fn parse_coin_types(&self, type_str: &str) -> Result<(String, String)> {
        let parts: Vec<&str> = type_str.split('<').collect();
        if parts.len() < 2 {
            return Err(anyhow!("Invalid pool type format: missing '<'"));
        }
        let generic_part = parts[1].trim_end_matches('>');
        let coins: Vec<&str> = generic_part.split(',').collect();
        if coins.len() < 2 {
            return Err(anyhow!(
                "Invalid pool type format: missing comma separator for coins"
            ));
        }
        Ok((coins[0].trim().to_string(), coins[1].trim().to_string()))
    }

    /// Normalizes JSON-RPC or GraphQL responses into standard fields and type
    fn parse_json_response(&self, val: &Value) -> Result<(Value, String)> {
        // 1. Try legacy JSON-RPC format: `data.content.fields` and `data.content.type`
        if let Some(content) = val.get("data").and_then(|d| d.get("content")) {
            let fields = content
                .get("fields")
                .ok_or_else(|| anyhow!("Missing 'fields' in content"))?;
            let type_str = content
                .get("type")
                .and_then(|t| t.as_str())
                .ok_or_else(|| anyhow!("Missing 'type' in content"))?;
            return Ok((fields.clone(), type_str.to_string()));
        }

        // 2. Try GraphQL format: `object.asMoveObject.contents.json` and `object.asMoveObject.type.repr`
        if let Some(as_move) = val.get("object").and_then(|o| o.get("asMoveObject")) {
            let fields = as_move
                .get("contents")
                .and_then(|c| c.get("json"))
                .ok_or_else(|| anyhow!("Missing 'contents.json' in GraphQL response"))?;
            let type_str = as_move
                .get("type")
                .and_then(|t| t.get("repr"))
                .and_then(|r| r.as_str())
                .ok_or_else(|| anyhow!("Missing 'type.repr' in GraphQL response"))?;
            return Ok((fields.clone(), type_str.to_string()));
        }

        // 3. Fallback: if it's already a raw fields structure (e.g. for testing)
        if let Some(type_str) = val.get("type").and_then(|t| t.as_str())
            && let Some(fields) = val.get("fields")
        {
            return Ok((fields.clone(), type_str.to_string()));
        }

        Err(anyhow!("Unsupported response format: {:?}", val))
    }

    fn parse_u32_field(fields: &Value, key: &str) -> Result<u32> {
        let val = fields
            .get(key)
            .ok_or_else(|| anyhow!("Cetus pool missing {}", key))?;
        if let Some(s) = val.as_str() {
            return Ok(s.parse::<u32>()?);
        }
        if let Some(n) = val.as_u64() {
            return Ok(n as u32);
        }
        Err(anyhow!("{} is not a valid u32", key))
    }

    fn parse_current_tick_index(fields: &Value) -> Result<i32> {
        let tick_val = fields
            .get("current_tick_index")
            .ok_or_else(|| anyhow!("Cetus pool missing current_tick_index"))?;
        let bits = parse_i32_bits_from_json(tick_val)
            .ok_or_else(|| anyhow!("current_tick_index missing bits"))?;
        Ok(decode_sui_i32(bits))
    }

    fn parse_tick_index_from_field_name(name: &Value) -> Option<i32> {
        let value = name.get("value")?;
        let bits = parse_i32_bits_from_json(value)?;
        Some(decode_sui_i32(bits))
    }

    fn parse_liquidity_net_from_tick_object(obj: &Value) -> Option<i128> {
        let fields = obj
            .get("data")
            .and_then(|d| d.get("content"))
            .and_then(|c| c.get("fields"))?;
        let ln = fields.get("liquidity_net")?;
        let bits = parse_i128_bits_from_json(ln)?;
        Some(decode_sui_i128(bits))
    }

    fn tick_in_window(tick_index: i32, current: i32, window: i32) -> bool {
        tick_index >= current - window && tick_index <= current + window
    }
}

#[async_trait]
impl DexDataCollector for CetusCollector {
    fn dex_name(&self) -> &'static str {
        "Cetus"
    }

    async fn fetch_pool(&self, client: &dyn SuiClientTrait, pool_id: &str) -> Result<PoolState> {
        // Query the pool object
        let response = client.get_object(pool_id).await?;
        let (fields, type_str) = self.parse_json_response(&response)?;

        let (coin_type_a, coin_type_b) = self.parse_coin_types(&type_str)?;

        // Extract pool state fields
        let current_sqrt_price_str = fields
            .get("current_sqrt_price")
            .ok_or_else(|| anyhow!("Cetus pool missing current_sqrt_price"))?
            .as_str()
            .ok_or_else(|| anyhow!("current_sqrt_price is not a string"))?;
        let current_sqrt_price = current_sqrt_price_str.parse::<u128>()?;

        let liquidity_str = fields
            .get("liquidity")
            .ok_or_else(|| anyhow!("Cetus pool missing liquidity"))?
            .as_str()
            .ok_or_else(|| anyhow!("liquidity is not a string"))?;
        let liquidity = liquidity_str.parse::<u128>()?;

        let fee_rate_str = fields
            .get("fee_rate")
            .ok_or_else(|| anyhow!("Cetus pool missing fee_rate"))?
            .as_str()
            .ok_or_else(|| anyhow!("fee_rate is not a string"))?;
        let fee_rate = fee_rate_str.parse::<u64>()?;

        let is_paused = fields
            .get("is_pause")
            .ok_or_else(|| anyhow!("Cetus pool missing is_pause"))?
            .as_bool()
            .ok_or_else(|| anyhow!("is_pause is not a boolean"))?;

        Ok(PoolState {
            pool_id: pool_id.to_string(),
            dex_name: self.dex_name().to_string(),
            coin_type_a,
            coin_type_b,
            sqrt_price: current_sqrt_price,
            liquidity,
            fee_rate,
            is_paused,
        })
    }

    async fn fetch_tick_data(
        &self,
        client: &dyn SuiClientTrait,
        pool_id: &str,
    ) -> Result<PoolTickData> {
        let response = client.get_object(pool_id).await?;
        let (fields, _) = self.parse_json_response(&response)?;

        let current_tick_index = Self::parse_current_tick_index(&fields)?;
        let tick_spacing = Self::parse_u32_field(&fields, "tick_spacing")?;

        let mut ticks = Vec::new();
        let mut cursor: Option<String> = None;
        let mut pages = 0u32;

        while pages < 10 {
            pages += 1;
            let page = client
                .get_dynamic_fields(pool_id, cursor.as_deref(), Some(50))
                .await?;

            let data = page
                .get("data")
                .and_then(|d| d.as_array())
                .cloned()
                .unwrap_or_default();

            if data.is_empty() {
                break;
            }

            for entry in &data {
                let name = match entry.get("name") {
                    Some(n) => n,
                    None => continue,
                };
                let tick_index = match Self::parse_tick_index_from_field_name(name) {
                    Some(t) => t,
                    None => continue,
                };
                if !Self::tick_in_window(tick_index, current_tick_index, MAX_TICK_FETCH_WINDOW) {
                    continue;
                }

                let object_id = entry
                    .get("objectId")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                if object_id.is_empty() {
                    continue;
                }

                let tick_obj = match client.get_object(object_id).await {
                    Ok(o) => o,
                    Err(e) => {
                        tracing::warn!(
                            "Failed to fetch tick object {} for pool {}: {:?}",
                            object_id,
                            pool_id,
                            e
                        );
                        continue;
                    }
                };

                let liquidity_net = match Self::parse_liquidity_net_from_tick_object(&tick_obj) {
                    Some(ln) => ln,
                    None => continue,
                };

                if liquidity_net != 0 {
                    ticks.push(TickInfo {
                        tick_index,
                        liquidity_net,
                    });
                }
            }

            cursor = page
                .get("nextCursor")
                .and_then(|c| c.as_str())
                .map(|s| s.to_string());

            if cursor.is_none() {
                break;
            }
        }

        ticks.sort_by_key(|t| t.tick_index);
        ticks.dedup_by_key(|t| t.tick_index);

        let (fee_a, fee_b) = crate::collectors::tick_fetch::parse_fee_growth_global(&fields)?;

        Ok(crate::collectors::tick_fetch::build_pool_tick_data(
            pool_id,
            current_tick_index,
            tick_spacing,
            ticks,
            fee_a,
            fee_b,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sui_client::tests::MockSuiClient;
    use serde_json::json;

    #[tokio::test]
    async fn test_cetus_collector_json_rpc() {
        let mock_client = MockSuiClient::new();

        *mock_client.get_object_mock.lock().unwrap() = Box::new(|pool_id| {
            assert_eq!(pool_id, "0x_cetus_pool");
            Ok(json!({
                "data": {
                    "objectId": "0x_cetus_pool",
                    "content": {
                        "type": "0x1eabed::pool::Pool<0x2::sui::SUI, 0x5d4b3::coin::COIN>",
                        "fields": {
                            "current_sqrt_price": "18446744073709551616",
                            "liquidity": "50000000",
                            "is_pause": false,
                            "fee_rate": "3000"
                        }
                    }
                }
            }))
        });

        let collector = CetusCollector::new();
        let pool_state = collector
            .fetch_pool(&mock_client, "0x_cetus_pool")
            .await
            .unwrap();

        assert_eq!(pool_state.pool_id, "0x_cetus_pool");
        assert_eq!(pool_state.dex_name, "Cetus");
        assert_eq!(pool_state.coin_type_a, "0x2::sui::SUI");
        assert_eq!(pool_state.coin_type_b, "0x5d4b3::coin::COIN");
        assert_eq!(pool_state.sqrt_price, 18446744073709551616);
        assert_eq!(pool_state.liquidity, 50000000);
        assert_eq!(pool_state.fee_rate, 3000);
        assert!(!pool_state.is_paused);
    }

    #[tokio::test]
    async fn test_cetus_collector_graphql() {
        let mock_client = MockSuiClient::new();

        *mock_client.get_object_mock.lock().unwrap() = Box::new(|pool_id| {
            assert_eq!(pool_id, "0x_cetus_pool");
            Ok(json!({
                "object": {
                    "asMoveObject": {
                        "type": {
                            "repr": "0x1eabed::pool::Pool<0x2::sui::SUI, 0x5d4b3::coin::COIN>"
                        },
                        "contents": {
                            "json": {
                                "current_sqrt_price": "18446744073709551616",
                                "liquidity": "50000000",
                                "is_pause": false,
                                "fee_rate": "3000"
                            }
                        }
                    }
                }
            }))
        });

        let collector = CetusCollector::new();
        let pool_state = collector
            .fetch_pool(&mock_client, "0x_cetus_pool")
            .await
            .unwrap();

        assert_eq!(pool_state.coin_type_a, "0x2::sui::SUI");
        assert_eq!(pool_state.coin_type_b, "0x5d4b3::coin::COIN");
        assert_eq!(pool_state.sqrt_price, 18446744073709551616);
    }

    #[tokio::test]
    async fn test_cetus_fetch_tick_data() {
        let mock_client = MockSuiClient::new();

        *mock_client.get_object_mock.lock().unwrap() = Box::new(|obj_id| {
            if obj_id == "0x_cetus_pool" {
                return Ok(json!({
                    "data": {
                        "content": {
                            "type": "0x1eabed::pool::Pool<0x2::sui::SUI, 0x5d4b3::coin::COIN>",
                            "fields": {
                                "current_sqrt_price": "18446744073709551616",
                                "liquidity": "50000000",
                                "is_pause": false,
                                "fee_rate": "3000",
                                "tick_spacing": 60,
                                "current_tick_index": { "type": "0x2::i32::I32", "fields": { "bits": 240 } }
                            }
                        }
                    }
                }));
            }
            if obj_id == "0x_tick_obj_60" {
                return Ok(json!({
                    "data": {
                        "content": {
                            "fields": {
                                "liquidity_net": { "fields": { "bits": "400" } }
                            }
                        }
                    }
                }));
            }
            Err(anyhow!("unexpected object {}", obj_id))
        });

        *mock_client.get_dynamic_fields_mock.lock().unwrap() =
            Box::new(|parent_id, _cursor, _limit| {
                assert_eq!(parent_id, "0x_cetus_pool");
                Ok(json!({
                    "data": [{
                        "objectId": "0x_tick_obj_60",
                        "name": {
                            "type": "0x2::i32::I32",
                            "value": { "bits": 240 }
                        }
                    }],
                    "nextCursor": null
                }))
            });

        let collector = CetusCollector::new();
        let tick_data = collector
            .fetch_tick_data(&mock_client, "0x_cetus_pool")
            .await
            .unwrap();

        assert_eq!(tick_data.pool_id, "0x_cetus_pool");
        assert_eq!(tick_data.current_tick_index, 60);
        assert_eq!(tick_data.tick_spacing, 60);
        assert_eq!(tick_data.ticks.len(), 1);
        assert_eq!(tick_data.ticks[0].tick_index, 60);
        assert_eq!(tick_data.ticks[0].liquidity_net, 100);
    }
}
