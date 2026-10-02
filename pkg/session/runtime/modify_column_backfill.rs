// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Bounded MVCC row conversion for the durable MODIFY COLUMN worker.
use super::{ConcreteSession, kv};
use astersql_meta_model::{ColumnInfo, TableInfo};
use std::collections::HashMap;
#[allow(clippy::too_many_arguments)]
pub(super) fn batch(
    session: &mut ConcreteSession,
    table: &TableInfo,
    old: &ColumnInfo,
    new: &ColumnInfo,
    physical: i64,
    start: &[u8],
    end: &[u8],
    limit: usize,
    mode: u64,
    location: Option<&astersql_meta_model::TimeZoneLocation>,
) -> Result<(Vec<u8>, i64), String> {
    let (codec_location, fixed) = match location {
        Some(location) if !location.name.is_empty() => (
            location
                .name
                .parse::<chrono_tz::Tz>()
                .map_err(|e| e.to_string())?,
            None,
        ),
        Some(location) if location.offset != 0 => (
            chrono_tz::UTC,
            Some(
                chrono::FixedOffset::east_opt(location.offset)
                    .ok_or("invalid MODIFY COLUMN timezone offset")?,
            ),
        ),
        _ => (chrono_tz::UTC, None),
    };
    let mut options = vec![
        astersql_expression_exprstatic::WithSQLMode(astersql_parser_mysql::r#const::SQLMode(
            mode as i64,
        )),
        astersql_expression_exprstatic::WithLocation(codec_location),
    ];
    if let Some(zone) = fixed {
        options.push(astersql_expression_exprstatic::WithCurrentTime(
            std::sync::Arc::new(move || {
                // The codec's Location ABI accepts named zones. Fixed-offset
                // evaluation uses local calendar fields in UTC, then translates
                // TIMESTAMP values at the storage boundary below.
                Ok(chrono::Utc::now()
                    .with_timezone(&zone)
                    .naive_local()
                    .and_utc()
                    .with_timezone(&chrono_tz::UTC))
            }),
        ));
    }
    let eval = std::sync::Arc::new(astersql_expression_exprstatic::NewEvalContext(options));
    let build = astersql_expression_exprstatic::NewExprContext(vec![
        astersql_expression_exprstatic::WithEvalCtx(eval.clone()),
    ]);
    let mut state = session.state.borrow_mut();
    let row_encoder_enabled = state.row_encoder_enabled;
    let txn = state
        .transaction
        .as_mut()
        .ok_or("active modify-column transaction required")?;
    let prefix = astersql_tablecodec::GenTableRecordPrefix(physical).0;
    if !start.starts_with(&prefix) || end > kv::Key(prefix.clone()).PrefixNext().0.as_slice() {
        return Err("modify-column backfill range outside physical table".into());
    }
    let mut iterator = txn
        .GetSnapshot()
        .Iter(kv::Key(start.to_vec()), Some(kv::Key(end.to_vec())))
        .map_err(|e| e.to_string())?;
    let mut keys = Vec::new();
    while iterator.Valid() && keys.len() < limit {
        if !iterator.Key().0.starts_with(&prefix) {
            break;
        }
        keys.push(iterator.Key());
        iterator.Next().map_err(|e| e.to_string())?;
    }
    let next = if iterator.Valid() && iterator.Key().0.starts_with(&prefix) {
        iterator.Key().0
    } else {
        end.to_vec()
    };
    iterator.Close();
    let fields: HashMap<_, _> = table
        .Columns
        .iter()
        .map(|c| (c.ID, Box::new(c.FieldType.clone())))
        .collect();
    let internal = kv::WithInternalSourceType(kv::Context::default(), kv::InternalTxnDDL);
    for key in &keys {
        txn.LockKeys(
            &internal,
            &mut kv::LockCtx {
                WaitTimeoutMs: -1,
                ..Default::default()
            },
            std::slice::from_ref(key),
        )
        .map_err(|e| e.to_string())?;
        let raw = match txn.Get(&internal, key.clone(), &[]) {
            Ok(value) => value.Value,
            Err(e) if kv::IsErrNotFound(&e) => continue,
            Err(e) => return Err(e.to_string()),
        };
        let mut row = astersql_tablecodec::DecodeRowToDatumMap(
            Some(raw),
            fields.clone(),
            Some(codec_location),
        )
        .map_err(|e| e.to_string())?;
        let mut value = match row.get(&old.ID) {
            Some(value) => value.clone(),
            None => astersql_table::column::GetColOriginDefaultValue(
                &build,
                &astersql_table::column::Column::New(Box::new(old.clone())),
            )
            .map_err(|e| e.to_string())?,
        };
        if value.IsNull() && astersql_parser_mysql::r#type::HasNotNullFlag(new.GetFlag()) {
            return Err("[ddl:1138]Invalid use of NULL value".into());
        }
        if let Some(zone) = fixed {
            if old.GetType() == astersql_parser_mysql::r#type::TypeTimestamp {
                translate_fixed_timestamp(&mut value, zone, true)?;
            }
        }
        let casted = if fixed.is_some()
            && new.GetType() == astersql_parser_mysql::r#type::TypeTimestamp
        {
            // Parse/round the local calendar value before validating its UTC
            // TIMESTAMP range, which may cross a date or the 2038 boundary.
            let mut local_column = new.clone();
            local_column.SetType(astersql_parser_mysql::r#type::TypeDatetime);
            let mut local =
                astersql_table::column::CastValue(eval.as_ref(), value, &local_column, true, false)
                    .map_err(|e| e.to_string())?;
            translate_fixed_timestamp(&mut local, fixed.unwrap(), false)?;
            astersql_table::column::CastValue(eval.as_ref(), local, new, true, false)
                .map_err(|e| e.to_string())?
        } else {
            astersql_table::column::CastValue(eval.as_ref(), value, new, true, false)
                .map_err(|e| e.to_string())?
        };
        row.insert(new.ID, casted);
        let (ids, values): (Vec<_>, Vec<_>) = row.into_iter().unzip();
        let encoded = astersql_tablecodec::EncodeRow(
            Some(codec_location),
            values,
            ids,
            Vec::new(),
            None,
            None,
            astersql_tablecodec::rowcodec::Encoder::new(row_encoder_enabled),
        )
        .map_err(|e| e.to_string())?;
        txn.Set(key.clone(), encoded).map_err(|e| e.to_string())?;
    }
    Ok((next, keys.len() as i64))
}

/// Preserve distinct temporary-index history under the same transaction as DML.
pub(super) fn temporary_value(
    txn: &mut dyn kv::Transaction,
    key: &kv::Key,
    value: Vec<u8>,
) -> Result<Vec<u8>, kv::errors::SharedError> {
    if !astersql_tablecodec::IsIndexKey(&key.0) || !astersql_tablecodec::IsTempIndexKey(&key.0) {
        return Ok(value);
    }
    use astersql_tablecodec::TempIndexValueExt;
    let decoded = astersql_tablecodec::DecodeTempIndexValue(value.clone())?;
    if !decoded.Current().is_some_and(|elem| elem.Distinct) {
        return Ok(value);
    }
    txn.LockKeys(
        &kv::Context::default(),
        &mut kv::LockCtx::default(),
        std::slice::from_ref(key),
    )?;
    match txn.Get(&kv::Context::default(), key.clone(), &[]) {
        Ok(old) => {
            let mut history = old.Value;
            history.extend(value);
            Ok(history)
        }
        Err(error) if kv::IsErrNotFound(&error) => Ok(value),
        Err(error) => Err(error),
    }
}

/// Apply bounded temporary-index history using current MVCC values and locks.
pub(super) fn merge(
    session: &mut ConcreteSession,
    request: astersql_ddl::backfilling::IndexBackfillBatch,
) -> Result<astersql_ddl::backfilling::BackfillTaskContext, String> {
    use astersql_tablecodec::TempIndexValueExt;
    let mut state = session.state.borrow_mut();
    let txn = state
        .transaction
        .as_mut()
        .ok_or("active merge transaction required")?;
    let mut iterator = txn
        .GetSnapshot()
        .Iter(
            kv::Key(request.task.start_key.clone()),
            Some(kv::Key(request.task.end_key.clone())),
        )
        .map_err(|e| e.to_string())?;
    let mut keys = Vec::new();
    while iterator.Valid() && keys.len() < request.batch_size {
        keys.push(iterator.Key());
        iterator.Next().map_err(|e| e.to_string())?;
    }
    let next = if iterator.Valid() {
        iterator.Key().0
    } else {
        request.task.end_key.clone()
    };
    iterator.Close();
    let table = astersql_meta::TransactionMutator::new(txn.as_mut())
        .get_table(request.schema_id, request.table_id)?
        .ok_or("merge table missing")?;
    for key in &keys {
        if !astersql_tablecodec::IsIndexKey(&key.0) || !astersql_tablecodec::IsTempIndexKey(&key.0)
        {
            return Err("temporary index merge range contains non-temporary key".into());
        }
        let id = astersql_tablecodec::DecodeIndexID(astersql_tablecodec::kv::Key(key.0.clone()))
            .map_err(|e| e.to_string())?
            & astersql_tablecodec::IndexIDMask;
        if !request.index_ids.contains(&id) {
            continue;
        }
        let index = table
            .Indices
            .iter()
            .find(|index| index.ID == id)
            .ok_or("merge index missing")?;
        let mut original = key.0.clone();
        astersql_tablecodec::TempIndexKey2IndexKey(&mut original);
        let original = kv::Key(original);
        txn.LockKeys(
            &kv::Context::default(),
            &mut kv::LockCtx::default(),
            &[key.clone(), original.clone()],
        )
        .map_err(|e| e.to_string())?;
        let raw = match txn.Get(&kv::Context::default(), key.clone(), &[]) {
            Ok(value) => value.Value,
            Err(error) if kv::IsErrNotFound(&error) => continue,
            Err(error) => return Err(error.to_string()),
        };
        for elem in astersql_tablecodec::DecodeTempIndexValue(raw)
            .map_err(|e| e.to_string())?
            .FilterOverwritten()
            .into_iter()
            .flatten()
        {
            if elem.KeyVer == astersql_tablecodec::TempIndexKeyTypeMerge {
                continue;
            }
            if elem.Delete {
                if elem.Distinct {
                    let existing = match txn.Get(&kv::Context::default(), original.clone(), &[]) {
                        Ok(value) => value.Value,
                        Err(error) if kv::IsErrNotFound(&error) => continue,
                        Err(error) => return Err(error.to_string()),
                    };
                    let handle = astersql_tablecodec::DecodeIndexHandle(
                        original.0.clone(),
                        existing,
                        index.Columns.len(),
                    )
                    .map_err(|e| e.to_string())?
                    .ok_or("merge unique index handle missing")?;
                    if !handle.Equal(elem.Handle.as_ref()) {
                        let row_key = astersql_tablecodec::EncodeRowKeyWithHandle(
                            request.task.physical_table_id,
                            handle,
                        );
                        match txn.Get(&kv::Context::default(), kv::Key(row_key.0), &[]) {
                            Ok(_) => continue,
                            Err(error) if kv::IsErrNotFound(&error) => {}
                            Err(error) => return Err(error.to_string()),
                        }
                    }
                }
                txn.Delete(original.clone()).map_err(|e| e.to_string())?;
            } else {
                if elem.Distinct {
                    match txn.Get(&kv::Context::default(), original.clone(), &[]) {
                        Ok(existing) => {
                            if existing.Value != elem.Value {
                                return Err(format!(
                                    "[kv:1062]Duplicate entry for key '{}'",
                                    index.Name.O
                                ));
                            }
                            // The existing value is already correct; Go skips replaying it.
                            continue;
                        }
                        Err(error) if kv::IsErrNotFound(&error) => {}
                        Err(error) => return Err(error.to_string()),
                    }
                }
                txn.Set(original.clone(), elem.Value)
                    .map_err(|e| e.to_string())?;
            }
        }
        txn.Delete(key.clone()).map_err(|e| e.to_string())?;
    }
    Ok(astersql_ddl::backfilling::BackfillTaskContext {
        next_key: next.clone(),
        done: next >= request.task.end_key,
        scan_count: keys.len() as i64,
        added_count: keys.len() as i64,
        finish_ts: txn.StartTS(),
        ..Default::default()
    })
}

/// Use the disk-backed Lightning engine and the store's Write/MultiIngest transport.
pub(super) fn ingest(
    domain: std::sync::Arc<astersql_domain::Domain>,
    job_id: i64,
    pairs: Vec<astersql_lightning_verification::KvPair>,
) -> Result<(), String> {
    ingest_with_options(
        domain,
        job_id,
        pairs,
        astersql_kv::SSTImportOptions::default(),
    )
}
pub(super) fn ingest_with_options(
    domain: std::sync::Arc<astersql_domain::Domain>,
    job_id: i64,
    pairs: Vec<astersql_lightning_verification::KvPair>,
    options: astersql_kv::SSTImportOptions,
) -> Result<(), String> {
    if pairs.is_empty() {
        return Ok(());
    }
    use astersql_lightning_backend as backend;
    let physical = super::import_sst::Backend::new_with_options(domain, job_id, options)
        .map_err(|e| e.to_string())?;
    let engines = backend::MakeEngineManager(physical.clone());
    let context = astersql_lightning_backend_encode::Context::default();
    let engine = engines
        .OpenEngine(
            &context,
            &backend::EngineConfig::default(),
            &format!("ddl-{job_id}"),
            0,
        )
        .map_err(|e| e.to_string())?;
    let mut writer = engine
        .LocalWriter(&context, &backend::LocalWriterConfig::default())
        .map_err(|e| e.to_string())?;
    writer
        .AppendRows(
            &context,
            &[],
            &astersql_lightning_backend_kv::Pairs {
                Pairs: pairs,
                ..Default::default()
            },
        )
        .map_err(|e| e.to_string())?;
    writer.Close(&context).map_err(|e| e.to_string())?;
    let engine = engine.Close(&context).map_err(|e| e.to_string())?;
    engine
        .Import(&context, 96 * 1024 * 1024, 960_000)
        .map_err(|e| e.to_string())?;
    engine.Cleanup(&context).map_err(|e| e.to_string())?;
    Ok(())
}

fn translate_fixed_timestamp(
    value: &mut astersql_types::datum::Datum,
    zone: chrono::FixedOffset,
    from_utc: bool,
) -> Result<(), String> {
    use chrono::{Datelike, TimeZone, Timelike};
    if value.Kind() != astersql_types::datum::KindMysqlTime {
        return Ok(());
    }
    let mut time = value.GetMysqlTime();
    if time.IsZero() {
        return Ok(());
    }
    let calendar = time.GoTime(chrono_tz::UTC).map_err(|e| e.to_string())?;
    let local = if from_utc {
        calendar.with_timezone(&zone).naive_local()
    } else {
        zone.from_local_datetime(&calendar.naive_utc())
            .single()
            .ok_or("invalid fixed-offset calendar time")?
            .with_timezone(&chrono_tz::UTC)
            .naive_local()
    };
    time.SetCoreTime(astersql_types::time::FromDate(
        local.year(),
        local.month() as i32,
        local.day() as i32,
        local.hour() as i32,
        local.minute() as i32,
        local.second() as i32,
        local.nanosecond() as i32 / 1000,
    ));
    value.SetMysqlTime(time);
    Ok(())
}
