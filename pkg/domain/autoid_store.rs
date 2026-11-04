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

    /// Initialize a sequence exactly once, independently of user transactions.
    /// The marker and water level commit together; retries never rewind values.
    pub(crate) fn initialize_sequence(
        &self,
        database_id: i64,
        table_id: i64,
        base: i64,
    ) -> Result<()> {
        let context = astersql_kv::Context::default();
        self.storage
            .with_storage(|storage| {
                astersql_kv::RunInNewTxn(&context, storage, true, |_, transaction| {
                    let marker = astersql_kv::Key(
                        format!("{AUTO_ID_KEY_PREFIX}:DB:{database_id}:SID-initialized:{table_id}")
                            .into_bytes(),
                    );
                    match transaction.Get(&context, marker.clone(), &[]) {
                        Ok(_) => return Ok(()),
                        Err(error) if astersql_kv::IsErrNotFound(&error) => {}
                        Err(error) => return Err(error),
                    }
                    let key = KvAutoIdTransaction::key(AutoIdKey {
                        database_id,
                        table_id,
                        kind: AutoIdKeyKind::SequenceValue,
                    });
                    // Preserve any water level written by an existing allocator.
                    match transaction.Get(&context, key.clone(), &[]) {
                        Ok(_) => {}
                        Err(error) if astersql_kv::IsErrNotFound(&error) => {
                            transaction.Set(key, base.to_string().into_bytes())?;
                        }
                        Err(error) => return Err(error),
                    }
                    transaction.Set(marker, b"1".to_vec())
                })
            })
            .map_err(|error| AutoIdError::Storage(error.to_string()))
    }

    /// Set the canonical sequence water level, including a PostgreSQL reset marker.
    /// Water level and called state commit together, independently of the caller's SQL transaction.
    pub(crate) fn set_sequence_value(
        &self,
        database_id: i64,
        table_id: i64,
        value: i64,
        is_called: bool,
    ) -> Result<()> {
        self.sequence_transaction(database_id, table_id, |adapter, key, reset_key| {
            adapter.put(key, value)?;
            // Publish initialization with the reset, never an intermediate water level.
            let initialized = astersql_kv::Key(
                format!("{AUTO_ID_KEY_PREFIX}:DB:{database_id}:SID-initialized:{table_id}")
                    .into_bytes(),
            );
            adapter
                .transaction
                .Set(initialized, b"1".to_vec())
                .map_err(KvAutoIdTransaction::storage_error)?;
            adapter
                .transaction
                .Set(reset_key, if is_called { b"1" } else { b"0" }.to_vec())
                .map_err(KvAutoIdTransaction::storage_error)?;
            Ok(value)
        })
        .map(|_| ())
    }

    /// Reserve a bounded contiguous range in the same KV transaction as setval.
    /// Native allocators retain their original start-congruence semantics; after a
    /// PG reset, the next value is the reset value plus the increment, even off-grid.
    pub(crate) fn next_sequence_range(
        &self,
        database_id: i64,
        table_id: i64,
        sequence: &astersql_meta_model::SequenceInfo,
        count: i64,
    ) -> Result<(i64, i64)> {
        self.sequence_transaction(database_id, table_id, |adapter, key, reset_key| {
            let reset = match adapter
                .transaction
                .Get(&adapter.context, reset_key.clone(), &[])
            {
                Ok(value) => Some(value.Value != b"0"),
                Err(error) if astersql_kv::IsErrNotFound(&error) => None,
                Err(error) => return Err(KvAutoIdTransaction::storage_error(error)),
            };
            let base = adapter.get(key)?;
            if reset == Some(false) {
                // Consume the pending value in the allocation transaction. No arithmetic
                // is needed, including at i64 boundaries or for off-grid increments.
                adapter
                    .transaction
                    .Set(reset_key, b"1".to_vec())
                    .map_err(KvAutoIdTransaction::storage_error)?;
                return Self::reserve_sequence_tail(adapter, key, sequence, count, base);
            }
            let cycle_key = AutoIdKey {
                kind: AutoIdKeyKind::SequenceCycle,
                ..key
            };
            let round = adapter.get(cycle_key)?;
            let offset = if sequence.Cycle && round > 0 {
                if sequence.Increment > 0 {
                    sequence.MinValue
                } else {
                    sequence.MaxValue
                }
            } else {
                sequence.Start
            };
            let next = if reset.is_some() {
                let next = i128::from(base) + i128::from(sequence.Increment);
                if next < i128::from(sequence.MinValue) || next > i128::from(sequence.MaxValue) {
                    None
                } else {
                    Some(next as i64)
                }
            } else {
                match astersql_meta_autoid::calc_sequence_batch_size(
                    base,
                    1,
                    sequence.Increment,
                    offset,
                    sequence.MinValue,
                    sequence.MaxValue,
                ) {
                    Ok(step) => Some(if sequence.Increment > 0 {
                        base.wrapping_add(step)
                    } else {
                        base.wrapping_sub(step)
                    }),
                    Err(AutoIdError::AutoIncrementReadFailed(_)) => None,
                    Err(error) => return Err(error),
                }
            };
            let value = match next {
                Some(value) => value,
                None if sequence.Cycle => {
                    let boundary = if sequence.Increment > 0 {
                        sequence.MinValue
                    } else {
                        sequence.MaxValue
                    };
                    adapter.put(cycle_key, round.wrapping_add(1))?;
                    boundary
                }
                None => {
                    return Err(AutoIdError::AutoIncrementReadFailed(
                        "sequence has reached its limit".into(),
                    ));
                }
            };
            Self::reserve_sequence_tail(adapter, key, sequence, count, value)
        })
    }

    // Stop at the boundary; a subsequent reservation performs CYCLE wrapping.
    // i128 keeps both the full bigint span and negative increments exact.
    fn reserve_sequence_tail(
        adapter: &mut KvAutoIdTransaction,
        key: AutoIdKey,
        sequence: &astersql_meta_model::SequenceInfo,
        count: i64,
        first: i64,
    ) -> Result<(i64, i64)> {
        let increment = i128::from(sequence.Increment);
        let distance = if increment > 0 {
            i128::from(sequence.MaxValue) - i128::from(first)
        } else {
            i128::from(first) - i128::from(sequence.MinValue)
        };
        let steps = (i128::from(count.max(1)) - 1).min(distance / increment.abs());
        let last = (i128::from(first) + steps * increment) as i64;
        adapter.put(key, last)?;
        Ok((first, last))
    }

    fn sequence_transaction<T: Default>(
        &self,
        database_id: i64,
        table_id: i64,
        mut operation: impl FnMut(
            &mut KvAutoIdTransaction<'_>,
            AutoIdKey,
            astersql_kv::Key,
        ) -> Result<T>,
    ) -> Result<T> {
        let context = astersql_kv::Context::default();
        let mut value = T::default();
        let mut operation_error = None;
        let result = self.storage.with_storage(|storage| {
            astersql_kv::RunInNewTxn(&context, storage, true, |_, transaction| {
                let mut adapter = KvAutoIdTransaction {
                    transaction,
                    context: astersql_kv::Context::default(),
                };
                let key = AutoIdKey {
                    database_id,
                    table_id,
                    kind: AutoIdKeyKind::SequenceValue,
                };
                let reset_key = astersql_kv::Key(
                    format!("{AUTO_ID_KEY_PREFIX}:DB:{database_id}:SID-pg-reset:{table_id}")
                        .into_bytes(),
                );
                match operation(&mut adapter, key, reset_key) {
                    Ok(result) => {
                        value = result;
                        operation_error = None;
                        Ok(())
                    }
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
        result.map_err(KvAutoIdTransaction::storage_error)?;
        Ok(value)
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
