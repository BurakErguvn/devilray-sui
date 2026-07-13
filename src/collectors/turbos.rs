use crate::collectors::DexDataCollector;
use crate::collectors::tick_fetch::{
    build_pool_tick_data, collect_turbos_ticks, parse_coin_types_from_pool_type,
    parse_fee_growth_global, parse_move_object_response, parse_raw_tick_index, parse_u32_field,
};
use crate::models::{PoolState, PoolTickData};
use crate::sui_client::SuiClientTrait;
use anyhow::{Result, anyhow};
use async_trait::async_trait;
use serde_json::Value;

pub struct TurbosCollector;

impl Default for TurbosCollector {
    fn default() -> Self {
        Self::new()
    }
}

impl TurbosCollector {
    pub fn new() -> Self {
        Self
    }

    /// Parses coin types from the pool generic type string (nested-generic safe).
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
impl DexDataCollector for TurbosCollector {
    fn dex_name(&self) -> &'static str {
        "Turbos"
    }

    async fn fetch_pool(&self, client: &dyn SuiClientTrait, pool_id: &str) -> Result<PoolState> {
        // Query the pool object
        let response = client.get_object(pool_id).await?;
        let (fields, type_str) = self.parse_json_response(&response)?;

        let (coin_type_a, coin_type_b) = self.parse_coin_types(&type_str)?;

        // Extract pool state fields: Turbos uses `sqrt_price`
        let sqrt_price_str = fields
            .get("sqrt_price")
            .ok_or_else(|| anyhow!("Turbos pool missing sqrt_price"))?
            .as_str()
            .ok_or_else(|| anyhow!("sqrt_price is not a string"))?;
        let sqrt_price = sqrt_price_str.parse::<u128>()?;

        let liquidity_str = fields
            .get("liquidity")
            .ok_or_else(|| anyhow!("Turbos pool missing liquidity"))?
            .as_str()
            .ok_or_else(|| anyhow!("liquidity is not a string"))?;
        let liquidity = liquidity_str.parse::<u128>()?;

        // Turbos fee rate is sometimes stored as `fee` or `fee_rate` in the fields
        let fee_rate = if let Some(fee_val) = fields.get("fee") {
            if let Some(fee_str) = fee_val.as_str() {
                fee_str.parse::<u64>()?
            } else {
                fee_val.as_u64().unwrap_or_default()
            }
        } else if let Some(fee_rate_val) = fields.get("fee_rate") {
            if let Some(fee_rate_str) = fee_rate_val.as_str() {
                fee_rate_str.parse::<u64>()?
            } else {
                0
            }
        } else {
            0
        };

        // Turbos pause flag: `unlocked` (live) or legacy `is_pause`.
        let is_paused = if let Some(unlocked) = fields.get("unlocked").and_then(|v| v.as_bool()) {
            !unlocked
        } else if let Some(is_pause) = fields.get("is_pause").and_then(|v| v.as_bool()) {
            is_pause
        } else {
            return Err(anyhow!("Turbos pool missing unlocked/is_pause"));
        };

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
            parse_raw_tick_index(&fields, &["tick_current_index", "current_tick_index"])?;
        let tick_spacing = parse_u32_field(&fields, &["tick_spacing"])?;
        let i32_type = fields
            .get("tick_current_index")
            .or_else(|| fields.get("current_tick_index"))
            .and_then(|v| v.get("type"))
            .and_then(|t| t.as_str())
            .unwrap_or(
                "0x91bfbc386a41afcfd9b2533058d7e915a1d3829089cc268ff4333d54d6339ca1::i32::I32",
            );

        let ticks =
            collect_turbos_ticks(client, pool_id, i32_type, current_tick_index, tick_spacing)
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
    use crate::sui_int::encode_raw_i32;
    use serde_json::json;

    #[tokio::test]
    async fn test_turbos_collector_json_rpc() {
        let mock_client = MockSuiClient::new();

        *mock_client.get_object_mock.lock().unwrap() = Box::new(|pool_id| {
            assert_eq!(pool_id, "0x_turbos_pool");
            Ok(json!({
                "data": {
                    "objectId": "0x_turbos_pool",
                    "content": {
                        "type": "0x5c78::pool::Pool<0x2::sui::SUI, 0x5d4b3::coin::COIN, 0x1::fee::Fee3000>",
                        "fields": {
                            "sqrt_price": "79228162514264337593543950336",
                            "liquidity": "30000000",
                            "is_pause": false,
                            "fee": "3000"
                        }
                    }
                }
            }))
        });

        let collector = TurbosCollector::new();
        let pool_state = collector
            .fetch_pool(&mock_client, "0x_turbos_pool")
            .await
            .unwrap();

        assert_eq!(pool_state.pool_id, "0x_turbos_pool");
        assert_eq!(pool_state.dex_name, "Turbos");
        assert_eq!(pool_state.coin_type_a, "0x2::sui::SUI");
        assert_eq!(pool_state.coin_type_b, "0x5d4b3::coin::COIN");
        assert_eq!(pool_state.sqrt_price, 79228162514264337593543950336);
        assert_eq!(pool_state.liquidity, 30000000);
        assert_eq!(pool_state.fee_rate, 3000);
        assert!(!pool_state.is_paused);
    }

    #[tokio::test]
    async fn test_turbos_fetch_tick_data() {
        let mock_client = MockSuiClient::new();
        let _i32_type = "0x91bfbc::i32::I32";

        *mock_client.get_object_mock.lock().unwrap() = Box::new(|obj_id| {
            if obj_id == "0x_turbos_pool" {
                return Ok(json!({
                    "data": {
                        "content": {
                            "type": "0x91bfbc::pool::Pool<...>",
                            "fields": {
                                "tick_current_index": {
                                    "type": "0x91bfbc::i32::I32",
                                    "fields": { "bits": encode_raw_i32(-60) }
                                },
                                "tick_spacing": 60
                            }
                        }
                    }
                }));
            }
            Err(anyhow!("unexpected object {}", obj_id))
        });

        *mock_client.get_dynamic_field_object_mock.lock().unwrap() = Box::new(|parent_id, name| {
            assert_eq!(parent_id, "0x_turbos_pool");
            let bits = name
                .pointer("/value/bits")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u32;
            if bits == encode_raw_i32(-60) {
                return Ok(json!({
                    "data": {
                        "content": {
                            "fields": {
                                "value": {
                                    "fields": {
                                        "liquidity_net": { "fields": { "bits": "400" } }
                                    }
                                }
                            }
                        }
                    }
                }));
            }
            Err(anyhow!("tick not initialized"))
        });

        let collector = TurbosCollector::new();
        let tick_data = collector
            .fetch_tick_data(&mock_client, "0x_turbos_pool")
            .await
            .unwrap();

        assert_eq!(tick_data.current_tick_index, -60);
        assert_eq!(tick_data.tick_spacing, 60);
        assert_eq!(tick_data.ticks.len(), 1);
        assert_eq!(tick_data.ticks[0].tick_index, -60);
        assert_eq!(tick_data.ticks[0].liquidity_net, 100);
    }
}
