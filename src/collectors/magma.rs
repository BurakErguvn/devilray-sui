use crate::collectors::DexDataCollector;
use crate::collectors::tick_fetch::{
    build_pool_tick_data, collect_ticks_from_skip_list, extract_nested_object_id,
    parse_cetus_tick_index, parse_fee_growth_global, parse_move_object_response,
    parse_raw_tick_index, parse_u32_field,
};
use crate::models::{PoolState, PoolTickData};
use crate::sui_client::SuiClientTrait;
use anyhow::{Result, anyhow};
use async_trait::async_trait;
use serde_json::Value;

pub struct MagmaCollector;

impl Default for MagmaCollector {
    fn default() -> Self {
        Self::new()
    }
}

impl MagmaCollector {
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

        // 3. Fallback
        if let Some(type_str) = val.get("type").and_then(|t| t.as_str())
            && let Some(fields) = val.get("fields")
        {
            return Ok((fields.clone(), type_str.to_string()));
        }

        Err(anyhow!("Unsupported response format: {:?}", val))
    }
}

#[async_trait]
impl DexDataCollector for MagmaCollector {
    fn dex_name(&self) -> &'static str {
        "Magma Finance"
    }

    async fn fetch_pool(&self, client: &dyn SuiClientTrait, pool_id: &str) -> Result<PoolState> {
        // Query the pool object
        let response = client.get_object(pool_id).await?;
        let (fields, type_str) = self.parse_json_response(&response)?;

        let (coin_type_a, coin_type_b) = self.parse_coin_types(&type_str)?;

        // Magma (ALMM/CLMM) may use `current_sqrt_price` (Cetus style) or `sqrt_price` (Turbos/Uniswap style)
        let sqrt_price = if let Some(price_val) = fields.get("current_sqrt_price") {
            let price_str = price_val
                .as_str()
                .ok_or_else(|| anyhow!("current_sqrt_price is not a string"))?;
            price_str.parse::<u128>()?
        } else if let Some(price_val) = fields.get("sqrt_price") {
            let price_str = price_val
                .as_str()
                .ok_or_else(|| anyhow!("sqrt_price is not a string"))?;
            price_str.parse::<u128>()?
        } else {
            return Err(anyhow!("Magma pool missing current_sqrt_price/sqrt_price"));
        };

        let liquidity_str = fields
            .get("liquidity")
            .ok_or_else(|| anyhow!("Magma pool missing liquidity"))?
            .as_str()
            .ok_or_else(|| anyhow!("liquidity is not a string"))?;
        let liquidity = liquidity_str.parse::<u128>()?;

        // Extract fee rate
        let fee_rate = if let Some(fee_val) = fields.get("fee_rate") {
            if let Some(fee_str) = fee_val.as_str() {
                fee_str.parse::<u64>()?
            } else {
                fee_val.as_u64().unwrap_or_default()
            }
        } else {
            0
        };

        // Extract pause state (defaults to false if not found)
        let is_paused = fields
            .get("is_pause")
            .or_else(|| fields.get("is_paused"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        Ok(PoolState {
            pool_id: pool_id.to_string(),
            dex_name: self.dex_name().to_string(),
            coin_type_a,
            coin_type_b,
            sqrt_price,
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
        let (fields, _) = parse_move_object_response(&response)?;

        let current_tick_index = parse_raw_tick_index(&fields, &["current_tick_index"])
            .or_else(|_| parse_cetus_tick_index(&fields))?;
        let tick_spacing = parse_u32_field(&fields, &["tick_spacing"])?;

        let tick_manager = fields
            .get("tick_manager")
            .ok_or_else(|| anyhow!("Magma pool missing tick_manager"))?;
        let ticks_node = tick_manager
            .get("fields")
            .and_then(|inner| inner.get("ticks"))
            .or_else(|| tick_manager.get("ticks"))
            .ok_or_else(|| anyhow!("Magma tick_manager missing ticks"))?;
        let skip_list_id = extract_nested_object_id(ticks_node)
            .ok_or_else(|| anyhow!("Magma ticks SkipList missing object id"))?;

        let ticks = collect_ticks_from_skip_list(
            client,
            &skip_list_id,
            current_tick_index,
            crate::collectors::MAX_TICK_FETCH_WINDOW,
        )
        .await?;

        let (fee_a, fee_b) = parse_fee_growth_global(&fields)?;

        Ok(build_pool_tick_data(
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
    async fn test_magma_collector() {
        let mock_client = MockSuiClient::new();

        *mock_client.get_object_mock.lock().unwrap() = Box::new(|pool_id| {
            assert_eq!(pool_id, "0x_magma_pool");
            Ok(json!({
                "data": {
                    "objectId": "0x_magma_pool",
                    "content": {
                        "type": "0x_magma::pool::Pool<0x2::sui::SUI, 0x5d4b3::coin::COIN>",
                        "fields": {
                            "current_sqrt_price": "18446744073709551616",
                            "liquidity": "40000000",
                            "is_pause": false,
                            "fee_rate": "3000"
                        }
                    }
                }
            }))
        });

        let collector = MagmaCollector::new();
        let pool_state = collector
            .fetch_pool(&mock_client, "0x_magma_pool")
            .await
            .unwrap();

        assert_eq!(pool_state.pool_id, "0x_magma_pool");
        assert_eq!(pool_state.dex_name, "Magma Finance");
        assert_eq!(pool_state.sqrt_price, 18446744073709551616);
        assert_eq!(pool_state.liquidity, 40000000);
        assert!(!pool_state.is_paused);
    }

    #[tokio::test]
    async fn test_magma_fetch_tick_data() {
        let mock_client = MockSuiClient::new();

        *mock_client.get_object_mock.lock().unwrap() = Box::new(|obj_id| {
            if obj_id == "0x_magma_pool" {
                return Ok(json!({
                    "data": {
                        "content": {
                            "type": "0x_magma::pool::Pool<...>",
                            "fields": {
                                "current_tick_index": { "fields": { "bits": 72_000 } },
                                "tick_spacing": 10,
                                "tick_manager": {
                                    "fields": {
                                        "ticks": {
                                            "fields": {
                                                "id": { "id": "0x_magma_skip_list" }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }));
            }
            if obj_id == "0x_magma_skip_node" {
                return Ok(json!({
                    "data": {
                        "content": {
                            "type": "0x_magma::skip_list::Node<...>",
                            "fields": {
                                "value": {
                                    "fields": {
                                        "value": {
                                            "fields": {
                                                "index": { "fields": { "bits": 72_000 } },
                                                "liquidity_net": { "fields": { "bits": "804" } }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }));
            }
            Err(anyhow!("unexpected object {}", obj_id))
        });

        *mock_client.get_dynamic_fields_mock.lock().unwrap() =
            Box::new(|parent_id, _cursor, _limit| {
                assert_eq!(parent_id, "0x_magma_skip_list");
                Ok(json!({
                    "data": [{
                        "objectId": "0x_magma_skip_node",
                        "name": { "type": "u64", "value": "72000" }
                    }],
                    "nextCursor": null
                }))
            });

        let collector = MagmaCollector::new();
        let tick_data = collector
            .fetch_tick_data(&mock_client, "0x_magma_pool")
            .await
            .unwrap();

        assert_eq!(tick_data.current_tick_index, 72_000);
        assert_eq!(tick_data.tick_spacing, 10);
        assert_eq!(tick_data.ticks.len(), 1);
        assert_eq!(tick_data.ticks[0].tick_index, 72_000);
        assert_eq!(tick_data.ticks[0].liquidity_net, 201);
    }
}
