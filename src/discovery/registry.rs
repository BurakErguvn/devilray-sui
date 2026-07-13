//! DEX-specific pool discovery and swap execution configuration (canonical mainnet contracts).

/// Per-DEX GraphQL object/event discovery configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DexDiscoverySpec {
    pub dex_name: &'static str,
    /// Deployment package id (pool + factory modules).
    pub package_id: &'static str,
    /// Fully-qualified pool struct type for `objects(filter: { type })` (no generics).
    pub pool_object_type: &'static str,
    /// Fully-qualified Move event types used with `EventFilter.type`.
    pub create_pool_event_types: &'static [&'static str],
    /// Move module name hosting the pool struct (`pool`, etc.).
    pub pool_type_module: &'static str,
    /// Required JSON fields on create-pool events (contract drift detection).
    pub required_event_fields: &'static [&'static str],
}

/// Shared/immutable objects required to build executable swap transactions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DexSwapContract {
    pub dex_name: &'static str,
    /// Package containing the swap / flash_swap entry functions.
    pub package_id: &'static str,
    /// Module hosting the primary swap entry (`pool` or `trade`).
    pub swap_module: &'static str,
    /// Optional global config / version object ids (shared).
    pub shared_objects: &'static [&'static str],
}

pub const OBJECT_BOOTSTRAP_SOURCE_KEY: &str = "object_bootstrap";

pub const CETUS_DISCOVERY_PACKAGE: &str =
    "0x1eabed72c53feb3805120a081dc15963c204dc8d091542592abaf7a35689b2fb";
/// Latest upgraded Cetus CLMM package (`published-at`) for MoveCall targets.
pub const CETUS_SWAP_PACKAGE: &str =
    "0x25ebb9a7c50eb17b3fa9c5a30fb8b5ad8f97caaf4928943acbcff7153dfee5e3";
pub const TURBOS_DISCOVERY_PACKAGE: &str =
    "0x91bfbc386a41afcfd9b2533058d7e915a1d3829089cc268ff4333d54d6339ca1";
/// Latest upgraded Turbos package (`published-at`) used for MoveCall targets.
/// Type origins remain on [`TURBOS_DISCOVERY_PACKAGE`]; calling the original package
/// aborts in `check_version` (expects v1 while Versioned is at v18+).
pub const TURBOS_SWAP_PACKAGE: &str =
    "0xa5a0c25c79e428eba04fb98b3fb2a34db45ab26d4c8faf0d7e39d66a63891e64";
pub const MAGMA_DISCOVERY_PACKAGE: &str =
    "0x4a35d3dfef55ed3631b7158544c6322a23bc434fe4fca1234cb680ce0505f82d";
/// Magma CLMM `published-at` (matches GlobalConfig.package_version == 4).
pub const MAGMA_SWAP_PACKAGE: &str =
    "0x0acd1d187950450ae3e625375f8067a7802e99a05b6e655e1fec124a0e3c891e";
pub const MOMENTUM_DISCOVERY_PACKAGE: &str =
    "0x70285592c97965e811e0c6f98dccc3a9c2b4ad854b3594faab9597ada267b860";

/// Cetus GlobalConfig (shared). From `config::InitConfigEvent.global_config_id`.
pub const CETUS_GLOBAL_CONFIG: &str =
    "0xdaa46292632c3c4d8f31f23ea0f9b36a28ff3677e9684980e4438403a67a3d8f";
/// Magma GlobalConfig (shared).
pub const MAGMA_GLOBAL_CONFIG: &str =
    "0x4c4e1402401f72c7d8533d0ed8d5f8949da363c7a3319ccef261ffe153d32f8a";
/// Momentum Version object (shared) — `version::Version`.
pub const MOMENTUM_VERSION_OBJECT: &str =
    "0x2375a0b1ec12010aaea3b2545acfa2ad34cfbba03ce4b59f4c39e1e25eed1b2a";
/// Turbos Versioned object (shared) — `pool::Versioned`.
pub const TURBOS_VERSIONED: &str =
    "0xf1cf0e81048df168ebeb1b8030fad24b3e0b53ae827c25053fff0779c1445b6f";
pub const SUI_CLOCK: &str = "0x6";
pub const SUI_FRAMEWORK_PACKAGE: &str = "0x2";

/// Locked discovery specs for the four supported CLMM DEXes (mainnet-verified).
pub static ALL_DISCOVERY_SPECS: &[DexDiscoverySpec] = &[
    DexDiscoverySpec {
        dex_name: "Cetus",
        package_id: CETUS_DISCOVERY_PACKAGE,
        pool_object_type: "0x1eabed72c53feb3805120a081dc15963c204dc8d091542592abaf7a35689b2fb::pool::Pool",
        create_pool_event_types: &[
            "0x1eabed72c53feb3805120a081dc15963c204dc8d091542592abaf7a35689b2fb::factory::CreatePoolEvent",
        ],
        pool_type_module: "pool",
        required_event_fields: &["pool_id", "coin_type_a", "coin_type_b"],
    },
    DexDiscoverySpec {
        dex_name: "Turbos",
        package_id: TURBOS_DISCOVERY_PACKAGE,
        pool_object_type: "0x91bfbc386a41afcfd9b2533058d7e915a1d3829089cc268ff4333d54d6339ca1::pool::Pool",
        create_pool_event_types: &[
            "0x91bfbc386a41afcfd9b2533058d7e915a1d3829089cc268ff4333d54d6339ca1::pool_factory::PoolCreatedEvent",
        ],
        pool_type_module: "pool",
        required_event_fields: &["pool"],
    },
    DexDiscoverySpec {
        dex_name: "Magma",
        package_id: MAGMA_DISCOVERY_PACKAGE,
        pool_object_type: "0x4a35d3dfef55ed3631b7158544c6322a23bc434fe4fca1234cb680ce0505f82d::pool::Pool",
        create_pool_event_types: &[
            "0x4a35d3dfef55ed3631b7158544c6322a23bc434fe4fca1234cb680ce0505f82d::factory::CreatePoolEvent",
        ],
        pool_type_module: "pool",
        required_event_fields: &["pool_id", "coin_type_a", "coin_type_b"],
    },
    DexDiscoverySpec {
        dex_name: "Momentum",
        package_id: MOMENTUM_DISCOVERY_PACKAGE,
        pool_object_type: "0x70285592c97965e811e0c6f98dccc3a9c2b4ad854b3594faab9597ada267b860::pool::Pool",
        create_pool_event_types: &[
            "0x70285592c97965e811e0c6f98dccc3a9c2b4ad854b3594faab9597ada267b860::create_pool::PoolCreatedEvent",
        ],
        pool_type_module: "pool",
        required_event_fields: &["pool_id", "type_x", "type_y"],
    },
];

/// Canonical swap packages / shared objects (aligned with discovery packages).
pub static ALL_SWAP_CONTRACTS: &[DexSwapContract] = &[
    DexSwapContract {
        dex_name: "Cetus",
        package_id: CETUS_SWAP_PACKAGE,
        swap_module: "pool",
        shared_objects: &[CETUS_GLOBAL_CONFIG, SUI_CLOCK],
    },
    DexSwapContract {
        dex_name: "Turbos",
        package_id: TURBOS_SWAP_PACKAGE,
        swap_module: "swap_router",
        shared_objects: &[TURBOS_VERSIONED, SUI_CLOCK],
    },
    DexSwapContract {
        dex_name: "Magma",
        package_id: MAGMA_SWAP_PACKAGE,
        swap_module: "pool",
        shared_objects: &[MAGMA_GLOBAL_CONFIG, SUI_CLOCK],
    },
    DexSwapContract {
        dex_name: "Momentum",
        package_id: MOMENTUM_DISCOVERY_PACKAGE,
        swap_module: "trade",
        shared_objects: &[MOMENTUM_VERSION_OBJECT, SUI_CLOCK],
    },
];

pub fn spec_for_dex_name(dex_name: &str) -> Option<&'static DexDiscoverySpec> {
    let lower = dex_name.to_lowercase();
    ALL_DISCOVERY_SPECS
        .iter()
        .find(|s| s.dex_name.to_lowercase() == lower || lower.contains(&s.dex_name.to_lowercase()))
}

pub fn swap_contract_for_dex_name(dex_name: &str) -> Option<&'static DexSwapContract> {
    let lower = dex_name.to_lowercase();
    ALL_SWAP_CONTRACTS
        .iter()
        .find(|s| s.dex_name.to_lowercase() == lower || lower.contains(&s.dex_name.to_lowercase()))
}

/// All shared/immutable object ids that swap builders may reference.
pub fn all_swap_shared_object_ids() -> Vec<&'static str> {
    let mut ids = vec![SUI_CLOCK];
    for spec in ALL_SWAP_CONTRACTS {
        for obj in spec.shared_objects {
            if !ids.contains(obj) {
                ids.push(obj);
            }
        }
    }
    ids
}

fn normalize_object_id(object_id: &str) -> String {
    let hex = object_id
        .trim()
        .trim_start_matches("0x")
        .to_ascii_lowercase();
    format!("0x{hex:0>64}")
}

/// Whether a shared object should be passed as mutable in PTB inputs.
///
/// Clock and DEX global config / version objects are Move `&` references (immutable shared).
/// Pool objects are `&mut` and must be mutable.
pub fn shared_object_is_mutable(object_id: &str) -> bool {
    let id = normalize_object_id(object_id);
    let immutable = [
        normalize_object_id(SUI_CLOCK),
        normalize_object_id(CETUS_GLOBAL_CONFIG),
        normalize_object_id(MAGMA_GLOBAL_CONFIG),
        normalize_object_id(MOMENTUM_VERSION_OBJECT),
        normalize_object_id(TURBOS_VERSIONED),
    ];
    !immutable.iter().any(|known| known == &id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_specs_have_pool_object_type_under_package() {
        for spec in ALL_DISCOVERY_SPECS {
            assert!(spec.pool_object_type.starts_with(spec.package_id));
            assert!(
                spec.pool_object_type
                    .contains(&format!("::{}::Pool", spec.pool_type_module))
            );
            for evt in spec.create_pool_event_types {
                assert!(evt.starts_with(spec.package_id));
            }
        }
    }

    #[test]
    fn swap_contracts_align_with_discovery_packages() {
        for swap in ALL_SWAP_CONTRACTS {
            let discovery = spec_for_dex_name(swap.dex_name).expect("discovery spec");
            match swap.dex_name {
                "Turbos" => {
                    assert_eq!(swap.package_id, TURBOS_SWAP_PACKAGE);
                    assert_eq!(discovery.package_id, TURBOS_DISCOVERY_PACKAGE);
                }
                "Cetus" => {
                    assert_eq!(swap.package_id, CETUS_SWAP_PACKAGE);
                    assert_eq!(discovery.package_id, CETUS_DISCOVERY_PACKAGE);
                }
                "Magma" => {
                    assert_eq!(swap.package_id, MAGMA_SWAP_PACKAGE);
                    assert_eq!(discovery.package_id, MAGMA_DISCOVERY_PACKAGE);
                }
                _ => assert_eq!(swap.package_id, discovery.package_id),
            }
        }
    }
}
