// Copyright (c) MangoNet Labs Ltd.
// SPDX-License-Identifier: Apache-2.0

use anyhow::anyhow;
use arc_swap::Guard;
use async_trait::async_trait;
use move_core_types::language_storage::TypeTag;
use mango_metrics::spawn_monitored_task;
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use mgo_core::authority::authority_per_epoch_store::AuthorityPerEpochStore;
use mgo_core::authority::AuthorityState;
use mgo_core::in_mem_execution_cache::ExecutionCacheRead;
use mgo_core::subscription_handler::SubscriptionHandler;
use mgo_json_rpc_types::{
    Coin as MgoCoin, DevInspectResults, DryRunTransactionBlockResponse, EventFilter, MgoEvent,
    MgoObjectDataFilter, TransactionFilter,
};
use mgo_storage::indexes::TotalBalance;
use mgo_storage::key_value_store::{
    KVStoreCheckpointData, KVStoreTransactionData, TransactionKeyValueStore,
    TransactionKeyValueStoreTrait,
};
use mgo_types::base_types::{
    MoveObjectType, ObjectID, ObjectInfo, ObjectRef, SequenceNumber, MgoAddress,
};
use mgo_types::committee::{Committee, EpochId};
use mgo_types::digests::{ChainIdentifier, TransactionDigest, TransactionEventsDigest};
use mgo_types::dynamic_field::DynamicFieldInfo;
use mgo_types::effects::TransactionEffects;
use mgo_types::error::{MgoError, UserInputError};
use mgo_types::event::EventID;
use mgo_types::governance::StakedMgo;
use mgo_types::messages_checkpoint::{
    CheckpointContents, CheckpointContentsDigest, CheckpointDigest, CheckpointSequenceNumber,
    VerifiedCheckpoint,
};
use mgo_types::object::{Object, ObjectRead, PastObjectRead};
use mgo_types::storage::{BackingPackageStore, ObjectStore, WriteKind};
use mgo_types::mgo_serde::BigInt;
use mgo_types::mgo_system_state::MgoSystemState;
use mgo_types::transaction::{Transaction, TransactionData, TransactionKind};
use thiserror::Error;
use tokio::task::JoinError;

#[cfg(test)]
use mockall::automock;

use crate::ObjectProvider;

pub type StateReadResult<T = ()> = Result<T, StateReadError>;

/// Trait for AuthorityState methods commonly used by at least two api.
#[cfg_attr(test, automock)]
#[async_trait]
pub trait StateRead: Send + Sync {
    async fn multi_get(
        &self,
        transactions: &[TransactionDigest],
        effects: &[TransactionDigest],
        events: &[TransactionEventsDigest],
    ) -> StateReadResult<KVStoreTransactionData>;

    async fn multi_get_checkpoints(
        &self,
        checkpoint_summaries: &[CheckpointSequenceNumber],
        checkpoint_contents: &[CheckpointSequenceNumber],
        checkpoint_summaries_by_digest: &[CheckpointDigest],
        checkpoint_contents_by_digest: &[CheckpointContentsDigest],
    ) -> StateReadResult<KVStoreCheckpointData>;

    fn get_object_read(&self, object_id: &ObjectID) -> StateReadResult<ObjectRead>;

    fn get_past_object_read(
        &self,
        object_id: &ObjectID,
        version: SequenceNumber,
    ) -> StateReadResult<PastObjectRead>;

    async fn get_object(&self, object_id: &ObjectID) -> StateReadResult<Option<Object>>;

    fn load_epoch_store_one_call_per_task(&self) -> Guard<Arc<AuthorityPerEpochStore>>;

    fn get_dynamic_fields(
        &self,
        owner: ObjectID,
        cursor: Option<ObjectID>,
        limit: usize,
    ) -> StateReadResult<Vec<(ObjectID, DynamicFieldInfo)>>;

    fn get_cache_reader(&self) -> Arc<dyn ExecutionCacheRead>;

    fn get_object_store(&self) -> Arc<dyn ObjectStore>;

    fn get_backing_package_store(&self) -> Arc<dyn BackingPackageStore>;

    fn get_owner_objects(
        &self,
        owner: MgoAddress,
        cursor: Option<ObjectID>,
        filter: Option<MgoObjectDataFilter>,
    ) -> StateReadResult<Vec<ObjectInfo>>;

    async fn query_events(
        &self,
        kv_store: &Arc<TransactionKeyValueStore>,
        query: EventFilter,
        // If `Some`, the query will start from the next item after the specified cursor
        cursor: Option<EventID>,
        limit: usize,
        descending: bool,
    ) -> StateReadResult<Vec<MgoEvent>>;

    // transaction_execution_api
    #[allow(clippy::type_complexity)]
    async fn dry_exec_transaction(
        &self,
        transaction: TransactionData,
        transaction_digest: TransactionDigest,
    ) -> StateReadResult<(
        DryRunTransactionBlockResponse,
        BTreeMap<ObjectID, (ObjectRef, Object, WriteKind)>,
        TransactionEffects,
        Option<ObjectID>,
    )>;

    async fn dev_inspect_transaction_block(
        &self,
        sender: MgoAddress,
        transaction_kind: TransactionKind,
        gas_price: Option<u64>,
        gas_budget: Option<u64>,
        gas_sponsor: Option<MgoAddress>,
        gas_objects: Option<Vec<ObjectRef>>,
        show_raw_txn_data_and_effects: Option<bool>,
        skip_checks: Option<bool>,
    ) -> StateReadResult<DevInspectResults>;

    // indexer_api
    fn get_subscription_handler(&self) -> Arc<SubscriptionHandler>;

    fn get_owner_objects_with_limit(
        &self,
        owner: MgoAddress,
        cursor: Option<ObjectID>,
        limit: usize,
        filter: Option<MgoObjectDataFilter>,
    ) -> StateReadResult<Vec<ObjectInfo>>;

    async fn get_transactions(
        &self,
        kv_store: &Arc<TransactionKeyValueStore>,
        filter: Option<TransactionFilter>,
        cursor: Option<TransactionDigest>,
        limit: Option<usize>,
        reverse: bool,
    ) -> StateReadResult<Vec<TransactionDigest>>;

    fn get_dynamic_field_object_id(
        &self,
        owner: ObjectID,
        name_type: TypeTag,
        name_bcs_bytes: &[u8],
    ) -> StateReadResult<Option<ObjectID>>;

    // governance_api
    async fn get_staked_mgo(&self, owner: MgoAddress) -> StateReadResult<Vec<StakedMgo>>;
    fn get_system_state(&self) -> StateReadResult<MgoSystemState>;
    fn get_or_latest_committee(&self, epoch: Option<BigInt<u64>>) -> StateReadResult<Committee>;

    // coin_api
    fn find_publish_txn_digest(&self, package_id: ObjectID) -> StateReadResult<TransactionDigest>;
    fn get_owned_coins(
        &self,
        owner: MgoAddress,
        cursor: (String, ObjectID),
        limit: usize,
        one_coin_type_only: bool,
    ) -> StateReadResult<Vec<MgoCoin>>;
    async fn get_executed_transaction_and_effects(
        &self,
        digest: TransactionDigest,
        kv_store: Arc<TransactionKeyValueStore>,
    ) -> StateReadResult<(Transaction, TransactionEffects)>;
    async fn get_balance(
        &self,
        owner: MgoAddress,
        coin_type: TypeTag,
    ) -> StateReadResult<TotalBalance>;
    async fn get_all_balance(
        &self,
        owner: MgoAddress,
    ) -> StateReadResult<Arc<HashMap<TypeTag, TotalBalance>>>;

    // read_api
    fn get_verified_checkpoint_by_sequence_number(
        &self,
        sequence_number: CheckpointSequenceNumber,
    ) -> StateReadResult<VerifiedCheckpoint>;

    fn get_checkpoint_contents(
        &self,
        digest: CheckpointContentsDigest,
    ) -> StateReadResult<CheckpointContents>;

    fn get_verified_checkpoint_summary_by_digest(
        &self,
        digest: CheckpointDigest,
    ) -> StateReadResult<VerifiedCheckpoint>;

    fn deprecated_multi_get_transaction_checkpoint(
        &self,
        digests: &[TransactionDigest],
    ) -> StateReadResult<Vec<Option<(EpochId, CheckpointSequenceNumber)>>>;

    fn deprecated_get_transaction_checkpoint(
        &self,
        digest: &TransactionDigest,
    ) -> StateReadResult<Option<(EpochId, CheckpointSequenceNumber)>>;

    fn multi_get_checkpoint_by_sequence_number(
        &self,
        sequence_numbers: &[CheckpointSequenceNumber],
    ) -> StateReadResult<Vec<Option<VerifiedCheckpoint>>>;

    fn get_total_transaction_blocks(&self) -> StateReadResult<u64>;

    fn get_checkpoint_by_sequence_number(
        &self,
        sequence_number: CheckpointSequenceNumber,
    ) -> StateReadResult<Option<VerifiedCheckpoint>>;

    fn get_latest_checkpoint_sequence_number(&self) -> StateReadResult<CheckpointSequenceNumber>;

    fn loaded_child_object_versions(
        &self,
        transaction_digest: &TransactionDigest,
    ) -> StateReadResult<Option<Vec<(ObjectID, SequenceNumber)>>>;

    fn get_chain_identifier(&self) -> StateReadResult<ChainIdentifier>;
}

#[async_trait]
impl StateRead for AuthorityState {
    async fn multi_get(
        &self,
        transactions: &[TransactionDigest],
        effects: &[TransactionDigest],
        events: &[TransactionEventsDigest],
    ) -> StateReadResult<KVStoreTransactionData> {
        Ok(
            <AuthorityState as TransactionKeyValueStoreTrait>::multi_get(
                self,
                transactions,
                effects,
                events,
            )
            .await?,
        )
    }

    async fn multi_get_checkpoints(
        &self,
        checkpoint_summaries: &[CheckpointSequenceNumber],
        checkpoint_contents: &[CheckpointSequenceNumber],
        checkpoint_summaries_by_digest: &[CheckpointDigest],
        checkpoint_contents_by_digest: &[CheckpointContentsDigest],
    ) -> StateReadResult<KVStoreCheckpointData> {
        Ok(
            <AuthorityState as TransactionKeyValueStoreTrait>::multi_get_checkpoints(
                self,
                checkpoint_summaries,
                checkpoint_contents,
                checkpoint_summaries_by_digest,
                checkpoint_contents_by_digest,
            )
            .await?,
        )
    }

    fn get_object_read(&self, object_id: &ObjectID) -> StateReadResult<ObjectRead> {
        Ok(self.get_object_read(object_id)?)
    }

    async fn get_object(&self, object_id: &ObjectID) -> StateReadResult<Option<Object>> {
        Ok(self.get_object(object_id).await?)
    }

    fn get_past_object_read(
        &self,
        object_id: &ObjectID,
        version: SequenceNumber,
    ) -> StateReadResult<PastObjectRead> {
        Ok(self.get_past_object_read(object_id, version)?)
    }

    fn load_epoch_store_one_call_per_task(&self) -> Guard<Arc<AuthorityPerEpochStore>> {
        self.load_epoch_store_one_call_per_task()
    }

    fn get_dynamic_fields(
        &self,
        owner: ObjectID,
        cursor: Option<ObjectID>,
        limit: usize,
    ) -> StateReadResult<Vec<(ObjectID, DynamicFieldInfo)>> {
        Ok(self.get_dynamic_fields(owner, cursor, limit)?)
    }

    fn get_cache_reader(&self) -> Arc<dyn ExecutionCacheRead> {
        self.get_cache_reader()
    }

    fn get_object_store(&self) -> Arc<dyn ObjectStore> {
        self.get_object_store()
    }

    fn get_backing_package_store(&self) -> Arc<dyn BackingPackageStore> {
        self.get_backing_package_store()
    }

    fn get_owner_objects(
        &self,
        owner: MgoAddress,
        cursor: Option<ObjectID>,
        filter: Option<MgoObjectDataFilter>,
    ) -> StateReadResult<Vec<ObjectInfo>> {
        Ok(self
            .get_owner_objects_iterator(owner, cursor, filter)?
            .collect())
    }

    async fn query_events(
        &self,
        kv_store: &Arc<TransactionKeyValueStore>,
        query: EventFilter,
        // If `Some`, the query will start from the next item after the specified cursor
        cursor: Option<EventID>,
        limit: usize,
        descending: bool,
    ) -> StateReadResult<Vec<MgoEvent>> {
        Ok(self
            .query_events(kv_store, query, cursor, limit, descending)
            .await?)
    }

    #[allow(clippy::type_complexity)]
    async fn dry_exec_transaction(
        &self,
        transaction: TransactionData,
        transaction_digest: TransactionDigest,
    ) -> StateReadResult<(
        DryRunTransactionBlockResponse,
        BTreeMap<ObjectID, (ObjectRef, Object, WriteKind)>,
        TransactionEffects,
        Option<ObjectID>,
    )> {
        Ok(self
            .dry_exec_transaction(transaction, transaction_digest)
            .await?)
    }

    async fn dev_inspect_transaction_block(
        &self,
        sender: MgoAddress,
        transaction_kind: TransactionKind,
        gas_price: Option<u64>,
        gas_budget: Option<u64>,
        gas_sponsor: Option<MgoAddress>,
        gas_objects: Option<Vec<ObjectRef>>,
        show_raw_txn_data_and_effects: Option<bool>,
        skip_checks: Option<bool>,
    ) -> StateReadResult<DevInspectResults> {
        Ok(self
            .dev_inspect_transaction_block(
                sender,
                transaction_kind,
                gas_price,
                gas_budget,
                gas_sponsor,
                gas_objects,
                show_raw_txn_data_and_effects,
                skip_checks,
            )
            .await?)
    }

    fn get_subscription_handler(&self) -> Arc<SubscriptionHandler> {
        self.subscription_handler.clone()
    }

    fn get_owner_objects_with_limit(
        &self,
        owner: MgoAddress,
        cursor: Option<ObjectID>,
        limit: usize,
        filter: Option<MgoObjectDataFilter>,
    ) -> StateReadResult<Vec<ObjectInfo>> {
        Ok(self.get_owner_objects(owner, cursor, limit, filter)?)
    }

    async fn get_transactions(
        &self,
        kv_store: &Arc<TransactionKeyValueStore>,
        filter: Option<TransactionFilter>,
        cursor: Option<TransactionDigest>,
        limit: Option<usize>,
        reverse: bool,
    ) -> StateReadResult<Vec<TransactionDigest>> {
        Ok(self
            .get_transactions(kv_store, filter, cursor, limit, reverse)
            .await?)
    }

    fn get_dynamic_field_object_id(
        // indexer
        &self,
        owner: ObjectID,
        name_type: TypeTag,
        name_bcs_bytes: &[u8],
    ) -> StateReadResult<Option<ObjectID>> {
        Ok(self.get_dynamic_field_object_id(owner, name_type, name_bcs_bytes)?)
    }

    async fn get_staked_mgo(&self, owner: MgoAddress) -> StateReadResult<Vec<StakedMgo>> {
        Ok(self
            .get_move_objects(owner, MoveObjectType::staked_mgo())
            .await?)
    }
    fn get_system_state(&self) -> StateReadResult<MgoSystemState> {
        Ok(self.database.get_mgo_system_state_object_unsafe()?)
    }
    fn get_or_latest_committee(&self, epoch: Option<BigInt<u64>>) -> StateReadResult<Committee> {
        Ok(self
            .committee_store()
            .get_or_latest_committee(epoch.map(|e| *e))?)
    }

    fn find_publish_txn_digest(&self, package_id: ObjectID) -> StateReadResult<TransactionDigest> {
        Ok(self.find_publish_txn_digest(package_id)?)
    }
    fn get_owned_coins(
        &self,
        owner: MgoAddress,
        cursor: (String, ObjectID),
        limit: usize,
        one_coin_type_only: bool,
    ) -> StateReadResult<Vec<MgoCoin>> {
        Ok(self
            .get_owned_coins_iterator_with_cursor(owner, cursor, limit, one_coin_type_only)?
            .map(|(coin_type, coin_object_id, coin)| MgoCoin {
                coin_type,
                coin_object_id,
                version: coin.version,
                digest: coin.digest,
                balance: coin.balance,
                previous_transaction: coin.previous_transaction,
            })
            .collect::<Vec<_>>())
    }

    async fn get_executed_transaction_and_effects(
        &self,
        digest: TransactionDigest,
        kv_store: Arc<TransactionKeyValueStore>,
    ) -> StateReadResult<(Transaction, TransactionEffects)> {
        Ok(self
            .get_executed_transaction_and_effects(digest, kv_store)
            .await?)
    }

    async fn get_balance(
        &self,
        owner: MgoAddress,
        coin_type: TypeTag,
    ) -> StateReadResult<TotalBalance> {
        Ok(self
            .indexes
            .as_ref()
            .ok_or(MgoError::IndexStoreNotAvailable)?
            .get_balance(owner, coin_type)
            .await?)
    }

    async fn get_all_balance(
        &self,
        owner: MgoAddress,
    ) -> StateReadResult<Arc<HashMap<TypeTag, TotalBalance>>> {
        Ok(self
            .indexes
            .as_ref()
            .ok_or(MgoError::IndexStoreNotAvailable)?
            .get_all_balance(owner)
            .await?)
    }

    fn get_verified_checkpoint_by_sequence_number(
        &self,
        sequence_number: CheckpointSequenceNumber,
    ) -> StateReadResult<VerifiedCheckpoint> {
        Ok(self.get_verified_checkpoint_by_sequence_number(sequence_number)?)
    }

    fn get_checkpoint_contents(
        &self,
        digest: CheckpointContentsDigest,
    ) -> StateReadResult<CheckpointContents> {
        Ok(self.get_checkpoint_contents(digest)?)
    }

    fn get_verified_checkpoint_summary_by_digest(
        &self,
        digest: CheckpointDigest,
    ) -> StateReadResult<VerifiedCheckpoint> {
        Ok(self.get_verified_checkpoint_summary_by_digest(digest)?)
    }

    fn deprecated_multi_get_transaction_checkpoint(
        &self,
        digests: &[TransactionDigest],
    ) -> StateReadResult<Vec<Option<(EpochId, CheckpointSequenceNumber)>>> {
        Ok(self
            .database
            .deprecated_multi_get_transaction_checkpoint(digests)?)
    }

    fn deprecated_get_transaction_checkpoint(
        &self,
        digest: &TransactionDigest,
    ) -> StateReadResult<Option<(EpochId, CheckpointSequenceNumber)>> {
        Ok(self
            .database
            .deprecated_get_transaction_checkpoint(digest)?)
    }

    fn multi_get_checkpoint_by_sequence_number(
        &self,
        sequence_numbers: &[CheckpointSequenceNumber],
    ) -> StateReadResult<Vec<Option<VerifiedCheckpoint>>> {
        Ok(self.multi_get_checkpoint_by_sequence_number(sequence_numbers)?)
    }

    fn get_total_transaction_blocks(&self) -> StateReadResult<u64> {
        Ok(self.get_total_transaction_blocks()?)
    }

    fn get_checkpoint_by_sequence_number(
        &self,
        sequence_number: CheckpointSequenceNumber,
    ) -> StateReadResult<Option<VerifiedCheckpoint>> {
        Ok(self.get_checkpoint_by_sequence_number(sequence_number)?)
    }

    fn get_latest_checkpoint_sequence_number(&self) -> StateReadResult<CheckpointSequenceNumber> {
        Ok(self.get_latest_checkpoint_sequence_number()?)
    }

    fn loaded_child_object_versions(
        &self,
        transaction_digest: &TransactionDigest,
    ) -> StateReadResult<Option<Vec<(ObjectID, SequenceNumber)>>> {
        Ok(self.loaded_child_object_versions(transaction_digest)?)
    }

    fn get_chain_identifier(&self) -> StateReadResult<ChainIdentifier> {
        Ok(self
            .get_chain_identifier()
            .ok_or(anyhow!("Chain identifier not found"))?)
    }
}

/// This implementation allows `S` to be a dynamically sized type (DST) that implements ObjectProvider
/// Valid as `S` is referenced only, and memory management is handled by `Arc`
#[async_trait]
impl<S: ?Sized + StateRead> ObjectProvider for Arc<S> {
    type Error = StateReadError;

    async fn get_object(
        &self,
        id: &ObjectID,
        version: &SequenceNumber,
    ) -> Result<Object, Self::Error> {
        Ok(self.get_past_object_read(id, *version)?.into_object()?)
    }

    async fn find_object_lt_or_eq_version(
        &self,
        id: &ObjectID,
        version: &SequenceNumber,
    ) -> Result<Option<Object>, Self::Error> {
        let cache = self.get_cache_reader();
        let id = *id;
        let version = *version;
        spawn_monitored_task!(async move { cache.find_object_lt_or_eq_version(id, version) })
            .await
            .map_err(StateReadError::from)
    }
}

#[async_trait]
impl<S: ?Sized + StateRead> ObjectProvider for (Arc<S>, Arc<TransactionKeyValueStore>) {
    type Error = StateReadError;

    async fn get_object(
        &self,
        id: &ObjectID,
        version: &SequenceNumber,
    ) -> Result<Object, Self::Error> {
        let object_read = self.0.get_past_object_read(id, *version)?;
        match object_read {
            PastObjectRead::ObjectNotExists(_) | PastObjectRead::VersionNotFound(..) => {
                match self.1.get_object(*id, *version).await? {
                    Some(object) => Ok(object),
                    None => Ok(PastObjectRead::VersionNotFound(*id, *version).into_object()?),
                }
            }
            _ => Ok(object_read.into_object()?),
        }
    }

    async fn find_object_lt_or_eq_version(
        &self,
        id: &ObjectID,
        version: &SequenceNumber,
    ) -> Result<Option<Object>, Self::Error> {
        let cache = self.0.get_cache_reader();
        let id = *id;
        let version = *version;
        spawn_monitored_task!(async move { cache.find_object_lt_or_eq_version(id, version) })
            .await
            .map_err(StateReadError::from)
    }
}

#[derive(Debug, Error)]
pub enum StateReadInternalError {
    #[error(transparent)]
    MgoError(#[from] MgoError),
    #[error(transparent)]
    JoinError(#[from] JoinError),
    #[error(transparent)]
    Anyhow(#[from] anyhow::Error),
}

#[derive(Debug, Error)]
pub enum StateReadClientError {
    #[error(transparent)]
    MgoError(#[from] MgoError),
    #[error(transparent)]
    UserInputError(#[from] UserInputError),
}

/// `StateReadError` is the error type for callers to work with.
/// It captures all possible errors that can occur while reading state, classifying them into two categories.
/// Unless `StateReadError` is the final error state before returning to caller, the app may still want error context.
/// This context is preserved in `Internal` and `Client` variants.
#[derive(Debug, Error)]
pub enum StateReadError {
    // mgo_json_rpc::Error will do the final conversion to generic error message
    #[error(transparent)]
    Internal(#[from] StateReadInternalError),

    // Client errors
    #[error(transparent)]
    Client(#[from] StateReadClientError),
}

impl From<MgoError> for StateReadError {
    fn from(e: MgoError) -> Self {
        match e {
            MgoError::IndexStoreNotAvailable
            | MgoError::TransactionNotFound { .. }
            | MgoError::UnsupportedFeatureError { .. }
            | MgoError::UserInputError { .. }
            | MgoError::WrongMessageVersion { .. } => StateReadError::Client(e.into()),
            _ => StateReadError::Internal(e.into()),
        }
    }
}

impl From<UserInputError> for StateReadError {
    fn from(e: UserInputError) -> Self {
        StateReadError::Client(e.into())
    }
}

impl From<JoinError> for StateReadError {
    fn from(e: JoinError) -> Self {
        StateReadError::Internal(e.into())
    }
}

impl From<anyhow::Error> for StateReadError {
    fn from(e: anyhow::Error) -> Self {
        StateReadError::Internal(e.into())
    }
}
