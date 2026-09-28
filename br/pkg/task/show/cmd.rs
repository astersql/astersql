// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

//! `br show` reads backup metadata and converts it into a displayable result.
//! Logic mirrors `br/pkg/task/show/cmd.go`.
//!
//! 本模块实现 `br show`：读取 backupmeta，经 MetaReader 展开表或 Raw 区间，
//! 输出可展示的 ShowResult。数据流对齐 Go `cmd.go`；版本窗口非法或 Raw V2
//! 索引存在时拒绝，避免把日志备份/未支持格式误展示为全量结果。

use std::fmt;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use crate::stubs::backuppb::{BackupMeta, CipherInfo, RawRange as PbRawRange};
use crate::stubs::objstore::BackendOptions;
use crate::stubs::oracle;
use crate::stubs::{
    self, Context, Error, HexBytes, MetaFile, MetaReader, MetaTable, NewMetaReader, Result,
    SkipFiles, SkipStats, berrors, task,
};

/// Config mirrors Go `show.Config`.
/// show 侧精简配置：存储 URI、后端选项与解密 Cipher。
#[derive(Clone, Debug, Default)]
pub struct Config {
    /// 备份存储 URI（如 local:// / s3://）。
    pub Storage: String,
    // 备份位置 URI。
    /// 对象存储后端选项，透传给 ReadBackupMeta。
    pub BackendCfg: BackendOptions,
    // S3/GCS 等后端选项。
    /// 解密 backupmeta / schema 所需的 CipherInfo。
    pub Cipher: CipherInfo,
    // 解密密钥材料。
}

impl Config {
    /// lameTaskConfig creates a `task.Config` via the `ShowConfig`.
    /// Because the call `ReadBackupMeta` requires a `task.Cfg` (which is a huge context!),
    /// for reusing it, we need to make a lame config with fields the call needs.
    /// 仅填充 ReadBackupMeta 需要的字段，避免构造完整 task.Config。
    fn lameTaskConfig(&self) -> task::Config {
        task::Config {
            Storage: self.Storage.clone(),
            BackendOptions: self.BackendCfg.clone(),
            CipherInfo: self.Cipher.clone(),
        }
    }
}

/// TimeStamp is a simple wrapper for the timestamp type.
/// Perhaps we can enhance its display.
/// TSO 包装类型：Display 时附带可读时间，便于 show 输出。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct TimeStamp(pub u64);

impl fmt::Display for TimeStamp {
    /// String implements fmt.Stringer via `oracle.GetTimeFromTS` + Go layout
    /// `"Y06M01D02,15:03:04"`.
    /// 格式：`{tso}({可读时间})`，与 Go Stringer 布局对齐。
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}({})", self.0, oracle::format_show_ts(self.0))
    }
}

/// RawKV 备份的一个 key 范围（CF + 起止键）。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RawRange {
    /// json:"column-family"
    /// 列族名，通常为 default。
    pub ColumnFamily: String,
    /// json:"start-key"
    /// 范围起点（十六进制展示）。
    pub StartKey: HexBytes,
    /// json:"end-key"
    /// 范围终点（十六进制展示）。
    pub EndKey: HexBytes,
}

/// 事务备份中单表的展示投影。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Table {
    /// json:"db-name"
    pub DBName: String,
    // 库名。
    /// json:"table-name"
    /// 仅库记录时为空串（Info 缺失）。
    pub TableName: String,
    /// json:"kv-count"
    pub KVCount: u64,
    // 键值条数。
    /// json:"kv-size"
    pub KVSize: u64,
    // 键值字节总量。
    /// json:"tiflash-replica"
    pub TiFlashReplica: u64,
    // TiFlash 副本数。
}

/// `br show` 的聚合结果：集群/版本窗口 + 表列表或 Raw 区间。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ShowResult {
    /// json:"cluster-id"
    pub ClusterID: u64,
    // 源集群 ID。
    /// json:"cluster-version"
    pub ClusterVersion: String,
    // 源集群版本串。
    /// json:"br-version"
    pub BRVersion: String,
    // 生成备份的 BR 版本。
    /// json:"version"
    /// backupmeta 协议版本号。
    pub Version: i32,
    /// json:"start-version"
    pub StartVersion: TimeStamp,
    // 备份起始 TSO。
    /// json:"end-version"
    pub EndVersion: TimeStamp,
    // 备份结束 TSO。
    /// json:"is-raw-kv"
    pub IsRawKV: bool,
    // 是否 RawKV 备份。
    /// json:"raw-ranges"
    pub RawRanges: Vec<RawRange>,
    // Raw 区间列表（IsRawKV 时填充）。
    /// json:"tables"
    pub Tables: Vec<Table>,
    // 表投影列表（事务备份时填充）。
}

/// CmdExecutor holds a `metautil.MetaReader`.
/// 持有 MetaReader，负责把 meta 转成 ShowResult。
#[derive(Debug)]
pub struct CmdExecutor {
    meta: MetaReader,
    // 内部 MetaReader，负责读 schema/files。
}

impl CmdExecutor {
    /// Construct from an already-built MetaReader (tests / injection).
    /// 测试注入用：跳过读盘，直接包装已有 reader。
    pub fn from_reader(meta: MetaReader) -> Self {
        Self { meta }
    }
}

/// CreateExec mirrors Go: read backupmeta, then wrap a MetaReader.
/// 读 backupmeta → NewMetaReader → CmdExecutor；失败时 Annotate。
pub fn CreateExec(ctx: &Context, cfg: Config) -> Result<CmdExecutor> {
    let (_backend, strg, backupMeta) = stubs::ReadBackupMeta(ctx, MetaFile, &cfg.lameTaskConfig())
        .map_err(|err| Error::Annotate(err, "failed to create execution"))?;
    let reader = NewMetaReader(backupMeta, strg, &cfg.Cipher);
    Ok(CmdExecutor { meta: reader })
}

impl CmdExecutor {
    /// Read validates the version window, then fills tables or raw ranges.
    /// 校验 Start≤End；非 Raw 异步读 schema，Raw 则展开 RawRanges（拒 V2 索引）。
    pub fn Read(&self, ctx: &Context) -> Result<ShowResult> {
        let mut res = convertBasic(self.meta.GetBasic());
        // 日志备份常见 Start>End；show 全量视角下直接判非法。
        if res.EndVersion < res.StartVersion {
            return Err(berrors::ErrInvalidMetaFile(format!(
                "the start version({}) is greater than the end version({}), perhaps reading a backup meta from log backup",
                res.StartVersion, res.EndVersion
            )));
        }

        if !res.IsRawKV {
            // 后台线程 ReadSchemasFiles，主线程 collectResult 聚合表投影。
            let (out_tx, out_rx) = mpsc::sync_channel::<MetaTable>(16);
            let (err_tx, err_rx) = mpsc::channel::<Result<()>>();
            let meta = self.meta.clone();
            let ctx_bg = ctx.clone();
            thread::spawn(move || {
                let err = meta.ReadSchemasFiles(&ctx_bg, &out_tx, &[SkipFiles, SkipStats]);
                // Match Go: send error, then close `out` by dropping the sender.
                // 先送错误再 drop out_tx，关闭输出通道以结束收集循环。
                let _ = err_tx.send(err);
                drop(out_tx);
            });
            let ts = collectResult(ctx, out_rx, err_rx, convertTable)?;
            res.Tables = ts;
        } else {
            // NOTE: here we assumed raw KV backup isn't executed in V2.
            // Raw V2（存在 RawRangeIndex）暂不支持 show。
            if self.meta.GetBasic().RawRangeIndex.is_some() {
                return Err(berrors::ErrInvalidMetaFile(
                    "show raw kv with backup meta v2 isn't supported for now",
                ));
            }
            for rr in self.meta.GetBasic().RawRanges {
                res.RawRanges.push(convertRawRange(&rr));
            }
        }
        Ok(res)
    }
}

/// collectResult collects from an output channel `c` and an error channel `e`.
/// Items are converted by mapper `m`.
///
/// Mirrors Go select over ctx / item / error.
/// 模拟 Go select：优先排空 item，再看 error，超时轮询以响应 ctx 取消。
pub fn collectResult<T, R, M>(
    ctx: &Context,
    c: mpsc::Receiver<T>,
    e: mpsc::Receiver<Result<()>>,
    m: M,
) -> Result<Vec<R>>
where
    M: Fn(T) -> R,
{
    let mut collected = Vec::new();
    let mut err_done = false;
    loop {
        // 取消优先：避免在生产者仍运行时无限阻塞。
        if ctx.Done() {
            return Err(ctx.Err().unwrap_or_else(|| Error::new("context canceled")));
        }

        // Drain available items first (keeps pace with sync_channel backpressure).
        // 先排空可用 item，匹配 sync_channel(16) 背压节奏。
        match c.try_recv() {
            Ok(item) => {
                collected.push(m(item));
                // 映射后追加到结果。
                continue;
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                // Channel closed — still surface a pending non-nil error if any.
                // 输出已关闭：若错误通道仍有 Err，优先返回该错误。
                if !err_done {
                    if let Ok(Err(err)) = e.try_recv() {
                        return Err(err);
                    }
                }
                return Ok(collected);
            }
            Err(mpsc::TryRecvError::Empty) => {}
        }

        // 错误通道：Ok(()) 只标记完成；Err 立即失败；Disconnected 视为完成。
        if !err_done {
            match e.try_recv() {
                Ok(Err(err)) => return Err(err),
                Ok(Ok(())) => {
                    err_done = true;
                    // 错误通道正常结束。
                    continue;
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    err_done = true;
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }

        // 短超时等待：兼顾低延迟取消检测与避免忙等。
        match c.recv_timeout(Duration::from_millis(1)) {
            Ok(item) => collected.push(m(item)),
            // 超时等待后收到 item。
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if ctx.Done() {
                    return Err(ctx.Err().unwrap_or_else(|| Error::new("context canceled")));
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                if !err_done {
                    if let Ok(Err(err)) = e.try_recv() {
                        return Err(err);
                    }
                }
                return Ok(collected);
            }
        }
    }
}

/// convertTable projects `metautil.Table` into show output.
/// Info 缺失表示纯库记录，TableName 置空。
pub fn convertTable(t: MetaTable) -> Table {
    // The name table may be empty (which means this is a record for a database.)
    let tableName = if let Some(info) = t.Info.as_ref() {
        info.Name.String()
    } else {
        String::new()
    };
    Table {
        DBName: t.DB.Name.String(),
        TableName: tableName,
        KVCount: t.TotalKvs,
        KVSize: t.TotalBytes,
        TiFlashReplica: t.TiFlashReplicas as u64,
    }
}

/// convertRawRange copies RawKV range fields.
/// CF/起止键拷贝为展示用 HexBytes。
pub fn convertRawRange(r: &PbRawRange) -> RawRange {
    RawRange {
        ColumnFamily: r.Cf.clone(),
        StartKey: HexBytes(r.StartKey.clone()),
        EndKey: HexBytes(r.EndKey.clone()),
    }
}

/// convertBasic maps backupmeta basics; tables / raw ranges filled later.
/// 只填集群与版本窗口；Tables/RawRanges 由 Read 后续填充。
pub fn convertBasic(basic: BackupMeta) -> ShowResult {
    ShowResult {
        ClusterID: basic.ClusterId,
        ClusterVersion: basic.ClusterVersion,
        BRVersion: basic.BrVersion,
        Version: basic.Version,
        StartVersion: TimeStamp(basic.StartVersion),
        EndVersion: TimeStamp(basic.EndVersion),
        IsRawKV: basic.IsRawKv,
        RawRanges: Vec::new(),
        Tables: Vec::new(),
    }
}
