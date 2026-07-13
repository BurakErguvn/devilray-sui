use crate::collectors::DexDataCollector;
use crate::collectors::tick_fetch::{
    build_pool_tick_data, collect_ticks_from_raw_i32_table, extract_nested_object_id,
    parse_coin_types_from_pool_type, parse_fee_growth_global, parse_move_object_response,
    parse_raw_tick_index, parse_u32_field,
};
use crate::models::{PoolState, PoolTickData};
use crate::sui_client::SuiClientTrait;
use anyhow::{Result, anyhow};
use async_trait::async_trait;
use serde_json::Value;

pub struct MomentumCollector;

impl Default for MomentumCollector {
    fn default() -> Self {
        Self::new()
    }
}

impl MomentumCollector {
    pub fn new() -> Self {
        Self
    }

    fn parse_coin_types(&self, type_str: &str) -> Result<(String, String)> {
        parse_coin_types_from_pool_type(type_str)
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
impl DexDataCollector for MomentumCollector {
    fn dex_name(&self) -> &'static str {
        "Momentum"
    }

    async fn fetch_pool(&self, client: &dyn SuiClientTrait, pool_id: &str) -> Result<PoolState> {
        // Query the pool object
        let response = client.get_object(pool_id).await?;
        let (fields, type_str) = self.parse_json_response(&response)?;

        let (coin_type_a, coin_type_b) = self.parse_coin_types(&type_str)?;

        // Momentum CLMM fields (similar to Cetus/Turbos)
        let sqrt_price = if let Some(price_val) = fields.get("sqrt_price") {
            let price_str = price_val
                .as_str()
                .ok_or_else(|| anyhow!("sqrt_price is not a string"))?;
            price_str.parse::<u128>()?
        } else if let Some(price_val) = fields.get("current_sqrt_price") {
            let price_str = price_val
                .as_str()
                .ok_or_else(|| anyhow!("current_sqrt_price is not a string"))?;
            price_str.parse::<u128>()?
        } else {
            return Err(anyhow!(
                "Momentum pool missing sqrt_price/current_sqrt_price"
            ));
        };

        let liquidity_str = fields
            .get("liquidity")
            .ok_or_else(|| anyhow!("Momentum pool missing liquidity"))?
            .as_str()
            .ok_or_else(|| anyhow!("liquidity is not a string"))?;
        let liquidity = liquidity_str.parse::<u128>()?;

        // Extract fee rate (`swap_fee_rate` on live mainnet pools).
        let fee_rate = if let Some(fee_val) = fields.get("swap_fee_rate") {
            if let Some(fee_str) = fee_val.as_str() {
                fee_str.parse::<u64>()?
            } else {
                fee_val.as_u64().unwrap_or_default()
            }
        } else if let Some(fee_val) = fields.get("fee_rate") {
            if let Some(fee_str) = fee_val.as_str() {
                fee_str.parse::<u64>()?
            } else {
                fee_val.as_u64().unwrap_or_default()
            }
        } else if let Some(fee_val) = fields.get("fee") {
            if let Some(fee_str) = fee_val.as_str() {
                fee_str.parse::<u64>()?
            } else {
                fee_val.as_u64().unwrap_or_default()
            }
        } else {
            0
        };

        // Extract pause state
        let is_paused = fields
            .get("is_paused")
            .or_else(|| fields.get("is_pause"))
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

        let current_tick_index =
            parse_raw_tick_index(&fields, &["tick_index", "current_tick_index"])?;
        let tick_spacing = parse_u32_field(&fields, &["tick_spacing"])?;

        let ticks_node = fields
            .get("ticks")
            .ok_or_else(|| anyhow!("Momentum pool missing ticks table"))?;
        let table_id = extract_nested_object_id(ticks_node)
            .ok_or_else(|| anyhow!("Momentum ticks table missing object id"))?;

        let ticks = collect_ticks_from_raw_i32_table(
            client,
            &table_id,
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
    async fn test_momentum_collector() {
        let mock_client = MockSuiClient::new();

        *mock_client.get_object_mock.lock().unwrap() = Box::new(|pool_id| {
            assert_eq!(pool_id, "0x_momentum_pool");
            Ok(json!({
                "data": {
                    "objectId": "0x_momentum_pool",
                    "content": {
                        "type": "0x_momentum::pool::Pool<0x2::sui::SUI, 0x5d4b3::coin::COIN>",
                        "fields": {
                            "sqrt_price": "18446744073709551616",
                            "liquidity": "25000000",
                            "is_paused": false,
                            "fee_rate": "3000"
                        }
                    }
                }
            }))
        });

        let collector = MomentumCollector::new();
        let pool_state = collector
            .fetch_pool(&mock_client, "0x_momentum_pool")
            .await
            .unwrap();

        assert_eq!(pool_state.pool_id, "0x_momentum_pool");
        assert_eq!(pool_state.dex_name, "Momentum");
        assert_eq!(pool_state.sqrt_price, 18446744073709551616);
        assert_eq!(pool_state.liquidity, 25000000);
        assert!(!pool_state.is_paused);
    }

    #[tokio::test]
    async fn test_momentum_fetch_tick_data() {
        let mock_client = MockSuiClient::new();

        *mock_client.get_object_mock.lock().unwrap() = Box::new(|obj_id| {
            if obj_id == "0x_momentum_pool" {
                return Ok(json!({
                    "data": {
                        "content": {
                            "type": "0x_momentum::pool::Pool<...>",
                            "fields": {
                                "tick_index": { "fields": { "bits": 6_882 } },
                                "tick_spacing": 2,
                                "ticks": {
                                    "fields": {
                                        "id": { "id": "0x_momentum_ticks_table" }
                                    }
                                }
                            }
                        }
                    }
                }));
            }
            if obj_id == "0x_momentum_tick_row" {
                return Ok(json!({
                    "data": {
                        "content": {
                            "type": "0x_momentum::tick::TickInfo",
                            "fields": {
                                "name": {
                                    "type": "0xmom::i32::I32",
                                    "fields": { "bits": 6_882 }
                                },
                                "value": {
                                    "fields": {
                                        "liquidity_net": { "fields": { "bits": "804" } }
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
                assert_eq!(parent_id, "0x_momentum_ticks_table");
                Ok(json!({
                    "data": [{
                        "objectId": "0x_momentum_tick_row",
                        "name": {
                            "type": "0xmom::i32::I32",
                            "value": { "bits": 6_882 }
                        }
                    }],
                    "nextCursor": null
                }))
            });

        let collector = MomentumCollector::new();
        let tick_data = collector
            .fetch_tick_data(&mock_client, "0x_momentum_pool")
            .await
            .unwrap();

        assert_eq!(tick_data.current_tick_index, 6_882);
        assert_eq!(tick_data.tick_spacing, 2);
        assert_eq!(tick_data.ticks.len(), 1);
        assert_eq!(tick_data.ticks[0].tick_index, 6_882);
        assert_eq!(tick_data.ticks[0].liquidity_net, 201);
    }
}
