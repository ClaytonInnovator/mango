// Copyright (c) MangoNet Labs Ltd.
// SPDX-License-Identifier: Apache-2.0

use crate::indexer_reader::IndexerReader;
use crate::IndexerError;
use async_trait::async_trait;
use jsonrpsee::core::RpcResult;
use jsonrpsee::types::SubscriptionEmptyError;
use jsonrpsee::types::SubscriptionResult;
use jsonrpsee::{RpcModule, SubscriptionSink};
use mgo_json_rpc::name_service::{Domain, NameRecord, NameServiceConfig};
use mgo_json_rpc::MgoRpcModule;
use mgo_json_rpc_api::{cap_page_limit, IndexerApiServer};
use mgo_json_rpc_types::{
    DynamicFieldPage, EventFilter, EventPage, ObjectsPage, Page, MgoObjectResponse,
    MgoObjectResponseQuery, MgoTransactionBlockResponseQuery, TransactionBlocksPage,
    TransactionFilter,
};
use mgo_open_rpc::Module;
use mgo_types::base_types::{ObjectID, MgoAddress};
use mgo_types::digests::TransactionDigest;
use mgo_types::dynamic_field::{DynamicFieldName, Field};
use mgo_types::error::MgoObjectResponseError;
use mgo_types::event::EventID;
use mgo_types::object::ObjectRead;
use mgo_types::TypeTag;

pub(crate) struct IndexerApiV2 {
    inner: IndexerReader,
    name_service_config: NameServiceConfig,
}

impl IndexerApiV2 {
    pub fn new(inner: IndexerReader) -> Self {
        Self {
            inner,
            // TODO allow configuring for other networks
            name_service_config: Default::default(),
        }
    }

    async fn get_owned_objects_internal(
        &self,
        address: MgoAddress,
        query: Option<MgoObjectResponseQuery>,
        cursor: Option<ObjectID>,
        limit: usize,
    ) -> RpcResult<ObjectsPage> {
        let MgoObjectResponseQuery { filter, options } = query.unwrap_or_default();
        let options = options.unwrap_or_default();
        let objects = self
            .inner
            .get_owned_objects_in_blocking_task(address, filter, cursor, limit + 1)
            .await?;
        let mut objects = self
            .inner
            .spawn_blocking(move |this| {
                objects
                    .into_iter()
                    .map(|object| object.try_into_object_read(&this))
                    .collect::<Result<Vec<_>, _>>()
            })
            .await?;
        let has_next_page = objects.len() > limit;
        objects.truncate(limit);

        let next_cursor = objects.last().map(|o_read| o_read.object_id());
        let mut parallel_tasks = vec![];
        for o in objects {
            let inner_clone = self.inner.clone();
            let options = options.clone();
            parallel_tasks.push(tokio::task::spawn(async move {
                match o {
                    ObjectRead::NotExists(id) => Ok(MgoObjectResponse::new_with_error(
                        MgoObjectResponseError::NotExists { object_id: id },
                    )),
                    ObjectRead::Exists(object_ref, o, layout) => {
                        if options.show_display {
                            match inner_clone.get_display_fields(&o, &layout).await {
                                Ok(rendered_fields) => Ok(MgoObjectResponse::new_with_data(
                                    (object_ref, o, layout, options, Some(rendered_fields))
                                        .try_into()?,
                                )),
                                Err(e) => Ok(MgoObjectResponse::new(
                                    Some((object_ref, o, layout, options, None).try_into()?),
                                    Some(MgoObjectResponseError::DisplayError {
                                        error: e.to_string(),
                                    }),
                                )),
                            }
                        } else {
                            Ok(MgoObjectResponse::new_with_data(
                                (object_ref, o, layout, options, None).try_into()?,
                            ))
                        }
                    }
                    ObjectRead::Deleted((object_id, version, digest)) => Ok(
                        MgoObjectResponse::new_with_error(MgoObjectResponseError::Deleted {
                            object_id,
                            version,
                            digest,
                        }),
                    ),
                }
            }));
        }
        let data = futures::future::join_all(parallel_tasks)
            .await
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e: tokio::task::JoinError| anyhow::anyhow!(e))?
            .into_iter()
            .collect::<Result<Vec<_>, anyhow::Error>>()?;

        Ok(Page {
            data,
            next_cursor,
            has_next_page,
        })
    }
}

#[async_trait]
impl IndexerApiServer for IndexerApiV2 {
    async fn get_owned_objects(
        &self,
        address: MgoAddress,
        query: Option<MgoObjectResponseQuery>,
        cursor: Option<ObjectID>,
        limit: Option<usize>,
    ) -> RpcResult<ObjectsPage> {
        let limit = cap_page_limit(limit);
        if limit == 0 {
            return Ok(ObjectsPage::empty());
        }
        self.get_owned_objects_internal(address, query, cursor, limit)
            .await
    }

    async fn query_transaction_blocks(
        &self,
        query: MgoTransactionBlockResponseQuery,
        cursor: Option<TransactionDigest>,
        limit: Option<usize>,
        descending_order: Option<bool>,
    ) -> RpcResult<TransactionBlocksPage> {
        let limit = cap_page_limit(limit);
        if limit == 0 {
            return Ok(TransactionBlocksPage::empty());
        }
        let mut results = self
            .inner
            .query_transaction_blocks_in_blocking_task(
                query.filter,
                query.options.unwrap_or_default(),
                cursor,
                limit + 1,
                descending_order.unwrap_or(false),
            )
            .await
            .map_err(|e: IndexerError| anyhow::anyhow!(e))?;

        let has_next_page = results.len() > limit;
        results.truncate(limit);
        let next_cursor = results.last().map(|o| o.digest);
        Ok(Page {
            data: results,
            next_cursor,
            has_next_page,
        })
    }

    async fn query_events(
        &self,
        query: EventFilter,
        // exclusive cursor if `Some`, otherwise start from the beginning
        cursor: Option<EventID>,
        limit: Option<usize>,
        descending_order: Option<bool>,
    ) -> RpcResult<EventPage> {
        let limit = cap_page_limit(limit);
        if limit == 0 {
            return Ok(EventPage::empty());
        }
        let descending_order = descending_order.unwrap_or(false);
        let mut results = self
            .inner
            .query_events_in_blocking_task(query, cursor, limit + 1, descending_order)
            .await?;

        let has_next_page = results.len() > limit;
        results.truncate(limit);
        let next_cursor = results.last().map(|o| o.id);
        Ok(Page {
            data: results,
            next_cursor,
            has_next_page,
        })
    }

    async fn get_dynamic_fields(
        &self,
        parent_object_id: ObjectID,
        cursor: Option<ObjectID>,
        limit: Option<usize>,
    ) -> RpcResult<DynamicFieldPage> {
        let limit = cap_page_limit(limit);
        if limit == 0 {
            return Ok(DynamicFieldPage::empty());
        }
        let mut results = self
            .inner
            .get_dynamic_fields_in_blocking_task(parent_object_id, cursor, limit + 1)
            .await?;

        let has_next_page = results.len() > limit;
        results.truncate(limit);
        let next_cursor = results.last().map(|o| o.object_id);
        Ok(Page {
            data: results,
            next_cursor,
            has_next_page,
        })
    }

    async fn get_dynamic_field_object(
        &self,
        parent_object_id: ObjectID,
        name: DynamicFieldName,
    ) -> RpcResult<MgoObjectResponse> {
        let name_bcs_value = self
            .inner
            .bcs_name_from_dynamic_field_name_in_blocking_task(&name)
            .await?;

        // Try as Dynamic Field
        let id = mgo_types::dynamic_field::derive_dynamic_field_id(
            parent_object_id,
            &name.type_,
            &name_bcs_value,
        )
        .expect("deriving dynamic field id can't fail");

        let options = mgo_json_rpc_types::MgoObjectDataOptions::full_content();
        match self.inner.get_object_read_in_blocking_task(id).await? {
            mgo_types::object::ObjectRead::NotExists(_)
            | mgo_types::object::ObjectRead::Deleted(_) => {}
            mgo_types::object::ObjectRead::Exists(object_ref, o, layout) => {
                return Ok(MgoObjectResponse::new_with_data(
                    (object_ref, o, layout, options, None).try_into()?,
                ));
            }
        }

        // Try as Dynamic Field Object
        let dynamic_object_field_struct =
            mgo_types::dynamic_field::DynamicFieldInfo::dynamic_object_field_wrapper(name.type_);
        let dynamic_object_field_type = TypeTag::Struct(Box::new(dynamic_object_field_struct));
        let dynamic_object_field_id = mgo_types::dynamic_field::derive_dynamic_field_id(
            parent_object_id,
            &dynamic_object_field_type,
            &name_bcs_value,
        )
        .expect("deriving dynamic field id can't fail");
        match self
            .inner
            .get_object_read_in_blocking_task(dynamic_object_field_id)
            .await?
        {
            mgo_types::object::ObjectRead::NotExists(_)
            | mgo_types::object::ObjectRead::Deleted(_) => {}
            mgo_types::object::ObjectRead::Exists(object_ref, o, layout) => {
                return Ok(MgoObjectResponse::new_with_data(
                    (object_ref, o, layout, options, None).try_into()?,
                ));
            }
        }

        Ok(MgoObjectResponse::new_with_error(
            mgo_types::error::MgoObjectResponseError::DynamicFieldNotFound { parent_object_id },
        ))
    }

    fn subscribe_event(&self, _sink: SubscriptionSink, _filter: EventFilter) -> SubscriptionResult {
        Err(SubscriptionEmptyError)
    }

    fn subscribe_transaction(
        &self,
        _sink: SubscriptionSink,
        _filter: TransactionFilter,
    ) -> SubscriptionResult {
        Err(SubscriptionEmptyError)
    }

    async fn resolve_name_service_address(&self, name: String) -> RpcResult<Option<MgoAddress>> {
        let domain = name.parse::<Domain>().map_err(|e| {
            IndexerError::InvalidArgumentError(format!(
                "Failed to parse NameService Domain with error: {:?}",
                e
            ))
        })?;

        let record_id = self.name_service_config.record_field_id(&domain);

        let field_record_object = match self.inner.get_object_in_blocking_task(record_id).await? {
            Some(o) => o,
            None => return Ok(None),
        };

        let record = field_record_object
            .to_rust::<Field<Domain, NameRecord>>()
            .ok_or_else(|| {
                IndexerError::PersistentStorageDataCorruptionError(format!(
                    "Malformed Object {record_id}"
                ))
            })?
            .value;

        Ok(record.target_address)
    }

    async fn resolve_name_service_names(
        &self,
        address: MgoAddress,
        _cursor: Option<ObjectID>,
        _limit: Option<usize>,
    ) -> RpcResult<Page<String, ObjectID>> {
        let reverse_record_id = self
            .name_service_config
            .reverse_record_field_id(address.as_ref());

        let field_reverse_record_object = match self
            .inner
            .get_object_in_blocking_task(reverse_record_id)
            .await?
        {
            Some(o) => o,
            None => {
                return Ok(Page {
                    data: vec![],
                    next_cursor: None,
                    has_next_page: false,
                })
            }
        };

        let domain = field_reverse_record_object
            .to_rust::<Field<MgoAddress, Domain>>()
            .ok_or_else(|| {
                IndexerError::PersistentStorageDataCorruptionError(format!(
                    "Malformed Object {reverse_record_id}"
                ))
            })?
            .value;

        Ok(Page {
            data: vec![domain.to_string()],
            next_cursor: None,
            has_next_page: false,
        })
    }
}

impl MgoRpcModule for IndexerApiV2 {
    fn rpc(self) -> RpcModule<Self> {
        self.into_rpc()
    }

    fn rpc_doc_module() -> Module {
        mgo_json_rpc_api::IndexerApiOpenRpc::module_doc()
    }
}
