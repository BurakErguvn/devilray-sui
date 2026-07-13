//! Convert symbolic [`PtbTransaction`] + resolved chain metadata into a canonical
//! [`sui_sdk_types::Transaction`] via [`sui_transaction_builder::TransactionBuilder`].

use std::collections::HashMap;
use std::str::FromStr;

use sui_sdk_types::bcs::ToBcs;
use sui_sdk_types::{Address, Digest, Identifier, Transaction, TypeTag};
use sui_transaction_builder::{Argument, Function, ObjectInput, TransactionBuilder};
use thiserror::Error;

use crate::models::{ObjectRefSummary, PtbArgument, PtbCommand, PtbTransaction};

/// Errors produced while mapping a symbolic PTB into a canonical Sui transaction.
#[derive(Debug, Error)]
pub enum TxBuildError {
    #[error("failed to parse address `{value}`: {source}")]
    InvalidAddress {
        value: String,
        #[source]
        source: sui_sdk_types::AddressParseError,
    },
    #[error("failed to parse digest `{value}`: {source}")]
    InvalidDigest {
        value: String,
        #[source]
        source: sui_sdk_types::DigestParseError,
    },
    #[error("failed to parse identifier `{value}`: {source}")]
    InvalidIdentifier {
        value: String,
        #[source]
        source: sui_sdk_types::TypeParseError,
    },
    #[error("failed to parse type tag `{value}`: {source}")]
    InvalidTypeTag {
        value: String,
        #[source]
        source: sui_sdk_types::TypeParseError,
    },
    #[error("missing object metadata for `{object_id}`")]
    MissingObjectMeta { object_id: String },
    #[error("shared object `{object_id}` is missing initial_shared_version")]
    MissingSharedVersion { object_id: String },
    #[error("unresolved InputCoin argument — resolve to Object before build")]
    UnresolvedInputCoin,
    #[error("unsupported or unparsable pure value `{value}`")]
    InvalidPureValue { value: String },
    #[error("command result index {index} is out of range (have {len})")]
    MissingCommandResult { index: u16, len: usize },
    #[error("command {index} does not produce a usable result")]
    CommandHasNoResult { index: u16 },
    #[error("SplitCoins amount argument must be a u64-compatible pure value")]
    InvalidSplitAmount,
    #[error("gas_coins must not be empty")]
    EmptyGasCoins,
    #[error("builder error: {0}")]
    Builder(#[from] sui_transaction_builder::Error),
    #[error("BCS encode/decode error: {0}")]
    Bcs(String),
}

/// Ownership kind for a resolved on-chain object.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OwnerKind {
    Owned,
    Shared,
    Immutable,
}

/// Resolved object metadata needed to form an [`ObjectInput`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectMeta {
    pub object_id: String,
    pub version: u64,
    pub digest: String,
    pub owner_kind: OwnerKind,
    pub initial_shared_version: Option<u64>,
    pub mutable: bool,
}

/// A resolved coin object available as a transaction input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedCoin {
    pub object_id: String,
    pub version: u64,
    pub digest: String,
    pub balance: u64,
    pub coin_type: String,
}

/// Fully resolved inputs for offline canonical transaction construction.
#[derive(Debug, Clone)]
pub struct CanonicalBuildInput {
    pub symbolic: PtbTransaction,
    pub sender: String,
    pub gas_price: u64,
    pub gas_budget: u64,
    pub gas_coins: Vec<ResolvedCoin>,
    pub input_coins: HashMap<String, ResolvedCoin>,
    pub object_meta: HashMap<String, ObjectMeta>,
}

/// Canonical unsigned transaction plus wallet-facing summaries.
#[derive(Debug, Clone)]
pub struct CanonicalBuildOutput {
    pub transaction: Transaction,
    pub transaction_data_bcs_base64: String,
    pub transaction_digest: String,
    pub gas_budget: u64,
    pub gas_price: u64,
    pub object_refs: Vec<ObjectRefSummary>,
}

/// Tracks per-command result roots so [`PtbArgument::Result`] /
/// [`PtbArgument::NestedResult`] can be remapped onto builder [`Argument`]s.
struct ResultTracker {
    /// Root argument for each command index. `None` when the command yields no result
    /// (e.g. TransferObjects / MergeCoins).
    roots: Vec<Option<Argument>>,
}

impl ResultTracker {
    fn new() -> Self {
        Self { roots: Vec::new() }
    }

    fn push_root(&mut self, root: Option<Argument>) {
        self.roots.push(root);
    }

    fn result(&self, cmd: u16) -> Result<Argument, TxBuildError> {
        let root = self
            .roots
            .get(cmd as usize)
            .ok_or(TxBuildError::MissingCommandResult {
                index: cmd,
                len: self.roots.len(),
            })?;
        root.ok_or(TxBuildError::CommandHasNoResult { index: cmd })
    }

    /// NestedResult(cmd, idx) → `roots[cmd].to_nested(idx + 1)[idx]`.
    fn nested(&self, cmd: u16, idx: u16) -> Result<Argument, TxBuildError> {
        let root = self.result(cmd)?;
        let nested = root.to_nested(idx as usize + 1);
        Ok(nested[idx as usize])
    }
}

/// Build a canonical unsigned [`Transaction`] from a symbolic PTB + resolved metadata.
pub fn build_canonical_transaction(
    input: &CanonicalBuildInput,
) -> Result<CanonicalBuildOutput, TxBuildError> {
    if input.gas_coins.is_empty() {
        return Err(TxBuildError::EmptyGasCoins);
    }

    let sender = parse_address(&input.sender)?;
    let mut builder = TransactionBuilder::new();
    builder.set_sender(sender);
    builder.set_gas_budget(input.gas_budget);
    builder.set_gas_price(input.gas_price);

    let mut gas_objects = Vec::with_capacity(input.gas_coins.len());
    for coin in &input.gas_coins {
        gas_objects.push(ObjectInput::owned(
            parse_address(&coin.object_id)?,
            coin.version,
            parse_digest(&coin.digest)?,
        ));
    }
    builder.add_gas_objects(gas_objects);

    let mut results = ResultTracker::new();
    for command in &input.symbolic.commands {
        apply_command(&mut builder, command, input, &mut results)?;
    }

    let transaction = builder.try_build()?;
    let transaction_data_bcs_base64 = transaction
        .to_bcs_base64()
        .map_err(|e| TxBuildError::Bcs(e.to_string()))?;
    let transaction_digest = transaction.digest().to_base58();

    Ok(CanonicalBuildOutput {
        transaction,
        transaction_data_bcs_base64,
        transaction_digest,
        gas_budget: input.gas_budget,
        gas_price: input.gas_price,
        object_refs: collect_object_refs(input),
    })
}

fn apply_command(
    builder: &mut TransactionBuilder,
    command: &PtbCommand,
    input: &CanonicalBuildInput,
    results: &mut ResultTracker,
) -> Result<(), TxBuildError> {
    match command {
        PtbCommand::SplitCoins { coin, amounts } => {
            let coin_arg = map_argument(builder, coin, input, results)?;
            let amount_args: Result<Vec<_>, _> = amounts
                .iter()
                .map(|a| map_split_amount(builder, a, input, results))
                .collect();
            let amount_args = amount_args?;
            let nested = builder.split_coins(coin_arg, amount_args);
            // `split_coins` already nests; keep first element as the command root so
            // subsequent NestedResult lookups can re-expand via `to_nested`.
            results.push_root(nested.first().copied());
        }
        PtbCommand::MergeCoins {
            destination,
            sources,
        } => {
            let dest = map_argument(builder, destination, input, results)?;
            let source_args: Result<Vec<_>, _> = sources
                .iter()
                .map(|s| map_argument(builder, s, input, results))
                .collect();
            builder.merge_coins(dest, source_args?);
            results.push_root(None);
        }
        PtbCommand::TransferObjects { objects, address } => {
            let object_args: Result<Vec<_>, _> = objects
                .iter()
                .map(|o| map_argument(builder, o, input, results))
                .collect();
            let recipient = map_argument(builder, address, input, results)?;
            builder.transfer_objects(object_args?, recipient);
            results.push_root(None);
        }
        PtbCommand::MoveCall {
            package,
            module,
            function,
            type_arguments,
            arguments,
        } => {
            let package_addr = parse_address(package)?;
            let module_id = parse_identifier(module)?;
            let function_id = parse_identifier(function)?;
            let type_args: Result<Vec<_>, _> =
                type_arguments.iter().map(|t| parse_type_tag(t)).collect();
            let fn_ref =
                Function::new(package_addr, module_id, function_id).with_type_args(type_args?);
            let args: Result<Vec<_>, _> = arguments
                .iter()
                .map(|a| map_argument(builder, a, input, results))
                .collect();
            let result = builder.move_call(fn_ref, args?);
            results.push_root(Some(result));
        }
        PtbCommand::MakeMoveVec { type_tag, elements } => {
            let type_ = match type_tag {
                Some(t) => Some(parse_type_tag(t)?),
                None => None,
            };
            let elems: Result<Vec<_>, _> = elements
                .iter()
                .map(|e| map_argument(builder, e, input, results))
                .collect();
            let result = builder.make_move_vec(type_, elems?);
            results.push_root(Some(result));
        }
    }
    Ok(())
}

fn map_split_amount(
    builder: &mut TransactionBuilder,
    arg: &PtbArgument,
    input: &CanonicalBuildInput,
    results: &ResultTracker,
) -> Result<Argument, TxBuildError> {
    match arg {
        PtbArgument::U64(v) => Ok(builder.pure(v)),
        PtbArgument::Pure(s) => {
            let v: u64 = s.parse().map_err(|_| TxBuildError::InvalidSplitAmount)?;
            Ok(builder.pure(&v))
        }
        other => {
            // Allow Result/NestedResult only if somehow pre-built; otherwise reject.
            match other {
                PtbArgument::Result(_) | PtbArgument::NestedResult(_, _) => {
                    map_argument(builder, other, input, results)
                }
                _ => Err(TxBuildError::InvalidSplitAmount),
            }
        }
    }
}

fn map_argument(
    builder: &mut TransactionBuilder,
    arg: &PtbArgument,
    input: &CanonicalBuildInput,
    results: &ResultTracker,
) -> Result<Argument, TxBuildError> {
    match arg {
        PtbArgument::GasCoin => Ok(builder.gas()),
        PtbArgument::InputCoin => Err(TxBuildError::UnresolvedInputCoin),
        PtbArgument::Result(i) => results.result(*i),
        PtbArgument::NestedResult(i, j) => results.nested(*i, *j),
        PtbArgument::Object(object_id) => {
            let meta = input.object_meta.get(object_id).ok_or_else(|| {
                TxBuildError::MissingObjectMeta {
                    object_id: object_id.clone(),
                }
            })?;
            Ok(builder.object(object_input_from_meta(meta)?))
        }
        PtbArgument::Bool(v) => Ok(builder.pure(v)),
        PtbArgument::U64(v) => Ok(builder.pure(v)),
        PtbArgument::U128(v) => Ok(builder.pure(v)),
        PtbArgument::Address(s) => {
            let addr = parse_address(s)?;
            Ok(builder.pure(&addr))
        }
        PtbArgument::Pure(s) => map_pure_string(builder, s),
    }
}

fn map_pure_string(
    builder: &mut TransactionBuilder,
    value: &str,
) -> Result<Argument, TxBuildError> {
    if let Ok(v) = value.parse::<bool>() {
        return Ok(builder.pure(&v));
    }
    if let Ok(v) = value.parse::<u64>() {
        return Ok(builder.pure(&v));
    }
    if let Ok(v) = value.parse::<u128>() {
        return Ok(builder.pure(&v));
    }
    if let Ok(addr) = Address::from_str(value) {
        return Ok(builder.pure(&addr));
    }
    Err(TxBuildError::InvalidPureValue {
        value: value.to_string(),
    })
}

fn object_input_from_meta(meta: &ObjectMeta) -> Result<ObjectInput, TxBuildError> {
    let object_id = parse_address(&meta.object_id)?;
    match meta.owner_kind {
        OwnerKind::Owned => Ok(ObjectInput::owned(
            object_id,
            meta.version,
            parse_digest(&meta.digest)?,
        )),
        OwnerKind::Immutable => Ok(ObjectInput::immutable(
            object_id,
            meta.version,
            parse_digest(&meta.digest)?,
        )),
        OwnerKind::Shared => {
            let initial =
                meta.initial_shared_version
                    .ok_or_else(|| TxBuildError::MissingSharedVersion {
                        object_id: meta.object_id.clone(),
                    })?;
            Ok(ObjectInput::shared(object_id, initial, meta.mutable))
        }
    }
}

fn collect_object_refs(input: &CanonicalBuildInput) -> Vec<ObjectRefSummary> {
    let mut refs = Vec::new();
    for coin in &input.gas_coins {
        refs.push(ObjectRefSummary {
            object_id: coin.object_id.clone(),
            version: coin.version,
            digest: coin.digest.clone(),
            owner_kind: "Owned".to_string(),
        });
    }
    for meta in input.object_meta.values() {
        let owner_kind = match meta.owner_kind {
            OwnerKind::Owned => "Owned",
            OwnerKind::Shared => "Shared",
            OwnerKind::Immutable => "Immutable",
        };
        refs.push(ObjectRefSummary {
            object_id: meta.object_id.clone(),
            version: meta.version,
            digest: meta.digest.clone(),
            owner_kind: owner_kind.to_string(),
        });
    }
    refs
}

fn parse_address(value: &str) -> Result<Address, TxBuildError> {
    Address::from_str(value).map_err(|source| TxBuildError::InvalidAddress {
        value: value.to_string(),
        source,
    })
}

fn parse_digest(value: &str) -> Result<Digest, TxBuildError> {
    Digest::from_base58(value)
        .or_else(|_| Digest::from_str(value))
        .map_err(|source| TxBuildError::InvalidDigest {
            value: value.to_string(),
            source,
        })
}

fn parse_identifier(value: &str) -> Result<Identifier, TxBuildError> {
    Identifier::new(value).map_err(|source| TxBuildError::InvalidIdentifier {
        value: value.to_string(),
        source,
    })
}

fn parse_type_tag(value: &str) -> Result<TypeTag, TxBuildError> {
    TypeTag::from_str(value).map_err(|source| TxBuildError::InvalidTypeTag {
        value: value.to_string(),
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use sui_sdk_types::bcs::FromBcs;

    fn fake_digest() -> String {
        Digest::ZERO.to_base58()
    }

    #[test]
    fn build_split_and_transfer_round_trips_bcs() {
        let sender = Address::ZERO.to_string();
        let gas_id = Address::from_str("0xA").unwrap().to_string();
        let recipient = Address::from_str("0xB").unwrap().to_string();
        let digest = fake_digest();

        let symbolic = PtbTransaction {
            sender: sender.clone(),
            commands: vec![
                PtbCommand::SplitCoins {
                    coin: PtbArgument::GasCoin,
                    amounts: vec![PtbArgument::U64(1_000_000)],
                },
                PtbCommand::TransferObjects {
                    objects: vec![PtbArgument::NestedResult(0, 0)],
                    address: PtbArgument::Address(recipient),
                },
            ],
        };

        let input = CanonicalBuildInput {
            symbolic,
            sender,
            gas_price: 1_000,
            gas_budget: 5_000_000,
            gas_coins: vec![ResolvedCoin {
                object_id: gas_id,
                version: 1,
                digest,
                balance: 10_000_000_000,
                coin_type: "0x2::sui::SUI".to_string(),
            }],
            input_coins: HashMap::new(),
            object_meta: HashMap::new(),
        };

        let output = build_canonical_transaction(&input).expect("build should succeed");
        assert!(!output.transaction_data_bcs_base64.is_empty());
        assert!(!output.transaction_digest.is_empty());
        assert_eq!(output.gas_budget, 5_000_000);
        assert_eq!(output.gas_price, 1_000);
        assert!(!output.object_refs.is_empty());

        let decoded = Transaction::from_bcs_base64(&output.transaction_data_bcs_base64)
            .expect("BCS base64 should decode");
        assert_eq!(decoded, output.transaction);

        let reencoded = decoded.to_bcs_base64().expect("re-encode");
        assert_eq!(reencoded, output.transaction_data_bcs_base64);
    }

    #[test]
    fn multi_output_split_builds_canonical_bcs() {
        let sender = Address::ZERO.to_string();
        let recipient = Address::from_str("0xB").unwrap().to_string();
        let symbolic = PtbTransaction {
            sender: sender.clone(),
            commands: vec![
                PtbCommand::SplitCoins {
                    coin: PtbArgument::GasCoin,
                    amounts: vec![PtbArgument::U64(400_000), PtbArgument::U64(600_000)],
                },
                PtbCommand::TransferObjects {
                    objects: vec![
                        PtbArgument::NestedResult(0, 0),
                        PtbArgument::NestedResult(0, 1),
                    ],
                    address: PtbArgument::Address(recipient),
                },
            ],
        };
        let input = CanonicalBuildInput {
            symbolic,
            sender,
            gas_price: 1_000,
            gas_budget: 5_000_000,
            gas_coins: vec![ResolvedCoin {
                object_id: Address::from_str("0xA").unwrap().to_string(),
                version: 1,
                digest: fake_digest(),
                balance: 10_000_000_000,
                coin_type: "0x2::sui::SUI".to_string(),
            }],
            input_coins: HashMap::new(),
            object_meta: HashMap::new(),
        };

        let output = build_canonical_transaction(&input).expect("multi split build");
        let decoded =
            Transaction::from_bcs_base64(&output.transaction_data_bcs_base64).expect("decode");
        assert_eq!(decoded, output.transaction);
        assert_eq!(decoded.digest().to_base58(), output.transaction_digest);
    }

    #[test]
    fn input_coin_without_resolution_errors() {
        let sender = Address::ZERO.to_string();
        let gas_id = Address::from_str("0xA").unwrap().to_string();

        let symbolic = PtbTransaction {
            sender: sender.clone(),
            commands: vec![PtbCommand::SplitCoins {
                coin: PtbArgument::InputCoin,
                amounts: vec![PtbArgument::U64(1)],
            }],
        };

        let input = CanonicalBuildInput {
            symbolic,
            sender,
            gas_price: 1_000,
            gas_budget: 5_000_000,
            gas_coins: vec![ResolvedCoin {
                object_id: gas_id,
                version: 1,
                digest: fake_digest(),
                balance: 10_000_000_000,
                coin_type: "0x2::sui::SUI".to_string(),
            }],
            input_coins: HashMap::new(),
            object_meta: HashMap::new(),
        };

        let err = build_canonical_transaction(&input).unwrap_err();
        assert!(matches!(err, TxBuildError::UnresolvedInputCoin));
    }

    #[test]
    fn pure_string_address_transfer_works() {
        let sender = Address::ZERO.to_string();
        let gas_id = Address::from_str("0xA").unwrap().to_string();
        let recipient = Address::from_str("0xC").unwrap().to_string();

        let symbolic = PtbTransaction {
            sender: sender.clone(),
            commands: vec![
                PtbCommand::SplitCoins {
                    coin: PtbArgument::GasCoin,
                    amounts: vec![PtbArgument::Pure("500".to_string())],
                },
                PtbCommand::TransferObjects {
                    objects: vec![PtbArgument::NestedResult(0, 0)],
                    address: PtbArgument::Pure(recipient),
                },
            ],
        };

        let input = CanonicalBuildInput {
            symbolic,
            sender,
            gas_price: 1_000,
            gas_budget: 5_000_000,
            gas_coins: vec![ResolvedCoin {
                object_id: gas_id,
                version: 1,
                digest: fake_digest(),
                balance: 10_000_000_000,
                coin_type: "0x2::sui::SUI".to_string(),
            }],
            input_coins: HashMap::new(),
            object_meta: HashMap::new(),
        };

        let output = build_canonical_transaction(&input).expect("build");
        let round_trip =
            Transaction::from_bcs_base64(&output.transaction_data_bcs_base64).expect("decode");
        assert_eq!(round_trip, output.transaction);
    }
}
