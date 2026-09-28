// Copyright 2026 AsterSQL.

//! Domain 规范 KV 与 `meta/autoid` 事务接口之间的适配层。

use std::sync::Arc;

use astersql_meta_autoid::{AutoIdError, AutoIdKey, AutoIdKeyKind, IdStore, IdTransaction, Result};

use crate::canonical_domain::StorageHandle;

const AUTO_ID_KEY_PREFIX: &str = "mAutoID:canonical:v1";
const SEPARATE_AUTO_INCREMENT_VERSION: u16 = 5;

/// 使用 Domain 唯一 StorageHandle 持久化 AutoID 高水位。
pub(crate) struct KvAutoIdStore {
    storage: Arc<StorageHandle>,
}

impl KvAutoIdStore {
    pub(crate) fn new(storage: Arc<StorageHandle>) -> Self {
        Self { storage }
    }
}

struct KvAutoIdTransaction<'a> {
    transaction: &'a mut dyn astersql_kv::Transaction,
    context: astersql_kv::Context,
}

impl KvAutoIdTransaction<'_> {
    fn key(key: AutoIdKey) -> astersql_kv::Key {
        // 对齐 Go meta accessor：V5 之前 AUTO_INCREMENT 与 RowID 共用 TID，
        // V5 起才使用独立 IID；其他类型分别使用 TARID/SID/SequenceCycle。
        let kind = match key.kind {
            AutoIdKeyKind::RowId => "TID",
            AutoIdKeyKind::IncrementId(version) if version < SEPARATE_AUTO_INCREMENT_VERSION => {
                "TID"
            }
            AutoIdKeyKind::IncrementId(_) => "IID",
            AutoIdKeyKind::RandomId => "TARID",
            AutoIdKeyKind::SequenceValue => "SID",
            AutoIdKeyKind::SequenceCycle => "SequenceCycle",
        };
        astersql_kv::Key(
            format!(
                "{AUTO_ID_KEY_PREFIX}:DB:{}:{kind}:{}",
                key.database_id, key.table_id
            )
            .into_bytes(),
        )
    }

    fn storage_error(error: impl std::fmt::Display) -> AutoIdError {
        AutoIdError::Storage(error.to_string())
    }
}

impl IdTransaction for KvAutoIdTransaction<'_> {
    fn get(&self, key: AutoIdKey) -> Result<i64> {
        match self.transaction.Get(&self.context, Self::key(key), &[]) {
            Ok(entry) => {
                let value = std::str::from_utf8(&entry.Value)
                    .map_err(Self::storage_error)?
                    .parse::<i64>()
                    .map_err(Self::storage_error)?;
                Ok(value)
            }
            Err(error) if astersql_kv::IsErrNotFound(&error) => Ok(0),
            Err(error) => Err(Self::storage_error(error)),
        }
    }

    fn put(&mut self, key: AutoIdKey, value: i64) -> Result<()> {
        self.transaction
            .Set(Self::key(key), value.to_string().into_bytes())
            .map_err(Self::storage_error)
    }

    fn inc(&mut self, key: AutoIdKey, step: i64) -> Result<i64> {
        let value = self.get(key)?.wrapping_add(step);
        self.put(key, value)?;
        Ok(value)
    }

    fn copy_to(&mut self, from: AutoIdKey, to: AutoIdKey) -> Result<()> {
        let value = self.get(from)?;
        if value != 0 {
            self.put(to, value)?;
        }
        Ok(())
    }
}

impl IdStore for KvAutoIdStore {
    fn run_in_transaction(
        &self,
        operation: &mut dyn FnMut(&mut dyn IdTransaction) -> Result<()>,
    ) -> Result<()> {
        let context = astersql_kv::Context::default();
        let mut operation_error = None;
        let transaction_result = self.storage.with_storage(|storage| {
            astersql_kv::RunInNewTxn(&context, storage, true, |_context, transaction| {
                let mut adapter = KvAutoIdTransaction {
                    transaction,
                    context: astersql_kv::Context::default(),
                };
                match operation(&mut adapter) {
                    Ok(()) => Ok(()),
                    Err(error) => {
                        operation_error = Some(error.clone());
                        Err(astersql_kv::errors::New(error.to_string()))
                    }
                }
            })
        });
        if let Some(error) = operation_error {
            return Err(error);
        }
        transaction_result.map_err(|error| AutoIdError::Storage(error.to_string()))
    }
}
