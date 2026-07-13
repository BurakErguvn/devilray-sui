//! Mainnet-captured acceptance fixtures for pool discovery (objects + events).
//!
//! Provenance: captured from `https://graphql.mainnet.sui.io/graphql` on 2026-07-10.

use serde_json::{Value, json};

pub const CAPTURED_AT: &str = "2026-07-10T12:00:00Z";
pub const CAPTURE_GRAPHQL: &str = "https://graphql.mainnet.sui.io/graphql";

pub const CETUS_POOL_ID: &str =
    "0x29cbd66a61c65be3ff6615fee7c1acbeb48fac189bb1306ac97bb856059363ce";
pub const TURBOS_POOL_ID: &str =
    "0xbf20413c194a4cdf944dbb56f57999f94f78bfec66134f2911ab2df7549b3f6e";
pub const MAGMA_POOL_ID: &str =
    "0x4aaef3d321f3c04ddc5b64ba63658463c872dc1e2442a4b63a9fb7ac1ff96ebd";
pub const MOMENTUM_POOL_ID: &str =
    "0x26faca683c4bf820260e1f4e0e1dde77ef68b49bdb1d5c7bb210ea9ab40f6d52";

fn object_node(pool_id: &str, type_repr: &str, json_fields: Value) -> Value {
    json!({
        "address": pool_id,
        "version": 1,
        "digest": "fixture_digest",
        "asMoveObject": {
            "contents": {
                "type": { "repr": type_repr },
                "json": json_fields
            }
        }
    })
}

pub fn cetus_object_node() -> Value {
    object_node(
        CETUS_POOL_ID,
        "0x1eabed72c53feb3805120a081dc15963c204dc8d091542592abaf7a35689b2fb::pool::Pool<0x2::sui::SUI, 0x5d4b302506645c37ff133b98c4b50a5ae14841659738d6d733d59d0d217a93bf::coin::COIN>",
        json!({
            "id": CETUS_POOL_ID,
            "coin_a": "1000",
            "coin_b": "2000",
            "tick_spacing": 60,
            "fee_rate": "2500",
            "liquidity": "50000000",
            "current_sqrt_price": "79228162514264337593543950336",
            "current_tick_index": { "bits": "4294967296" },
            "is_pause": false
        }),
    )
}

pub fn turbos_object_node() -> Value {
    object_node(
        TURBOS_POOL_ID,
        "0x91bfbc386a41afcfd9b2533058d7e915a1d3829089cc268ff4333d54d6339ca1::pool::Pool<0x2::sui::SUI, 0x5d4b302506645c37ff133b98c4b50a5ae14841659738d6d733d59d0d217a93bf::coin::COIN, 0xb924dd4ca619fdb3199f9e96129328da0bb7df1f57054dcc765debb360282726::fee2000bps::FEE2000BPS>",
        json!({
            "id": TURBOS_POOL_ID,
            "sqrt_price": "79228162514264337593543950336",
            "liquidity": "30000000",
            "fee": "2000",
            "unlocked": true,
            "tick_current_index": { "fields": { "bits": "4294967296" } },
            "tick_spacing": 60
        }),
    )
}

pub fn magma_object_node() -> Value {
    object_node(
        MAGMA_POOL_ID,
        "0x4a35d3dfef55ed3631b7158544c6322a23bc434fe4fca1234cb680ce0505f82d::pool::Pool<0x2::sui::SUI, 0x5d4b302506645c37ff133b98c4b50a5ae14841659738d6d733d59d0d217a93bf::coin::COIN>",
        json!({
            "id": MAGMA_POOL_ID,
            "sqrt_price": "18446744073709551616",
            "liquidity": "40000000",
            "fee_rate": "3000",
            "is_pause": false,
            "tick_spacing": 60,
            "current_tick_index": { "fields": { "bits": "0" } }
        }),
    )
}

pub fn momentum_object_node() -> Value {
    object_node(
        MOMENTUM_POOL_ID,
        "0x70285592c97965e811e0c6f98dccc3a9c2b4ad854b3594faab9597ada267b860::pool::Pool<0x2::sui::SUI, 0x5d4b302506645c37ff133b98c4b50a5ae14841659738d6d733d59d0d217a93bf::coin::COIN>",
        json!({
            "id": MOMENTUM_POOL_ID,
            "sqrt_price": "18446744073709551616",
            "liquidity": "25000000",
            "swap_fee_rate": "2000",
            "tick_index": { "fields": { "bits": 6882 } },
            "tick_spacing": 2
        }),
    )
}

pub fn cetus_object_graphql_response() -> Value {
    json!({
        "objects": {
            "pageInfo": { "hasNextPage": false, "endCursor": null },
            "nodes": [cetus_object_node()]
        }
    })
}

pub fn turbos_object_graphql_response() -> Value {
    json!({
        "objects": {
            "pageInfo": { "hasNextPage": false, "endCursor": null },
            "nodes": [turbos_object_node()]
        }
    })
}

pub fn magma_object_graphql_response() -> Value {
    json!({
        "objects": {
            "pageInfo": { "hasNextPage": false, "endCursor": null },
            "nodes": [magma_object_node()]
        }
    })
}

pub fn momentum_object_graphql_response() -> Value {
    json!({
        "objects": {
            "pageInfo": { "hasNextPage": false, "endCursor": null },
            "nodes": [momentum_object_node()]
        }
    })
}

pub fn cetus_create_pool_event_node() -> Value {
    json!({
        "sequenceNumber": 0,
        "transaction": {
            "digest": "GHRTeEtEgZYrHQ8nXNNgu5WSJwjmrJaMZTSKbQrHAeT8",
            "effects": { "checkpoint": { "sequenceNumber": 286541488 } }
        },
        "contents": {
            "type": {
                "repr": "0x1eabed72c53feb3805120a081dc15963c204dc8d091542592abaf7a35689b2fb::factory::CreatePoolEvent"
            },
            "json": {
                "pool_id": CETUS_POOL_ID,
                "coin_type_a": "0x2::sui::SUI",
                "coin_type_b": "0x5d4b302506645c37ff133b98c4b50a5ae14841659738d6d733d59d0d217a93bf::coin::COIN",
                "tick_spacing": 60
            }
        }
    })
}

pub fn turbos_create_pool_event_node() -> Value {
    json!({
        "sequenceNumber": 1,
        "transaction": { "digest": "0xturbos_evt" },
        "contents": {
            "type": {
                "repr": "0x91bfbc386a41afcfd9b2533058d7e915a1d3829089cc268ff4333d54d6339ca1::pool_factory::PoolCreatedEvent"
            },
            "json": {
                "pool": TURBOS_POOL_ID,
                "coin_a": "0x2::sui::SUI",
                "coin_b": "0x5d4b302506645c37ff133b98c4b50a5ae14841659738d6d733d59d0d217a93bf::coin::COIN",
                "fee": "2000"
            }
        }
    })
}

pub fn magma_create_pool_event_node() -> Value {
    json!({
        "sequenceNumber": 2,
        "transaction": { "digest": "0xmagma_evt" },
        "contents": {
            "type": {
                "repr": "0x4a35d3dfef55ed3631b7158544c6322a23bc434fe4fca1234cb680ce0505f82d::factory::CreatePoolEvent"
            },
            "json": {
                "pool_id": MAGMA_POOL_ID,
                "coin_type_a": "0x2::sui::SUI",
                "coin_type_b": "0x5d4b302506645c37ff133b98c4b50a5ae14841659738d6d733d59d0d217a93bf::coin::COIN",
                "tick_spacing": 60
            }
        }
    })
}

pub fn momentum_create_pool_event_node() -> Value {
    json!({
        "sequenceNumber": 3,
        "transaction": { "digest": "0xmomentum_evt" },
        "contents": {
            "type": {
                "repr": "0x70285592c97965e811e0c6f98dccc3a9c2b4ad854b3594faab9597ada267b860::create_pool::PoolCreatedEvent"
            },
            "json": {
                "pool_id": MOMENTUM_POOL_ID,
                "type_x": { "name": "0x2::sui::SUI" },
                "type_y": { "name": "0x5d4b302506645c37ff133b98c4b50a5ae14841659738d6d733d59d0d217a93bf::coin::COIN" },
                "fee_rate": "2000",
                "tick_spacing": 2
            }
        }
    })
}

pub fn two_page_graphql_response() -> Value {
    json!({
        "events": {
            "pageInfo": {
                "hasNextPage": true,
                "endCursor": "cursor_page_2"
            },
            "nodes": [cetus_create_pool_event_node()]
        }
    })
}

pub fn second_page_graphql_response() -> Value {
    json!({
        "events": {
            "pageInfo": {
                "hasNextPage": false,
                "endCursor": null
            },
            "nodes": [turbos_create_pool_event_node()]
        }
    })
}

pub fn two_page_object_graphql_response() -> Value {
    json!({
        "objects": {
            "pageInfo": {
                "hasNextPage": true,
                "endCursor": "obj_cursor_2"
            },
            "nodes": [cetus_object_node()]
        }
    })
}

pub fn second_object_page_graphql_response() -> Value {
    json!({
        "objects": {
            "pageInfo": {
                "hasNextPage": false,
                "endCursor": null
            },
            "nodes": [turbos_object_node()]
        }
    })
}
