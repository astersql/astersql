// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc. Licensed under Apache-2.0.

//! Compacted-SST compaction estimates and TiKV flow-control configuration.
use crate::client::LogClient;
use crate::stubs::glue::{Session, SqlArg};
use crate::stubs::{Context, Error, Result};
use astersql_br_pkg_restore::BatchBackupFileSet;

pub const tikvSoftPendingCompactionBytesLimit: &str =
    "storage.flow-control.soft-pending-compaction-bytes-limit";
pub const tikvHardPendingCompactionBytesLimit: &str =
    "storage.flow-control.hard-pending-compaction-bytes-limit";
const TIB: u64 = 1 << 40;
const GIB: u64 = 1 << 30;
pub const compactedSSTFlowControlPendingThreshold: u64 = 100 * GIB;

#[derive(Clone, Debug)]
pub struct TiKVConfigValue {
    pub instance: String,
    pub value: String,
}
#[derive(Clone, Debug)]
pub struct CompactedSSTFlowControlConfig {
    pub soft: Vec<TiKVConfigValue>,
    pub hard: Vec<TiKVConfigValue>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompactedSSTFlowControlEstimate {
    pub snapshotRestoreBytes: u64,
    pub compactedSSTBytes: u64,
    pub l6BytesPerStore: u64,
    pub l5BytesPerStore: u64,
    pub pendingBytes: u64,
    pub storeCount: u32,
    pub replicaCount: u32,
}
fn ceilDiv(a: u64, b: u64) -> u64 {
    if b == 0 {
        0
    } else {
        a / b + u64::from(a % b != 0)
    }
}
fn estimateLevelBytesPerStore(total: u64, stores: u32, replicas: u32) -> u64 {
    if stores == 0 || replicas == 0 || total == 0 {
        return 0;
    }
    ceilDiv(total, stores as u64).saturating_mul(replicas.min(stores) as u64)
}
pub fn estimatePendingCompactionBytes(l6: u64, l5: u64) -> u64 {
    if l5 == 0 {
        return 0;
    }
    let l6 = l6 as f64;
    let l5 = l5 as f64;
    let ratio = l6 / l5;
    if ratio > 10.0 || l5 <= l6 / 10.0 {
        return 0;
    }
    let pending = (l5 - l6 / 10.0) * (ratio + 1.0);
    if pending <= 0.0 {
        0
    } else if pending >= u64::MAX as f64 {
        u64::MAX
    } else {
        pending as u64
    }
}
pub fn estimateCompactedSSTFlowControl(
    sets: &BatchBackupFileSet,
    snapshot: u64,
    checkpoint: u64,
    stores: u32,
    replicas: u32,
) -> CompactedSSTFlowControlEstimate {
    let compacted = sets
        .iter()
        .flat_map(|set| &set.SSTFiles)
        .fold(checkpoint, |sum, file| {
            sum.saturating_add(if file.Size_ > 0 {
                file.Size_
            } else {
                file.TotalBytes
            })
        });
    let l6 = estimateLevelBytesPerStore(snapshot, stores, replicas);
    let l5 = estimateLevelBytesPerStore(compacted, stores, replicas);
    CompactedSSTFlowControlEstimate {
        snapshotRestoreBytes: snapshot,
        compactedSSTBytes: compacted,
        l6BytesPerStore: l6,
        l5BytesPerStore: l5,
        pendingBytes: estimatePendingCompactionBytes(l6, l5),
        storeCount: stores,
        replicaCount: replicas,
    }
}

pub fn parseByteSizeConfig(input: &str) -> Result<u64> {
    // Docker go-units splits at the last digit, dot, or single space;
    // ParseFloat then accepts exponent notation and rejects surrounding whitespace.
    let separator = input
        .rfind(|c: char| c.is_ascii_digit() || c == '.' || c == ' ')
        .ok_or_else(|| Error::new(format!("invalid byte size: {input}")))?;
    let end = separator + usize::from(input.as_bytes()[separator] != b' ');
    let value: f64 = input[..end]
        .parse()
        .map_err(|_| Error::new(format!("invalid byte size: {input}")))?;
    let suffix = input[separator + 1..].to_ascii_lowercase();
    // Docker RAMInBytes takes precedence over FromHumanSize, including KB/MB as powers of 1024.
    let power = match suffix.as_str() {
        "" | "b" => 0,
        "k" | "kb" | "kib" => 1,
        "m" | "mb" | "mib" => 2,
        "g" | "gb" | "gib" => 3,
        "t" | "tb" | "tib" => 4,
        "p" | "pb" | "pib" => 5,
        _ => return Err(Error::new(format!("invalid byte size: {input}"))),
    };
    let bytes = value * 1024_f64.powi(power);
    if !bytes.is_finite() || bytes < 0.0 || bytes >= i64::MAX as f64 {
        return Err(Error::new(format!("invalid byte size: {input}")));
    }
    Ok(bytes as u64)
}
pub fn maxTiKVConfigBytes(configs: &[TiKVConfigValue]) -> u64 {
    configs
        .iter()
        .filter_map(|c| parseByteSizeConfig(&c.value).ok())
        .max()
        .unwrap_or(0)
}
pub fn allTiKVConfigsAtLeast(configs: &[TiKVConfigValue], target: u64) -> bool {
    !configs.is_empty()
        && configs
            .iter()
            .all(|c| parseByteSizeConfig(&c.value).is_ok_and(|value| value >= target))
}
pub fn compactedSSTFlowControlTarget(
    config: &CompactedSSTFlowControlConfig,
    pending: u64,
) -> (u64, u64) {
    let soft = TIB
        .max(pending.saturating_add(ceilDiv(pending, 4)))
        .max(maxTiKVConfigBytes(&config.soft));
    let hard = (2 * TIB)
        .max(soft.saturating_mul(2))
        .max(maxTiKVConfigBytes(&config.hard));
    (soft, hard)
}
pub fn formatBytes(bytes: u64) -> String {
    for (unit, suffix) in [
        (TIB, "TiB"),
        (GIB, "GiB"),
        (1 << 20, "MiB"),
        (1 << 10, "KiB"),
    ] {
        if bytes >= unit && bytes % unit == 0 {
            return format!("{}{suffix}", bytes / unit);
        }
    }
    format!("{bytes}B")
}
fn sqlString(value: Option<&SqlArg>) -> Result<String> {
    match value {
        Some(SqlArg::Str(v)) => Ok(v.clone()),
        Some(SqlArg::U64(v)) => Ok(v.to_string()),
        Some(SqlArg::I64(v)) => Ok(v.to_string()),
        Some(SqlArg::Bytes(v)) => {
            String::from_utf8(v.clone()).map_err(|e| Error::new(e.to_string()))
        }
        None => Err(Error::new("missing SHOW CONFIG column")),
    }
}
fn getTiKVConfigValues(
    ctx: &Context,
    session: &dyn Session,
    name: &str,
) -> Result<Vec<TiKVConfigValue>> {
    session
        .ExecRestrictedSQL(
            ctx,
            "show config where name = %? and type = 'tikv'",
            &[SqlArg::Str(name.into())],
        )?
        .into_iter()
        .map(|row| {
            Ok(TiKVConfigValue {
                instance: sqlString(row.cols.get(1))?,
                value: sqlString(row.cols.get(3))?,
            })
        })
        .collect()
}
fn setTiKVConfig(ctx: &Context, session: &dyn Session, name: &str, value: String) -> Result<()> {
    session
        .ExecRestrictedSQL(
            ctx,
            &format!("set config tikv `{name}`=%?"),
            &[SqlArg::Str(value.clone())],
        )
        .map_err(|e| Error::Annotate(e, format!("failed to set config `{name}`={value}")))?;
    Ok(())
}
impl LogClient {
    pub fn adjustTiKVFlowControlForCompactedSSTRestore(
        &self,
        ctx: &Context,
        sets: &BatchBackupFileSet,
        snapshot: u64,
        checkpoint: u64,
    ) -> Result<()> {
        let Some(session) = self.unsafeSession.as_deref() else {
            return Ok(());
        };
        let Some(manager) = &self.sstRestoreManager else {
            return Ok(());
        };
        if manager.storeCount == 0 || manager.replicaCount == 0 {
            return Ok(());
        }
        let soft = getTiKVConfigValues(ctx, session, tikvSoftPendingCompactionBytesLimit)?;
        let hard = getTiKVConfigValues(ctx, session, tikvHardPendingCompactionBytesLimit)?;
        if soft.is_empty() || hard.is_empty() {
            return Ok(());
        }
        let estimate = estimateCompactedSSTFlowControl(
            sets,
            snapshot,
            checkpoint,
            manager.storeCount,
            manager.replicaCount,
        );
        if estimate.pendingBytes <= compactedSSTFlowControlPendingThreshold {
            return Ok(());
        }
        let config = CompactedSSTFlowControlConfig { soft, hard };
        let (soft, hard) = compactedSSTFlowControlTarget(&config, estimate.pendingBytes);
        if allTiKVConfigsAtLeast(&config.soft, soft) && allTiKVConfigsAtLeast(&config.hard, hard) {
            return Ok(());
        }
        setTiKVConfig(
            ctx,
            session,
            tikvHardPendingCompactionBytesLimit,
            formatBytes(hard),
        )?;
        setTiKVConfig(
            ctx,
            session,
            tikvSoftPendingCompactionBytesLimit,
            formatBytes(soft),
        )?;
        Ok(())
    }
}
