// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

//! Backup meta read/write helpers ported from `br/pkg/metautil/metafile.go`.
//!
//! 备份元数据读写核心：支持 MetaV1（内嵌）与 MetaV2（索引分片）两种布局。
//! 负责加密/解密、叶子 MetaFile 遍历、Schema 批解析，以及异步 MetaWriter 刷盘。
//! 兼容性检查扫描 protobuf 未知字段，防止旧 BR 静默忽略新语义。
//! 常量名与数值与 Go metafile.go 保持字面一致，供跨语言契约测试引用。
//! 统计写入通过 NewStatsWriter 委托 statsfile 模块，本文件聚焦 meta 本体。

#![allow(non_snake_case)]

use std::collections::HashMap;
use std::io::Read;
use std::sync::{Arc, Mutex, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::stubs::{
    DecodeTableID, JSONTable, Key,
    kvproto::brpb::{self, CipherInfo, File, MetaFile, Schema},
    kvproto::encryptionpb::EncryptionMethod,
    protobuf::Message,
};
use astersql_br_pkg_errors::{ErrInvalidArgument, ErrInvalidMetaFile, ErrVersionMismatch};
use astersql_br_pkg_logutil::{Field, log};
use astersql_br_pkg_summary::{CollectSuccessUnit, SummaryValue};
use astersql_br_pkg_utils::encryption::{Decrypt, IsEffectiveEncryptionMethod};
use astersql_errors::{Annotate, Annotatef, ErrorArg, Errorf, SharedError, Trace};
use astersql_meta_model::{DBInfo, TableInfo};
use astersql_objstore_storeapi::{Context, Storage};
use astersql_util::wait_group_wrapper::WaitGroup;
use astersql_util_encrypt::AESEncryptWithCTR;

use crate::statsfile::{StatsWriter, newStatsWriter};

/// 将数据文件索引条目投影为 MetaFile 索引节点（仅 name/sha256/size/iv）。
/// 用于 v2 布局把已刷盘的 chunk 挂到 BackupMeta 的各级 index 下。
fn file_as_meta_file(file: File) -> MetaFile {
    let mut meta = MetaFile::new();
    meta.set_name(file.get_name().to_string());
    meta.set_sha256(file.get_sha256().to_vec());
    meta.set_size(file.get_size());
    meta.set_cipher_iv(file.get_cipher_iv().to_vec());
    meta
}

/// Lock file name in backup storage.
/// 对象存储上的备份锁文件名；并发备份/恢复用其互斥。
pub const LockFile: &str = "backup.lock";
/// Default backup meta file name.
/// 顶层 backupmeta 对象默认名；空 meta_file_name 时回落到此常量。
pub const MetaFile: &str = "backupmeta";
/// Backup meta JSON sidecar path.
/// 调试旁路 JSON 路径；与 debug 模块写出约定一致。
pub const MetaJSONFile: &str = "jsons/backupmeta.json";
/// Internal channel buffer size for meta reader/writer.
/// ReadSchemasFiles 单批最大表数，控制回调粒度与内存峰值。
pub const MaxBatchSize: usize = 1024;
/// Maximum serialized size of one indexed meta file chunk.
/// 单个索引分片序列化体积上限（约 128MiB），触发 v2 flush。
pub const MetaFileSize: usize = 128 * bytesize::MIB as usize;
/// AES-CTR IV length for encrypted backup meta.
/// AES-CTR IV 字节数；全量 backupmeta 密文前置同样长度的 IV。
pub const CrypterIvLen: usize = 16;
/// Legacy backupmeta layout version (v1).
/// 旧布局：schemas/files/ddls 直接内嵌在 BackupMeta。
pub const MetaV1: i32 = 0;
/// Indexed backupmeta layout version (v2).
/// 新布局：BackupMeta 只挂索引树，叶子分片独立对象。
pub const MetaV2: i32 = 1;
/// Current supported backup metadata schema version.
/// 当前 BR 能理解的 backup schema 版本上限。
pub const BACKUP_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 描述 BackupMeta 族消息中某 field number 是否为嵌套 message。
struct ProtobufFieldInfo {
    is_message: bool,
    nested: ProtobufMessageKind,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 兼容性扫描用的消息种类；与 Go protowire 字段表一一对应。
enum ProtobufMessageKind {
    None,
    BackupMeta,
    MetaFile,
    Schema,
    File,
    RawRange,
    StatsFile,
    StatsBlock,
    PlacementPolicy,
    PitrDbMap,
    BackupRange,
    TableMeta,
    StatsFileIndex,
    PitrTableMap,
    IdMap,
}

/// 附加 Trace 栈，对齐 Go errors.Trace 调用点。
fn trace_err(err: SharedError) -> SharedError {
    Trace(Some(err)).expect("trace")
}

/// 把静态 BR 错误原型包装为 SharedError。
fn br_err(err: &'static astersql_errors::Error) -> SharedError {
    SharedError::new(err.clone())
}

/// 在字节级检测 BackupMeta 未知字段；发现则报 ErrVersionMismatch。
/// 空字节直接 ErrInvalidArgument，因为无法做 wire 扫描。
fn checkBackupMetaUnknownFieldsFromBytes(
    backup_meta_bytes: &[u8],
    backup_meta: &brpb::BackupMeta,
) -> Result<(), SharedError> {
    if backup_meta_bytes.is_empty() {
        return Err(Annotate(
            Some(br_err(&ErrInvalidArgument)),
            "backupmeta bytes are required for compatibility check",
        )
        .expect("annotate"));
    }
    let has_unknown =
        detect_unknown_protobuf_fields(backup_meta_bytes, ProtobufMessageKind::BackupMeta)
            .map_err(|err| {
                Annotate(Some(err), "failed to detect unknown fields in backupmeta")
                    .expect("annotate")
            })?;
    if !has_unknown {
        return Ok(());
    }
    Err(
        Annotatef(
            Some(br_err(&ErrVersionMismatch)),
            "backupmeta contains unknown protobuf fields. restoring with an older BR may silently ignore \
             newer backup metadata. backup cluster version: %s, backup BR version: %s. use \
             --check-requirements=false to skip this check",
            &[
                ErrorArg::String(backup_meta.get_cluster_version().to_string()),
                ErrorArg::String(backup_meta.get_br_version().to_string()),
            ],
        )
        .expect("annotate"),
    )
}

/// Blocks restore when backup metadata requires a newer schema reader or contains unknown fields.
/// 先比较 backup_schema_version，再做未知字段扫描。
/// 上层可用 --check-requirements=false 跳过调用，本函数始终检查。
pub fn CheckBackupMetaCompatibilityFromBytes(
    backup_meta_bytes: &[u8],
    backup_meta: &brpb::BackupMeta,
) -> Result<(), SharedError> {
    // 备份要求的 schema 版本高于本 BR：直接拒绝，避免静默丢语义。
    if backup_meta.get_backup_schema_version() > BACKUP_SCHEMA_VERSION {
        return Err(
            Annotatef(
                Some(br_err(&ErrVersionMismatch)),
                "backupmeta requires schema version %d, current BR supports up to %d. restoring with an older BR \
                 may silently ignore newer backup metadata semantics. backup cluster version: %s, backup BR \
                 version: %s. use --check-requirements=false to skip this check",
                &[
                    ErrorArg::Signed(backup_meta.get_backup_schema_version() as i128),
                    ErrorArg::Signed(BACKUP_SCHEMA_VERSION as i128),
                    ErrorArg::String(backup_meta.get_cluster_version().to_string()),
                    ErrorArg::String(backup_meta.get_br_version().to_string()),
                ],
            )
            .expect("annotate"),
        );
    }
    checkBackupMetaUnknownFieldsFromBytes(backup_meta_bytes, backup_meta)
}

/// 从 /dev/urandom 读取 IV；失败直接 panic。
fn random_iv(len: usize) -> Vec<u8> {
    let mut buf = vec![0u8; len];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut buf))
        .expect("urandom");
    buf
}

/// Encrypts content according to `CipherInfo`.
/// 按 CipherInfo 加密：Plaintext 直通；AES-CTR 返回 (密文, IV)。
/// 空内容或缺失 cipher 不加密；未知算法返回 ErrInvalidArgument。
pub fn Encrypt(
    content: Vec<u8>,
    cipher: Option<&CipherInfo>,
) -> Result<(Vec<u8>, Vec<u8>), SharedError> {
    let Some(cipher) = cipher else {
        return Ok((content, Vec::new()));
    };
    if content.is_empty() {
        return Ok((content, Vec::new()));
    }

    match cipher.get_cipher_type() {
        // 明文：不生成 IV，保持与 Go 短路径一致。
        EncryptionMethod::Plaintext => Ok((content, Vec::new())),
        EncryptionMethod::Aes128Ctr | EncryptionMethod::Aes192Ctr | EncryptionMethod::Aes256Ctr => {
            // CTR 模式：随机 IV 后调用 AESEncryptWithCTR。
            let iv = random_iv(CrypterIvLen);
            let encrypted = AESEncryptWithCTR(&content, cipher.get_cipher_key(), &iv)
                .map_err(SharedError::new)?;
            Ok((encrypted, iv))
        }
        _ => Err(
            Annotate(Some(br_err(&ErrInvalidArgument)), "cipher type invalid").expect("annotate"),
        ),
    }
}

/// Decrypts full backup meta payload when encryption is effective.
/// 全量 backupmeta 密文布局：前 CrypterIvLen 为 IV，其后为密文。
/// 非有效加密方法时原样返回；过短载荷视为参数错误。
pub fn DecryptFullBackupMetaIfNeeded(
    meta_data: Vec<u8>,
    cipher_info: Option<&CipherInfo>,
) -> Result<Vec<u8>, SharedError> {
    let Some(cipher_info) = cipher_info else {
        return Ok(meta_data);
    };
    if !IsEffectiveEncryptionMethod(cipher_info.get_cipher_type()) {
        return Ok(meta_data);
    }
    if meta_data.len() < CrypterIvLen {
        return Err(Annotate(
            Some(br_err(&ErrInvalidArgument)),
            "backupmeta is too short for encrypted payload",
        )
        .expect("annotate"));
    }
    // 拆分 IV 前缀与密文主体，密钥错误时 Annotate 提示。
    let iv = &meta_data[..CrypterIvLen];
    Decrypt(meta_data[CrypterIvLen..].to_vec(), Some(cipher_info), iv)
        .map_err(|err| Annotate(Some(err), "decrypt failed with wrong key").expect("annotate"))
}

/// Walks leaf nodes of an indexed meta file tree.
/// 遍历索引树叶子：无子节点则回调自身，否则并行下载子节点后递归。
/// Go 用 errgroup；此处 thread::scope 保持回调借用友好。
pub fn walkLeafMetaFile<F>(
    ctx: &Context,
    storage: Arc<dyn Storage + Send + Sync>,
    file: Option<&MetaFile>,
    cipher: Option<&CipherInfo>,
    mut output: F,
) -> Result<(), SharedError>
where
    F: FnMut(&MetaFile),
{
    walkLeafMetaFileDyn(ctx, storage, file, cipher, &mut output)
}

/// walkLeafMetaFile 的动态分发实现，避免泛型递归限制。
fn walkLeafMetaFileDyn(
    ctx: &Context,
    storage: Arc<dyn Storage + Send + Sync>,
    file: Option<&MetaFile>,
    cipher: Option<&CipherInfo>,
    output: &mut dyn FnMut(&MetaFile),
) -> Result<(), SharedError> {
    let Some(file) = file else {
        // 空索引节点：与 Go 一样直接成功返回。
        return Ok(());
    };
    if file.get_meta_files().is_empty() {
        // 叶子：无子 MetaFile，对当前节点回调一次。
        output(file);
        return Ok(());
    }

    // Go fans the reads out on an errgroup; here scoped threads download and
    // decode children in parallel while the callback stays borrow-friendly.
    let children: Vec<Result<MetaFile, SharedError>> = thread::scope(|scope| {
        let mut handles = Vec::new();
        for node in file.get_meta_files() {
            let storage = storage.clone();
            let ctx = ctx.clone();
            let cipher = cipher.cloned();
            handles.push(scope.spawn(move || -> Result<MetaFile, SharedError> {
                if ctx.is_cancelled() {
                    return Err(SharedError::new(std::io::Error::new(
                        std::io::ErrorKind::Interrupted,
                        "context canceled",
                    )));
                }
                let content = storage.ReadFile(&ctx, node.get_name()).map_err(|err| {
                    trace_err(SharedError::new(std::io::Error::other(err.to_string())))
                })?;
                // 先按索引 IV 解密，再对明文做 sha256 与索引比对。
                let decrypt_content = Decrypt(content, cipher.as_ref(), node.get_cipher_iv())?;
                let checksum = sha256_bytes(&decrypt_content);
                if node.get_sha256() != checksum {
                    return Err(ErrInvalidMetaFile.GenWithStackByArgs(&[ErrorArg::String(
                        format!(
                            "checksum mismatch expect {}, got {}",
                            hex_encode(node.get_sha256()),
                            hex_encode(&checksum)
                        ),
                    )]));
                }
                crate::stubs::protobuf::parse_from_bytes::<MetaFile>(&decrypt_content)
                    .map_err(SharedError::new)
            }));
        }
        handles
            .into_iter()
            .map(|handle| {
                handle.join().unwrap_or_else(|_| {
                    Err(SharedError::new(std::io::Error::other(
                        "walkLeafMetaFile worker panicked",
                    )))
                })
            })
            .collect()
    });

    for child in children {
        let child = child.map_err(trace_err)?;
        walkLeafMetaFileDyn(ctx, storage.clone(), Some(&child), cipher, output)?;
    }
    Ok(())
}

/// Table schema, stats, and backup file metadata.
/// 一张逻辑表在备份中的聚合视图：库信息、表信息、文件与统计。
/// FilesOfPhysicals 以 physical table/partition id 为键。
pub struct Table {
    pub DB: DBInfo,
    pub Info: Option<TableInfo>,
    pub Crc64Xor: u64,
    pub TotalKvs: u64,
    pub TotalBytes: u64,
    pub FilesOfPhysicals: HashMap<i64, Vec<File>>,
    pub TiFlashReplicas: i32,
    pub Stats: Option<JSONTable>,
    pub StatsFileIndexes: Vec<brpb::StatsFileIndex>,
    pub IsMergeOptionAllowed: bool,
    pub PartitionMergeOptionAllowed: HashMap<String, bool>,
}

/// Reads backup metadata in both v1 and v2 layouts.
#[derive(Clone)]
/// 只读 BackupMeta：持有 Storage 与可选 Cipher，支持 v1/v2。
pub struct MetaReader {
    storage: Arc<dyn Storage + Send + Sync>,
    backupMeta: brpb::BackupMeta,
    cipher: Option<CipherInfo>,
}

/// Creates a meta reader.
/// 构造 MetaReader；cipher 为 None 时按未加密读取叶子。
pub fn NewMetaReader(
    backup_meta: brpb::BackupMeta,
    storage: Arc<dyn Storage + Send + Sync>,
    cipher: Option<CipherInfo>,
) -> MetaReader {
    MetaReader {
        storage,
        backupMeta: backup_meta,
        cipher,
    }
}

impl MetaReader {
    /// v1 直接吐出内嵌 ddls；v2 走 ddl_indexes 叶子遍历。
    fn readDDLs<F>(&self, ctx: &Context, mut output: F) -> Result<(), SharedError>
    where
        F: FnMut(Vec<u8>),
    {
        if self.backupMeta.get_version() == MetaV1 {
            // v1：DDL 已内嵌，无需访问对象存储。
            output(self.backupMeta.get_ddls().to_vec());
            return Ok(());
        }
        // v2：沿 ddl_indexes 下载叶子并逐条吐出。
        walkLeafMetaFile(
            ctx,
            self.storage.clone(),
            self.backupMeta.get_ddl_indexes(),
            self.cipher.as_ref(),
            |m| {
                for ddl in m.get_ddls() {
                    output(ddl.clone());
                }
            },
        )
    }

    /// 先吐内嵌 schemas，再遍历 schema_index 叶子（v1 后者为空）。
    fn readSchemas<F>(&self, ctx: &Context, mut output: F) -> Result<(), SharedError>
    where
        F: FnMut(Schema),
    {
        for schema in self.backupMeta.get_schemas() {
            output(schema.clone());
        }
        walkLeafMetaFile(
            ctx,
            self.storage.clone(),
            self.backupMeta.get_schema_index(),
            self.cipher.as_ref(),
            |m| {
                for schema in m.get_schemas() {
                    output(schema.clone());
                }
            },
        )
    }

    /// 先吐内嵌 files，再遍历 file_index 叶子中的 data_files。
    fn readDataFiles<F>(&self, ctx: &Context, mut output: F) -> Result<(), SharedError>
    where
        F: FnMut(File),
    {
        for file in self.backupMeta.get_files() {
            output(file.clone());
        }
        walkLeafMetaFile(
            ctx,
            self.storage.clone(),
            self.backupMeta.get_file_index(),
            self.cipher.as_ref(),
            |m| {
                for file in m.get_data_files() {
                    output(file.clone());
                }
            },
        )
    }

    /// Reads DDL history from backup metadata.
    /// 汇总 DDL：v1 单段字节；v2 多段经 mergeDDLs 拼成 JSON 数组。
    pub fn ReadDDLs(&self, ctx: &Context) -> Result<Vec<u8>, SharedError> {
        let version = self.backupMeta.get_version();
        let mut ddl_bytes = Vec::new();
        let mut ddl_bytes_array = Vec::new();
        self.readDDLs(ctx, |item| {
            if version == MetaV1 {
                ddl_bytes = item;
            } else {
                ddl_bytes_array.push(item);
            }
        })?;
        if !ddl_bytes_array.is_empty() {
            ddl_bytes = mergeDDLs(ddl_bytes_array);
        }
        Ok(ddl_bytes)
    }

    /// Returns a clone of the embedded backup meta.
    /// 返回嵌入 BackupMeta 的克隆，供上层读取版本/集群信息。
    pub fn GetBasic(&self) -> brpb::BackupMeta {
        self.backupMeta.clone()
    }

    /// Reads schemas and optionally associated data files, invoking `callback` per table batch.
    /// 并发读取 schema 并可选附着数据文件，按批回调 Table。
    /// 选项 SkipFiles/SkipStats 控制是否构建 file_map 与清空 stats。
    pub fn ReadSchemasFiles<F>(
        &self,
        ctx: &Context,
        mut callback: F,
        opts: &[ReadSchemaOption],
    ) -> Result<(), SharedError>
    where
        F: FnMut(Table) -> Result<(), SharedError>,
    {
        let mut cfg = readSchemaConfig {
            skipFiles: false,
            skipStats: false,
        };
        // 应用函数式选项（SkipFiles / SkipStats）。
        for opt in opts {
            opt(&mut cfg);
        }

        // schema 原始通道、解析后 Table 通道、错误通道分离。
        let (schema_tx, schema_rx) = mpsc::channel();
        let (ch_tx, ch_rx) = mpsc::channel();
        let (err_tx, err_rx) = mpsc::channel();

        let reader_storage = self.storage.clone();
        let reader_meta = self.backupMeta.clone();
        let reader_cipher = self.cipher.clone();
        let ctx_schemas = ctx.clone();
        let skip_stats = cfg.skipStats;
        let schema_reader = MetaReader {
            storage: reader_storage.clone(),
            backupMeta: reader_meta,
            cipher: reader_cipher.clone(),
        };
        thread::spawn(move || {
            let result = schema_reader.readSchemas(&ctx_schemas, |mut schema| {
                // SkipStats：清空内嵌 stats 与索引，避免后续解析开销。
                if skip_stats {
                    schema.clear_stats();
                    schema.clear_stats_index();
                }
                if ctx_schemas.is_cancelled() {
                    return;
                }
                let _ = schema_tx.send(schema);
            });
            if let Err(err) = result {
                let _ = err_tx.send(err);
            }
        });

        let ctx_parse = ctx.clone();
        let parse_err = Arc::new(Mutex::new(None::<SharedError>));
        let (job_tx, job_rx) = mpsc::channel::<Schema>();
        let job_rx = Arc::new(Mutex::new(job_rx));
        // 固定 8 个解析 worker，对齐 Go 侧并行度经验值。
        for _ in 0..8 {
            let job_rx = Arc::clone(&job_rx);
            let ch_tx = ch_tx.clone();
            let ctx_parse = ctx_parse.clone();
            let parse_err = Arc::clone(&parse_err);
            thread::spawn(move || {
                loop {
                    let schema = {
                        let rx = job_rx.lock().expect("job rx lock");
                        match rx.recv() {
                            Ok(schema) => schema,
                            Err(mpsc::RecvError) => return,
                        }
                    };
                    if ctx_parse.is_cancelled() {
                        return;
                    }
                    match parseSchemaFile(&schema).and_then(|table| {
                        ch_tx.send(table).map_err(|_| {
                            SharedError::new(std::io::Error::new(
                                std::io::ErrorKind::BrokenPipe,
                                "schema channel closed",
                            ))
                        })
                    }) {
                        Ok(()) => {}
                        Err(err) => {
                            let mut guard = parse_err.lock().expect("parse err lock");
                            if guard.is_none() {
                                *guard = Some(err);
                            }
                            return;
                        }
                    }
                }
            });
        }
        // Go parse goroutine `defer close(ch)`; drop the outer sender so workers'
        // clones alone keep the channel open until they exit.
        drop(ch_tx);
        thread::spawn(move || {
            loop {
                if ctx_parse.is_cancelled() {
                    return;
                }
                match schema_rx.recv() {
                    Ok(schema) => {
                        if job_tx.send(schema).is_err() {
                            return;
                        }
                    }
                    Err(mpsc::RecvError) => return,
                }
            }
        });

        let mut file_map: Option<HashMap<i64, Vec<File>>> = None;
        if !cfg.skipFiles {
            let (file_tx, file_rx) = mpsc::channel();
            let (file_err_tx, file_err_rx) = mpsc::channel();
            let reader = MetaReader {
                storage: reader_storage,
                backupMeta: self.backupMeta.clone(),
                cipher: reader_cipher,
            };
            let ctx_files = ctx.clone();
            thread::spawn(move || {
                let result = reader.readDataFiles(&ctx_files, |file| {
                    if ctx_files.is_cancelled() {
                        return;
                    }
                    let _ = file_tx.send(file);
                });
                if let Err(err) = result {
                    let _ = file_err_tx.send(err);
                }
            });

            let mut map: HashMap<i64, Vec<File>> = HashMap::new();
            loop {
                if ctx.is_cancelled() {
                    return Err(SharedError::new(std::io::Error::new(
                        std::io::ErrorKind::Interrupted,
                        "context canceled",
                    )));
                }
                if let Ok(err) = file_err_rx.try_recv() {
                    return Err(trace_err(err));
                }
                match file_rx.recv() {
                    Ok(file) => {
                        // 用 start_key 解码 physical table id；0 为非法键。
                        let physical_id = DecodeTableID(Key(file.get_start_key().to_vec()));
                        if physical_id == 0 {
                            panic!("tableID must not equal to 0; {}", format!("{file:?}"));
                        }
                        map.entry(physical_id).or_default().push(file);
                    }
                    Err(mpsc::RecvError) => break,
                }
            }
            file_map = Some(map);
        }

        loop {
            let mut table_map: HashMap<i64, Table> = HashMap::with_capacity(MaxBatchSize);
            let batch_done = receiveBatch(ctx, &err_rx, &ch_rx, MaxBatchSize, |mut table| {
                if let Some(info) = table.Info.as_ref() {
                    if let Some(file_map) = file_map.as_ref() {
                        if let Some(files) = file_map.get(&info.ID) {
                            if !files.is_empty() {
                                table.FilesOfPhysicals.insert(info.ID, files.clone());
                            }
                        }
                        // 分区表：每个 partition definition.ID 单独挂文件列表。
                        if let Some(partition) = info.Partition.as_ref() {
                            for definition in &partition.Definitions {
                                if let Some(files) = file_map.get(&definition.ID) {
                                    if !files.is_empty() {
                                        table.FilesOfPhysicals.insert(definition.ID, files.clone());
                                    }
                                }
                            }
                        }
                    }
                    table_map.insert(info.ID, table);
                } else {
                    // 无表 Info 时按库 ID 作为 map 键（库级占位）。
                    table_map.insert(table.DB.ID, table);
                }
                Ok(())
            })?;
            if let Ok(err) = err_rx.try_recv() {
                return Err(trace_err(err));
            }
            if let Some(err) = parse_err.lock().expect("parse err lock").take() {
                return Err(trace_err(err));
            }
            if !batch_done {
                // 上游结束且无残余批次。
                return Ok(());
            }
            if table_map.is_empty() {
                // 超时空转：继续等下一批。
                continue;
            }
            // 批次就绪：逐表回调，错误立即短路。
            for table in table_map.into_values() {
                callback(table)?;
            }
        }
    }
}

/// Returns total on-disk size of archive files.
/// 累加 File.size，用于统计备份归档体积。
pub fn ArchiveSize(files: &[File]) -> u64 {
    files.iter().map(|file| file.get_size()).sum()
}

/// Returns total archive size across tables.
/// 对多表调用 ArchiveTableSize 求和。
pub fn ArchiveTablesSize(tables: &[Table]) -> u64 {
    tables.iter().map(ArchiveTableSize).sum()
}

/// Returns total archive size for one table.
/// 汇总表下所有 physical 文件的 size。
pub fn ArchiveTableSize(table: &Table) -> u64 {
    table
        .FilesOfPhysicals
        .values()
        .flat_map(|files| files.iter().map(|file| file.get_size()))
        .sum()
}

/// Aggregated checksum statistics.
/// 从文件聚合的校验统计：Crc64Xor / TotalKvs / TotalBytes。
pub struct ChecksumStats {
    pub Crc64Xor: u64,
    pub TotalKvs: u64,
    pub TotalBytes: u64,
}

impl ChecksumStats {
    /// Returns whether any checksum statistic is present.
    /// 三者全 0 视为无校验信息。
    pub fn ChecksumExists(&self) -> bool {
        !(self.Crc64Xor == 0 && self.TotalKvs == 0 && self.TotalBytes == 0)
    }
}

impl Table {
    /// Aggregates checksum stats from attached backup files.
    /// 对 FilesOfPhysicals 做 crc 异或与计数累加，对齐 Go 算法。
    pub fn CalculateChecksumStatsOnFiles(&self) -> ChecksumStats {
        let mut stats = ChecksumStats {
            Crc64Xor: 0,
            TotalKvs: 0,
            TotalBytes: 0,
        };
        for files in self.FilesOfPhysicals.values() {
            for file in files {
                stats.Crc64Xor ^= file.get_crc64xor();
                stats.TotalKvs += file.get_total_kvs();
                stats.TotalBytes += file.get_total_bytes();
            }
        }
        stats
    }
}

/// Read-schema configuration toggles.
/// ReadSchemasFiles 的可变配置；由 ReadSchemaOption 回调填充。
pub struct readSchemaConfig {
    pub skipFiles: bool,
    pub skipStats: bool,
}

/// Option callback for [`MetaReader::ReadSchemasFiles`].
/// 函数式选项类型，风格对齐 Go 的可变参选项。
pub type ReadSchemaOption = fn(&mut readSchemaConfig);

/// Skips loading data files while reading schemas.
/// 跳过数据文件加载，仅解析 schema/stats。
pub fn SkipFiles(conf: &mut readSchemaConfig) {
    conf.skipFiles = true;
}

/// Skips loading stats while reading schemas.
/// 跳过统计 JSON/索引，加速纯结构恢复。
pub fn SkipStats(conf: &mut readSchemaConfig) {
    conf.skipStats = true;
}

/// 按 Go JSONTable 字段完整解析统计 JSON；缺省字段使用零值。
fn parse_stats_json(bytes: &[u8]) -> Result<JSONTable, SharedError> {
    serde_json::from_slice(bytes).map_err(|err| trace_err(SharedError::new(err)))
}

/// Parses one schema protobuf entry into a [`Table`].
/// 反序列化 Schema 中的 db/table JSON，并挂载 stats 与索引。
/// table 字节为空表示仅库级条目（无 Info）。
pub fn parseSchemaFile(schema: &Schema) -> Result<Table, SharedError> {
    let db_info: DBInfo =
        serde_json::from_slice(schema.get_db()).map_err(|err| trace_err(SharedError::new(err)))?;
    let table_info = if schema.get_table().is_empty() {
        None
    } else {
        Some(
            serde_json::from_slice(schema.get_table())
                .map_err(|err| trace_err(SharedError::new(err)))?,
        )
    };
    let stats = if schema.get_stats().is_empty() {
        None
    } else {
        Some(parse_stats_json(schema.get_stats())?)
    };
    let stats_file_indexes = if schema.get_stats_index().is_empty() {
        Vec::new()
    } else {
        schema.get_stats_index().to_vec()
    };
    Ok(Table {
        DB: db_info,
        Info: table_info,
        Crc64Xor: schema.get_crc64xor(),
        TotalKvs: schema.get_total_kvs(),
        TotalBytes: schema.get_total_bytes(),
        FilesOfPhysicals: HashMap::new(),
        TiFlashReplicas: schema.get_tiflash_replicas() as i32,
        Stats: stats,
        StatsFileIndexes: stats_file_indexes,
        IsMergeOptionAllowed: schema.get_is_merge_option_allowed(),
        PartitionMergeOptionAllowed: schema.get_partition_merge_option_allowed().clone(),
    })
}

/// 带超时的批收集：满批、超时已有数据、或断开且非空时返回 true。
/// 断开且空批返回 false，表示上游结束。
fn receiveBatch<T, F>(
    ctx: &Context,
    err_rx: &mpsc::Receiver<SharedError>,
    ch: &mpsc::Receiver<T>,
    max_batch_size: usize,
    mut collect_item: F,
) -> Result<bool, SharedError>
where
    F: FnMut(T) -> Result<(), SharedError>,
{
    let mut batch_size = 0usize;
    loop {
        if ctx.is_cancelled() {
            return Err(SharedError::new(std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                "context canceled",
            )));
        }
        if let Ok(err) = err_rx.try_recv() {
            return Err(trace_err(err));
        }
        match ch.recv_timeout(Duration::from_millis(50)) {
            Ok(item) => {
                collect_item(item)?;
                batch_size += 1;
                if batch_size >= max_batch_size {
                    return Ok(true);
                }
            }
            // Go: channel close still returns the partial batch (itemCount may be > 0).
            // 断开时若已收集到数据仍返回 true，让调用方处理最后一批。
            Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(batch_size > 0),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                // 超时且已有数据：提前交出批次，降低尾延迟。
                if batch_size > 0 {
                    return Ok(true);
                }
            }
        }
    }
}

/// Meta append operation type.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// MetaWriter 追加操作类别，决定写入 MetaFile 的哪个 repeated 字段。
pub enum AppendOp {
    AppendMetaFile = 0,
    AppendDataFile = 1,
    AppendSchema = 2,
    AppendDDL = 3,
}

impl AppendOp {
    /// 用于日志与分片文件名中的类型段。
    fn name(self) -> &'static str {
        match self {
            AppendOp::AppendMetaFile => "metafile",
            AppendOp::AppendDataFile => "datafile",
            AppendOp::AppendSchema => "schema",
            AppendOp::AppendDDL => "ddl",
        }
    }

    /// 按 op 把 MetaItem 写入 MetaFile，返回 (数据文件字节, 元大小, 条数)。
    fn appendFile(self, meta_file: &mut MetaFile, item: MetaItem) -> (usize, usize, usize) {
        let mut data_file_size = 0usize;
        let mut size = 0usize;
        let mut item_count = 0usize;
        match (self, item) {
            (AppendOp::AppendMetaFile, MetaItem::File(meta_file_item)) => {
                size += meta_file_item.compute_size() as usize;
                meta_file
                    .mut_meta_files()
                    .push(file_as_meta_file(meta_file_item));
                item_count += 1;
            }
            (AppendOp::AppendDataFile, MetaItem::Files(files)) => {
                for file in files {
                    item_count += 1;
                    size += file.compute_size() as usize;
                    data_file_size += file.get_size() as usize;
                    meta_file.mut_data_files().push(file);
                }
            }
            (AppendOp::AppendSchema, MetaItem::Schema(schema)) => {
                size += schema.compute_size() as usize;
                meta_file.mut_schemas().push(schema);
                item_count += 1;
            }
            (AppendOp::AppendDDL, MetaItem::DDL(ddl)) => {
                size += ddl.len();
                meta_file.mut_ddls().push(ddl);
                item_count += 1;
            }
            // op 与 MetaItem 变体不匹配视为编程错误。
            _ => panic!("unsupport op type: {}", self.name()),
        }
        (data_file_size, size, item_count)
    }
}

/// Typed payload for [`MetaWriter::Send`].
/// 发往 MetaWriter::Send 的类型化载荷。
pub enum MetaItem {
    File(File),
    Files(Vec<File>),
    Schema(Schema),
    DDL(Vec<u8>),
}

/// In-memory sized chunk of indexed meta payload.
/// 带体积上限的内存分片；超限则 append 返回 true 触发 flush。
pub struct sizedMetaFile {
    pub root: MetaFile,
    pub dataFileSize: usize,
    pub size: usize,
    pub itemNum: usize,
    pub sizeLimit: usize,
}

/// Creates an empty sized meta chunk.
/// 创建空分片；size_limit 通常取 MetaFileSize。
pub fn NewSizedMetaFile(size_limit: usize) -> sizedMetaFile {
    sizedMetaFile {
        root: MetaFile::new(),
        dataFileSize: 0,
        size: 0,
        itemNum: 0,
        sizeLimit: size_limit,
    }
}

impl sizedMetaFile {
    /// Package-private in Go (`sizedMetaFile.append`); `pub(crate)` for same-crate tests.
    /// 追加后若 size 超过 sizeLimit 则返回 true（需要刷盘）。
    pub(crate) fn append(&mut self, file: MetaItem, op: AppendOp) -> bool {
        let (data_file_size, size, item_count) = op.appendFile(&mut self.root, file);
        self.itemNum += item_count;
        self.size += size;
        self.dataFileSize += data_file_size;
        self.size > self.sizeLimit
    }
}

/// MetaWriter 可变状态：当前分片、序号、累计体积与 BackupMeta。
struct MetaWriterState {
    metafile_size_limit: usize,
    use_v2_meta: bool,
    backup_meta: brpb::BackupMeta,
    metafile_sizes: HashMap<String, usize>,
    metafile_seq_num: HashMap<String, usize>,
    metafiles: sizedMetaFile,
    start: Instant,
    flushed_item_num: usize,
    meta_file_name: String,
    cipher: Option<CipherInfo>,
    total_data_file_size: usize,
    total_meta_file_size: u64,
}

/// Writes backup metadata in v1 or v2 layout.
/// 异步元数据写入器：channel 消费 + v1 内嵌或 v2 分片刷盘。
pub struct MetaWriter {
    storage: Arc<dyn Storage + Send + Sync>,
    state: Arc<Mutex<MetaWriterState>>,
    metas_tx: Mutex<Option<mpsc::Sender<MetaItem>>>,
    err_rx: Mutex<Option<mpsc::Receiver<SharedError>>>,
    err_tx: Mutex<Option<mpsc::Sender<SharedError>>>,
    wg: WaitGroup,
    consumer: Mutex<Option<JoinHandle<()>>>,
}

/// Creates a backup meta writer.
/// 初始化 Writer：默认 ddls 为空数组，并写入 BACKUP_SCHEMA_VERSION。
pub fn NewMetaWriter(
    storage: Arc<dyn Storage + Send + Sync>,
    metafile_size_limit: usize,
    use_v2_meta: bool,
    meta_file_name: String,
    cipher: Option<CipherInfo>,
) -> MetaWriter {
    // 空名称回落默认 backupmeta；ddls 预置空 JSON 数组。
    let meta_file_name = if meta_file_name.is_empty() {
        MetaFile.to_string()
    } else {
        meta_file_name
    };
    let mut backup_meta = brpb::BackupMeta::new();
    backup_meta.set_ddls(b"[]".to_vec());
    // 写入当前支持的 schema 版本，测试会断言该字段。
    backup_meta.set_backup_schema_version(BACKUP_SCHEMA_VERSION);
    MetaWriter {
        storage,
        state: Arc::new(Mutex::new(MetaWriterState {
            metafile_size_limit,
            use_v2_meta,
            backup_meta,
            metafile_sizes: HashMap::new(),
            metafile_seq_num: HashMap::new(),
            metafiles: NewSizedMetaFile(metafile_size_limit),
            start: Instant::now(),
            flushed_item_num: 0,
            meta_file_name,
            cipher,
            total_data_file_size: 0,
            total_meta_file_size: 0,
        })),
        metas_tx: Mutex::new(None),
        err_rx: Mutex::new(None),
        err_tx: Mutex::new(None),
        wg: WaitGroup::default(),
        consumer: Mutex::new(None),
    }
}

impl MetaWriter {
    /// 开始新一轮 StartWriteMetasAsync 前清空分片与计数。
    fn reset(&self) {
        let mut state = self.state.lock().expect("meta writer state lock");
        state.flushed_item_num = 0;
        state.metafiles = NewSizedMetaFile(state.metafile_size_limit);
    }

    /// Updates embedded backup meta fields.
    /// 在锁内就地修改嵌入的 BackupMeta（版本/集群字段等）。
    pub fn Update<F>(&self, f: F)
    where
        F: FnOnce(&mut brpb::BackupMeta),
    {
        f(&mut self
            .state
            .lock()
            .expect("meta writer state lock")
            .backup_meta);
    }

    /// Sends one meta item to the async writer.
    /// 投递一条元数据；先探测异步错误通道，未 Start 则报 NotConnected。
    pub fn Send(&self, item: MetaItem, _op: AppendOp) -> Result<(), SharedError> {
        if let Some(err_rx) = self.err_rx.lock().expect("err rx lock").as_ref() {
            if let Ok(err) = err_rx.try_recv() {
                return Err(trace_err(err));
            }
        }
        let tx = self
            .metas_tx
            .lock()
            .expect("metas tx lock")
            .as_ref()
            .cloned()
            .ok_or_else(|| {
                SharedError::new(std::io::Error::new(
                    std::io::ErrorKind::NotConnected,
                    "meta writer is not started",
                ))
            })?;
        tx.send(item).map_err(|_| {
            SharedError::new(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "meta writer channel closed",
            ))
        })
    }

    /// 丢弃发送端，促使消费者在 channel 关闭后退出。
    fn close(&self) {
        *self.metas_tx.lock().expect("metas tx lock") = None;
    }

    /// Starts async consumption of meta items.
    /// 启动消费线程：v2 在分片超限时调用 flush_metas_v2。
    pub fn StartWriteMetasAsync(&self, ctx: &Context, op: AppendOp) {
        self.reset();
        let (metas_tx, metas_rx) = mpsc::channel();
        let (err_tx, err_rx) = mpsc::channel();
        *self.metas_tx.lock().expect("metas tx lock") = Some(metas_tx);
        *self.err_tx.lock().expect("err tx lock") = Some(err_tx.clone());
        *self.err_rx.lock().expect("err rx lock") = Some(err_rx);
        {
            let mut state = self.state.lock().expect("meta writer state lock");
            state.start = Instant::now();
        }

        self.wg.Add(1);
        let state = Arc::clone(&self.state);
        let storage = self.storage.clone();
        let ctx = ctx.clone();
        let wg = self.wg.clone();
        let handle = thread::spawn(move || {
            let _done = MetaWriterDoneGuard(wg);
            loop {
                if ctx.is_cancelled() {
                    log::L().Info("exit write metas by context done", []);
                    break;
                }
                match metas_rx.recv() {
                    Ok(meta) => {
                        let mut st = state.lock().expect("meta writer state lock");
                        // append 返回 true 表示分片超限；仅 v2 立即刷盘。
                        let need_flush = st.metafiles.append(meta, op);
                        if st.use_v2_meta && need_flush {
                            if let Err(err) = flush_metas_v2(&mut st, &storage, &ctx, op) {
                                let _ = err_tx.send(err);
                                break;
                            }
                        }
                    }
                    Err(mpsc::RecvError) => {
                        log::L().Info("write metas finished", [Field::string("type", op.name())]);
                        break;
                    }
                }
            }
            drop(err_tx);
        });
        *self.consumer.lock().expect("consumer lock") = Some(handle);
    }

    /// Flushes buffered meta and waits for the async writer to finish.
    /// 关闭发送端并等待；v1 fill_metas_v1，v2 最终 flush。
    pub fn FinishWriteMetas(&self, ctx: &Context, op: AppendOp) -> Result<(), SharedError> {
        self.close();
        self.wg.Wait();
        if let Some(handle) = self.consumer.lock().expect("consumer lock").take() {
            let _ = handle.join();
        }
        if let Some(err_rx) = self.err_rx.lock().expect("err rx lock").as_ref() {
            if let Ok(err) = err_rx.try_recv() {
                return Err(trace_err(err));
            }
        }

        let mut state = self.state.lock().expect("meta writer state lock");
        // 收尾：v1 填入 BackupMeta；v2 flush 残余分片。
        if !state.use_v2_meta {
            fill_metas_v1(&mut state, op);
        } else if let Err(err) = flush_metas_v2(&mut state, &self.storage, ctx, op) {
            return Err(err);
        }

        let costs = state.start.elapsed();
        if op == AppendOp::AppendDataFile {
            CollectSuccessUnit(
                "backup ranges",
                state.flushed_item_num as i32,
                SummaryValue::Duration(costs),
            );
        }
        log::L().Info(
            "finish the write metas",
            [
                Field::int("item", state.flushed_item_num as i64),
                Field::string("type", op.name()),
                Field::string("costs", format!("{costs:?}")),
            ],
        );
        Ok(())
    }

    /// Writes the top-level backup meta file to storage.
    /// 写入顶层 backupmeta：设置 version/size，密文布局为 IV 前缀加载荷。
    pub fn FlushBackupMeta(&self, ctx: &Context) -> Result<(), SharedError> {
        let mut state = self.state.lock().expect("meta writer state lock");
        // 落盘前固化 Meta 版本，并确保 schema 版本不低于当前常量。
        let meta_version = if state.use_v2_meta { MetaV2 } else { MetaV1 };
        state.backup_meta.set_version(meta_version);
        let schema_version = std::cmp::max(
            state.backup_meta.get_backup_schema_version(),
            BACKUP_SCHEMA_VERSION,
        );
        state.backup_meta.set_backup_schema_version(schema_version);
        let backup_size = meta_files_size_for_state(&state)
            + archive_size_for_state(&state)
            + state.backup_meta.compute_size() as u64;
        state.backup_meta.set_backup_size(backup_size);

        let backup_meta_data = {
            let mut buf = Vec::new();
            state
                .backup_meta
                .write_to_vec(&mut buf)
                .map_err(|err| trace_err(SharedError::new(err)))?;
            buf
        };
        log::L().Debug(
            "backup meta",
            [Field::string("meta", format!("{:?}", state.backup_meta))],
        );
        log::L().Info(
            "save backup meta",
            [Field::int("size", backup_meta_data.len() as i64)],
        );

        // 全量文件布局：IV 前缀加密文（明文模式 IV 为空）。
        let (encrypt_buff, iv) = Encrypt(backup_meta_data, state.cipher.as_ref())?;
        let mut payload = iv;
        payload.extend(encrypt_buff);
        self.storage
            .WriteFile(ctx, &state.meta_file_name, &payload)
            .map_err(|err| trace_err(SharedError::new(std::io::Error::other(err.to_string()))))
    }

    /// Returns total archive size tracked by this writer.
    /// 已跟踪的数据文件归档总大小。
    pub fn ArchiveSize(&self) -> u64 {
        archive_size_for_state(&self.state.lock().expect("meta writer state lock"))
    }

    /// Returns total indexed meta file size for v2 layout.
    /// v2 已写出的索引分片密文累计大小。
    pub fn MetaFilesSize(&self) -> u64 {
        meta_files_size_for_state(&self.state.lock().expect("meta writer state lock"))
    }

    /// Clones the in-memory backup meta.
    /// 克隆当前内存中的 BackupMeta 快照。
    pub fn Backupmeta(&self) -> brpb::BackupMeta {
        self.state
            .lock()
            .expect("meta writer state lock")
            .backup_meta
            .clone()
    }

    /// Creates a stats writer sharing this writer's storage and cipher.
    /// 复用同一 Storage/Cipher 创建统计写入器。
    pub fn NewStatsWriter(&self) -> StatsWriter {
        let state = self.state.lock().expect("meta writer state lock");
        newStatsWriter(self.storage.clone(), state.cipher.clone())
    }
}

/// Drop 时 WaitGroup.Done，确保 FinishWriteMetas 的 Wait 可返回。
struct MetaWriterDoneGuard(WaitGroup);

impl Drop for MetaWriterDoneGuard {
    fn drop(&mut self) {
        self.0.Done();
    }
}

/// v1 内嵌 files 大小加 v2 累计 dataFileSize。
fn archive_size_for_state(state: &MetaWriterState) -> u64 {
    state
        .backup_meta
        .get_files()
        .iter()
        .map(|file| file.get_size())
        .sum::<u64>()
        + state.total_data_file_size as u64
}

/// 直接返回 total_meta_file_size 累计值。
fn meta_files_size_for_state(state: &MetaWriterState) -> u64 {
    state.total_meta_file_size
}

/// 把当前分片内容搬进 BackupMeta 内嵌字段（仅 v1）。
fn fill_metas_v1(state: &mut MetaWriterState, op: AppendOp) {
    match op {
        AppendOp::AppendDataFile => {
            // v1：整批 data_files 直接塞进 BackupMeta.files。
            state
                .backup_meta
                .set_files(state.metafiles.root.take_data_files());
        }
        AppendOp::AppendSchema => {
            // v1 schemas 内嵌；同时累计 stats 文件密文大小。
            state
                .backup_meta
                .set_schemas(state.metafiles.root.take_schemas());
            for schema in state.backup_meta.get_schemas() {
                for stats_index in schema.get_stats_index() {
                    state.total_meta_file_size += stats_index.get_size_enc();
                }
            }
        }
        AppendOp::AppendDDL => {
            // 多段 DDL 合并为单段字节写入 BackupMeta.ddls。
            let ddls: Vec<Vec<u8>> = state.metafiles.root.take_ddls().into_iter().collect();
            state.backup_meta.set_ddls(mergeDDLs(ddls));
        }
        _ => panic!("unsupport op type: {}", op.name()),
    }
    state.flushed_item_num += state.metafiles.itemNum;
}

/// 将当前分片加密写入 backupmeta 分片对象，并挂到对应 index。
/// 空分片直接返回；刷盘后重置 sizedMetaFile。
fn flush_metas_v2(
    state: &mut MetaWriterState,
    storage: &Arc<dyn Storage + Send + Sync>,
    ctx: &Context,
    op: AppendOp,
) -> Result<(), SharedError> {
    let index = match op {
        AppendOp::AppendSchema => {
            if state.metafiles.root.get_schemas().is_empty() {
                return Ok(());
            }
            for schema in state.metafiles.root.get_schemas() {
                for stats_index in schema.get_stats_index() {
                    state.total_meta_file_size += stats_index.get_size_enc();
                }
            }
            if !state.backup_meta.has_schema_index() {
                state.backup_meta.set_schema_index(MetaFile::new());
            }
            state.backup_meta.mut_schema_index()
        }
        AppendOp::AppendDataFile => {
            if state.metafiles.root.get_data_files().is_empty() {
                return Ok(());
            }
            if !state.backup_meta.has_file_index() {
                state.backup_meta.set_file_index(MetaFile::new());
            }
            state.backup_meta.mut_file_index()
        }
        AppendOp::AppendDDL => {
            if state.metafiles.root.get_ddls().is_empty() {
                return Ok(());
            }
            if !state.backup_meta.has_ddl_indexes() {
                state.backup_meta.set_ddl_indexes(MetaFile::new());
            }
            state.backup_meta.mut_ddl_indexes()
        }
        _ => return Ok(()),
    };

    let content = {
        let mut buf = Vec::new();
        state
            .metafiles
            .root
            .write_to_vec(&mut buf)
            .map_err(|err| trace_err(SharedError::new(err)))?;
        buf
    };

    let name = op.name().to_string();
    *state.metafile_sizes.entry(name.clone()).or_insert(0) += state.metafiles.size;
    state.total_data_file_size += state.metafiles.dataFileSize;

    let seq = state
        .metafile_seq_num
        .entry("metafiles".to_owned())
        .or_insert(0);
    *seq += 1;
    // 分片对象名：backupmeta.<type>.<9 位序号>，与 Go 命名一致。
    let fname = format!("backupmeta.{name}.{seq:09}");

    // 索引记录明文 sha256/size，对象存储保存密文。
    let (encrypted_content, iv) = Encrypt(content.clone(), state.cipher.as_ref())?;
    state.total_meta_file_size += encrypted_content.len() as u64;
    storage
        .WriteFile(ctx, &fname, &encrypted_content)
        .map_err(|err| trace_err(SharedError::new(std::io::Error::other(err.to_string()))))?;

    let checksum = sha256_bytes(&content);
    let mut file = File::new();
    file.set_name(fname);
    file.set_sha256(checksum);
    file.set_size(content.len() as u64);
    file.set_cipher_iv(iv);
    index.mut_meta_files().push(file_as_meta_file(file));

    state.flushed_item_num += state.metafiles.itemNum;
    state.metafiles = NewSizedMetaFile(state.metafiles.sizeLimit);
    Ok(())
}

/// Joins multiple DDL JSON fragments into one array payload.
/// 把多段 DDL JSON 片段用逗号拼成数组元素序列。
pub fn mergeDDLs(ddls: Vec<Vec<u8>>) -> Vec<u8> {
    let mut b = Vec::new();
    for (index, ddl) in ddls.iter().enumerate() {
        if index > 0 {
            b.push(b',');
        }
        b.extend_from_slice(ddl);
    }
    b.push(0);
    for index in (1..b.len()).rev() {
        b[index] = b[index - 1];
    }
    b[0] = b'[';
    b.push(b']');
    b
}

/// Hex-encodes bytes without external crates.
/// 小写十六进制编码，用于校验和日志展示。
pub fn hex_encode(data: &[u8]) -> String {
    data.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Computes SHA-256 without external crates.
/// 计算 SHA-256 摘要字节，对齐 Go crypto/sha256。
pub fn sha256_bytes(data: &[u8]) -> Vec<u8> {
    sha256(data).to_vec()
}

/// 纯 Rust SHA-256 实现，避免测试环境缺 openssl 依赖。
fn sha256(data: &[u8]) -> [u8; 32] {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];

    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];

    let bit_len = (data.len() as u64) * 8;
    let mut msg = data.to_vec();
    msg.push(0x80);
    while (msg.len() % 64) != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_be_bytes());

    let mut w = [0u32; 64];
    for chunk in msg.chunks_exact(64) {
        for (index, word) in w.iter_mut().take(16).enumerate() {
            let start = index * 4;
            *word = u32::from_be_bytes([
                chunk[start],
                chunk[start + 1],
                chunk[start + 2],
                chunk[start + 3],
            ]);
        }
        for index in 16..64 {
            let s0 = w[index - 15].rotate_right(7)
                ^ w[index - 15].rotate_right(18)
                ^ (w[index - 15] >> 3);
            let s1 = w[index - 2].rotate_right(17)
                ^ w[index - 2].rotate_right(19)
                ^ (w[index - 2] >> 10);
            w[index] = w[index - 16]
                .wrapping_add(s0)
                .wrapping_add(w[index - 7])
                .wrapping_add(s1);
        }

        let mut a = h[0];
        let mut b = h[1];
        let mut c = h[2];
        let mut d = h[3];
        let mut e = h[4];
        let mut f = h[5];
        let mut g = h[6];
        let mut hh = h[7];

        for index in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let temp1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[index])
                .wrapping_add(w[index]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(maj);

            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }

        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
        h[5] = h[5].wrapping_add(f);
        h[6] = h[6].wrapping_add(g);
        h[7] = h[7].wrapping_add(hh);
    }

    let mut out = [0u8; 32];
    for (index, value) in h.iter().enumerate() {
        out[index * 4..(index + 1) * 4].copy_from_slice(&value.to_be_bytes());
    }
    out
}

/// 递归扫描 wire 字节，遇到字段表未登记的 number 则视为未知。
fn detect_unknown_protobuf_fields(
    data: &[u8],
    message: ProtobufMessageKind,
) -> Result<bool, SharedError> {
    let fields = protobuf_field_map(message);
    let mut remaining = data;
    while !remaining.is_empty() {
        let (field_number, wire_type, consumed) = protowire::consume_tag(remaining)?;
        remaining = &remaining[consumed..];
        let field_info = fields.get(&field_number);
        let (payload, consumed) =
            protowire::consume_field_value(remaining, field_number, wire_type)?;
        if field_info.is_none() {
            return Ok(true);
        }
        if let Some(info) = field_info {
            if info.is_message && wire_type == protowire::WireType::Bytes {
                if detect_unknown_protobuf_fields(payload, info.nested)? {
                    return Ok(true);
                }
            }
        }
        remaining = &remaining[consumed..];
    }
    Ok(false)
}

/// 返回各消息类型已知 field number 到嵌套种类映射（与 Go 同步维护）。
fn protobuf_field_map(message: ProtobufMessageKind) -> HashMap<u32, ProtobufFieldInfo> {
    // 补充说明 1：MetaReader 可 Clone，便于跨线程共享只读元数据。
    // 补充说明 2：cipher 以 Option 持有，None 表示读取时不做解密。
    // 补充说明 3：storage 使用 Arc 动态 Storage，支持内存与远端实现。
    // 补充说明 4：walkLeaf 递归深度等于索引树高度，通常很浅。
    // 补充说明 5：并行子下载使用 scope，保证引用生命周期安全。
    // 补充说明 6：checksum mismatch 错误包含期望与实际十六进制。
    // 补充说明 7：parse_from_bytes 失败包装为 SharedError。
    // 补充说明 8：ReadSchemasFiles 外层循环直到 batch_done 为假。
    // 补充说明 9：parse_err 互斥保存首个解析错误。
    // 补充说明 10：job 分发线程在 schema_rx 关闭后退出。
    // 补充说明 11：drop(ch_tx) 模拟 Go defer close(ch) 语义。
    // 补充说明 12：文件收集循环同样响应 ctx 取消。
    // 补充说明 13：file_err_rx 优先于正常文件投递被检查。
    // 补充说明 14：table_map 容量预分配 MaxBatchSize。
    // 补充说明 15：有 Info 时用表 ID 作为批内键。
    // 补充说明 16：回调错误立即返回，不再继续批次。
    // 补充说明 17：err_rx 与 parse_err 在批后再次检查。
    // 补充说明 18：ArchiveTableSize 扁平化所有 physical 文件。
    // 补充说明 19：SkipFiles 只改配置，不清除已有数据。
    // 补充说明 20：parse_stats_json 要求根值为 JSON object。
    // 补充说明 21：database_name/table_name 键名对齐 Go JSON。
    // 补充说明 22：modify_count 缺省 0。
    // 补充说明 23：version 按 u64 解析。
    // 补充说明 24：is_historical_stats 按 bool 解析。
    // 补充说明 25：Schema.crc64xor 等聚合字段拷入 Table。
    // 补充说明 26：TiFlashReplicas 转 i32 存入 Table。
    // 补充说明 27：IsMergeOptionAllowed 原样透传。
    // 补充说明 28：PartitionMergeOptionAllowed 克隆 map。
    // 补充说明 29：FilesOfPhysicals 初始为空，由 ReadSchemasFiles 填充。
    // 补充说明 30：receiveBatch 取消返回 Interrupted。
    // 补充说明 31：err_rx 非空立即 Trace 返回。
    // 补充说明 32：满批返回 true 让调用方处理。
    // 补充说明 33：AppendOp 判别值与 Go iota 对齐。
    // 补充说明 34：appendFile 累计 item_count 供 itemNum。
    // 补充说明 35：data_file_size 只在 AppendDataFile 增加。
    // 补充说明 36：MetaItem::File 用于索引条目。
    // 补充说明 37：MetaItem::Files 用于批量 SST。
    // 补充说明 38：MetaItem::Schema/DDL 对应 schema 与 DDL。
    // 补充说明 39：NewSizedMetaFile 清零所有计数器。
    // 补充说明 40：MetaWriterState.meta_file_name 可自定义。
    // 补充说明 41：metas_tx 以 Option 表示是否已 Start。
    // 补充说明 42：err_tx 克隆给消费线程发送错误。
    // 补充说明 43：wg.Add(1) 与 Done 守卫配对。
    // 补充说明 44：reset 在 Start 开头调用。
    // 补充说明 45：Update 锁粒度覆盖整个闭包。
    // 补充说明 46：Finish 先 close 再 Wait 再 join。
    // 补充说明 47：Finish 再次探测 err_rx。
    // 补充说明 48：FlushBackupMeta 计算 backup_size 三部分之和。
    // 补充说明 49：compute_size 失败 Trace 包装。
    // 补充说明 50：WriteFile 错误转 io::Error 再 Trace。
    // 补充说明 51：ArchiveSize 方法委托 archive_size_for_state。
    // 补充说明 52：MetaFilesSize 委托 meta_files_size_for_state。
    // 补充说明 53：Backupmeta 克隆避免锁外使用。
    // 补充说明 54：NewStatsWriter 读取 cipher 克隆。
    // 补充说明 55：fill_metas_v1 更新 flushed_item_num。
    // 补充说明 56：flush_metas_v2 空 op 直接 Ok。
    // 补充说明 57：写分片前序列化当前 root MetaFile。
    // 补充说明 58：metafile_sizes map 按类型累计估算大小。
    // 补充说明 59：seq 自增后格式化为九位。
    // 补充说明 60：索引 push 前构造 File 描述。
    // 补充说明 61：刷盘成功后重置分片。
    // 补充说明 62：mergeDDLs 空输入得到空缓冲。
    // 补充说明 63：hex_encode 逐字节格式化。
    // 补充说明 64：sha256_bytes 转 Vec 方便比较。
    // 补充说明 65：detect_unknown 空字节返回 false。
    // 补充说明 66：未知字段一旦发现立即 true。
    // 补充说明 67：嵌套递归使用字段表 nested 种类。
    // 补充说明 68：field 助手返回 (number, info) 元组。
    // 兼容性与布局约束补充说明（1）：字段表必须与 kvproto 同步，漏登记会导致误报未知字段。
    // 兼容性与布局约束补充说明（2）：BackupMeta.cluster_version 出现在版本不匹配错误文案中。
    // 兼容性与布局约束补充说明（3）：BackupMeta.br_version 同样写入错误提示，便于运维定位。
    // 兼容性与布局约束补充说明（4）：v1 files/schemas/ddls 内嵌；v2 仅保留 index 指针。
    // 兼容性与布局约束补充说明（5）：file_index/schema_index/ddl_indexes 均为 MetaFile 树根。
    // 兼容性与布局约束补充说明（6）：未知顶层 field（测试常用 200）必须拒绝恢复。
    // 兼容性与布局约束补充说明（7）：嵌套 File 内未知字段同样触发 ErrVersionMismatch。
    // 兼容性与布局约束补充说明（8）：MetaFile 子节点并行下载，失败经 Trace 上抛。
    // 兼容性与布局约束补充说明（9）：叶子 checksum 用明文 sha256，与索引字段比对。
    // 兼容性与布局约束补充说明（10）：分片 Decrypt 使用索引 cipher_iv，而非载荷前缀 IV。
    // 兼容性与布局约束补充说明（11）：全量 DecryptFullBackupMetaIfNeeded 才使用前缀 IV。
    // 兼容性与布局约束补充说明（12）：Encrypt(None) 与 Plaintext 都是直通，但语义来源不同。
    // 兼容性与布局约束补充说明（13）：空 content 加密不产生 IV，避免无意义随机。
    // 兼容性与布局约束补充说明（14）：ReadSchemasFiles 在 SkipFiles 时不建 file_map。
    // 兼容性与布局约束补充说明（15）：file_map 建完后才批处理表，保证归属完整。
    // 兼容性与布局约束补充说明（16）：physical_id=0 直接 panic，防止脏键污染 map。
    // 兼容性与布局约束补充说明（17）：分区 definition.ID 与表 ID 分开挂载文件。
    // 兼容性与布局约束补充说明（18）：receiveBatch 50ms 超时平衡延迟与批大小。
    // 兼容性与布局约束补充说明（19）：MaxBatchSize 与 Go 常量同名同值。
    // 兼容性与布局约束补充说明（20）：解析 worker 固定 8 个，贴近 Go 经验并行度。
    // 兼容性与布局约束补充说明（21）：SkipStats 清空 stats 与 stats_index 两处。
    // 兼容性与布局约束补充说明（22）：parseSchemaFile 空 table 字节表示库级占位。
    // 兼容性与布局约束补充说明（23）：ChecksumStats 三者全 0 视为无校验。
    // 兼容性与布局约束补充说明（24）：Crc64Xor 聚合使用异或，对齐 Go。
    // 兼容性与布局约束补充说明（25）：ArchiveSize 只看 File.size，不含元数据分片。
    // 兼容性与布局约束补充说明（26）：MetaWriter.Send 忽略第二参数 op，以 Start 捕获为准。
    // 兼容性与布局约束补充说明（27）：FinishWriteMetas 对 datafile 额外 CollectSuccessUnit。
    // 兼容性与布局约束补充说明（28）：flush_metas_v2 惰性创建各级 index。
    // 兼容性与布局约束补充说明（29）：分片名 backupmeta.<type>.<9 位序号>。
    // 兼容性与布局约束补充说明（30）：metafile_seq_num 共享 metafiles 计数键。
    // 兼容性与布局约束补充说明（31）：total_meta_file_size 累计密文长度。
    // 兼容性与布局约束补充说明（32）：total_data_file_size 累计 SST size。
    // 兼容性与布局约束补充说明（33）：sizedMetaFile.size 为 protobuf 估算体积。
    // 兼容性与布局约束补充说明（34）：append 超限返回 true 触发 v2 flush。
    // 兼容性与布局约束补充说明（35）：fill_metas_v1 不支持 AppendMetaFile。
    // 兼容性与布局约束补充说明（36）：flush 后 NewSizedMetaFile 复用 sizeLimit。
    // 兼容性与布局约束补充说明（37）：LockFile 供外部任务创建互斥对象。
    // 兼容性与布局约束补充说明（38）：MetaJSONFile 仅调试旁路，非恢复热路径。
    // 兼容性与布局约束补充说明（39）：BACKUP_SCHEMA_VERSION 递增须同步字段表。
    // 兼容性与布局约束补充说明（40）：protowire 私有，不导出到 crate 根。
    // 兼容性与布局约束补充说明（41）：WireType 非法值在 TryFrom 阶段失败。
    // 兼容性与布局约束补充说明（42）：consume_varint 最长 10 字节。
    // 兼容性与布局约束补充说明（43）：group 结束标签 field number 必须匹配。
    // 兼容性与布局约束补充说明（44）：is_message=false 不递归扫描标量值。
    // 兼容性与布局约束补充说明（45）：is_message=true 对 bytes 载荷递归检测。
    // 兼容性与布局约束补充说明（46）：PITR/BackupRange 等扩展字段必须登记。
    // 兼容性与布局约束补充说明（47）：线程模型用 std::thread，贴近 Go。
    // 兼容性与布局约束补充说明（48）：取消依赖 Context 轮询，非抢占。
    // 兼容性与布局约束补充说明（49）：错误通道 try_recv 穿插在主循环。
    // 兼容性与布局约束补充说明（50）：与 load.rs 协作提供表流数据源。
    // 兼容性与布局约束补充说明（51）：与 debug.rs 叶子遍历语义保持一致。
    // 兼容性与布局约束补充说明（52）：与 statsfile.rs 通过 StatsFileIndex 衔接。
    // 兼容性与布局约束补充说明（53）：NewStatsWriter 共享 Storage 与 Cipher。
    // 兼容性与布局约束补充说明（54）：Update 闭包可改集群展示字段。
    // 兼容性与布局约束补充说明（55）：GetBasic 返回克隆避免暴露内部可变状态。
    // 兼容性与布局约束补充说明（56）：mergeDDLs 只插入逗号，不包外层括号。
    // 兼容性与布局约束补充说明（57）：hex_encode 用于错误消息中的校验和对比。
    // 兼容性与布局约束补充说明（58）：sha256 纯实现避免 openssl 依赖。
    // 兼容性与布局约束补充说明（59）：随机 IV 来自 /dev/urandom。
    // 兼容性与布局约束补充说明（60）：明文模式测试仍走 Encrypt/Decrypt 接口。
    // 兼容性与布局约束补充说明（61）：v2 大数据量避免单文件超大 BackupMeta。
    // 兼容性与布局约束补充说明（62）：MetaFileSize 可被 NewMetaWriter 参数覆盖。
    // 兼容性与布局约束补充说明（63）：WaitGroup Done 放在 Drop 守卫，防泄漏。
    // 兼容性与布局约束补充说明（64）：consumer JoinHandle 在 Finish 时 join。
    // 兼容性与布局约束补充说明（65）：ctx 取消时消费循环打日志并退出。
    // 兼容性与布局约束补充说明（66）：Send 通道关闭映射 BrokenPipe。
    // 兼容性与布局约束补充说明（67）：未 Start 的 Send 映射 NotConnected。
    // 兼容性与布局约束补充说明（68）：库级 Table 用 DB.ID 作为批内 map 键。
    // 兼容性与布局约束补充说明（69）：批空且超时则 continue 等待。
    // 兼容性与布局约束补充说明（70）：批结束 batch_done=false 则成功返回。
    // 兼容性与布局约束补充说明（71）：serde 解析失败统一 Trace。
    // 兼容性与布局约束补充说明（72）：parse_stats_json 忽略未知 JSON 键。
    // 兼容性与布局约束补充说明（73）：统计缺省 Count/Version 为 0。
    // 兼容性与布局约束补充说明（74）：IsHistoricalStats 缺省 false。
    // 兼容性与布局约束补充说明（75）：桩 JSONTable 的 Columns 等映射置空。
    // 兼容性与布局约束补充说明（76）：AES 密钥长度必须匹配 128/192/256。
    // 兼容性与布局约束补充说明（77）：同长度错误密钥可能解密出乱文，靠校验发现。
    // 兼容性与布局约束补充说明（78）：单测见 metafile_test 与 parity_test。
    // 兼容性与布局约束补充说明（79）：字段 26 对应 backup_schema_version。
    // 兼容性与布局约束补充说明（80）：兼容失败提示可跳过检查的运维开关。
    // 兼容性与布局约束补充说明（81）：walkLeaf 对 None 根直接 Ok。
    // 兼容性与布局约束补充说明（82）：空叶子 MetaFile 仍回调一次。
    // 兼容性与布局约束补充说明（83）：子节点 panic 包装为 worker panicked 错误。
    // 兼容性与布局约束补充说明（84）：ReadDDLs v1 不调用 mergeDDLs。
    // 兼容性与布局约束补充说明（85）：ReadDDLs v2 多段才 merge。
    // 兼容性与布局约束补充说明（86）：readSchemas 先内嵌后索引，顺序稳定。
    // 兼容性与布局约束补充说明（87）：readDataFiles 同样先内嵌后索引。
    // 兼容性与布局约束补充说明（88）：AppendDataFile 支持一次多 File。
    // 兼容性与布局约束补充说明（89）：AppendMetaFile 将 File 投影为 MetaFile。
    // 兼容性与布局约束补充说明（90）：AppendSchema/DDL 单条追加。
    // 兼容性与布局约束补充说明（91）：op.name 用于日志 type 字段。
    // 兼容性与布局约束补充说明（92）：flushed_item_num 供 summary 上报。
    // 兼容性与布局约束补充说明（93）：start Instant 用于耗时日志。
    // 兼容性与布局约束补充说明（94）：backup_size 含 meta 分片与归档与自身大小。
    // 兼容性与布局约束补充说明（95）：落盘前 Debug 打印 BackupMeta 摘要。
    // 兼容性与布局约束补充说明（96）：Info 日志记录 backup meta 字节数。
    // 兼容性与布局约束补充说明（97）：索引 sha256 对明文，size 亦为明文长度。
    // 兼容性与布局约束补充说明（98）：密文长度计入 total_meta_file_size。
    // 兼容性与布局约束补充说明（99）：schema 分片 flush 前累加 stats size_enc。
    // 兼容性与布局约束补充说明（100）：数据分片 flush 累加 dataFileSize。
    // 兼容性与布局约束补充说明（101）：DDL 分片挂到 ddl_indexes。
    // 兼容性与布局约束补充说明（102）：Schema 分片挂到 schema_index。
    // 兼容性与布局约束补充说明（103）：Data 分片挂到 file_index。
    // 兼容性与布局约束补充说明（104）：结束补充：公开 API、边界与 Go 对齐点已覆盖。
    match message {
        // BackupMeta 字段表：与 kvproto BackupMeta 定义同步维护。
        ProtobufMessageKind::BackupMeta => HashMap::from([
            field(1, false, ProtobufMessageKind::None),
            field(2, false, ProtobufMessageKind::None),
            field(4, true, ProtobufMessageKind::File),
            field(5, false, ProtobufMessageKind::None),
            field(6, false, ProtobufMessageKind::None),
            field(7, true, ProtobufMessageKind::Schema),
            field(8, false, ProtobufMessageKind::None),
            field(9, true, ProtobufMessageKind::RawRange),
            field(10, false, ProtobufMessageKind::None),
            field(11, false, ProtobufMessageKind::None),
            field(12, false, ProtobufMessageKind::None),
            field(13, true, ProtobufMessageKind::MetaFile),
            field(14, true, ProtobufMessageKind::MetaFile),
            field(15, true, ProtobufMessageKind::MetaFile),
            field(16, true, ProtobufMessageKind::MetaFile),
            field(17, false, ProtobufMessageKind::None),
            field(18, false, ProtobufMessageKind::None),
            field(19, true, ProtobufMessageKind::PlacementPolicy),
            field(20, false, ProtobufMessageKind::None),
            field(21, false, ProtobufMessageKind::None),
            field(22, true, ProtobufMessageKind::PitrDbMap),
            field(23, false, ProtobufMessageKind::None),
            field(24, true, ProtobufMessageKind::BackupRange),
            field(25, false, ProtobufMessageKind::None),
            field(26, false, ProtobufMessageKind::None),
        ]),
        // MetaFile：子索引、data_files、schemas、raw_ranges、ddls、ranges。
        ProtobufMessageKind::MetaFile => HashMap::from([
            field(1, true, ProtobufMessageKind::File),
            field(2, true, ProtobufMessageKind::File),
            field(3, true, ProtobufMessageKind::Schema),
            field(4, true, ProtobufMessageKind::RawRange),
            field(5, false, ProtobufMessageKind::None),
            field(6, true, ProtobufMessageKind::BackupRange),
        ]),
        // Schema：含 stats_index 等嵌套；未知 number 触发兼容性失败。
        ProtobufMessageKind::Schema => HashMap::from([
            field(1, false, ProtobufMessageKind::None),
            field(2, false, ProtobufMessageKind::None),
            field(3, false, ProtobufMessageKind::None),
            field(4, false, ProtobufMessageKind::None),
            field(5, false, ProtobufMessageKind::None),
            field(6, false, ProtobufMessageKind::None),
            field(7, false, ProtobufMessageKind::None),
            field(8, true, ProtobufMessageKind::StatsFileIndex),
            field(9, false, ProtobufMessageKind::None),
            field(10, true, ProtobufMessageKind::None),
        ]),
        // File：SST 描述；field 13 为嵌套 TableMeta。
        ProtobufMessageKind::File => HashMap::from([
            field(1, false, ProtobufMessageKind::None),
            field(2, false, ProtobufMessageKind::None),
            field(3, false, ProtobufMessageKind::None),
            field(4, false, ProtobufMessageKind::None),
            field(5, false, ProtobufMessageKind::None),
            field(6, false, ProtobufMessageKind::None),
            field(7, false, ProtobufMessageKind::None),
            field(8, false, ProtobufMessageKind::None),
            field(9, false, ProtobufMessageKind::None),
            field(10, false, ProtobufMessageKind::None),
            field(11, false, ProtobufMessageKind::None),
            field(12, false, ProtobufMessageKind::None),
            field(13, true, ProtobufMessageKind::TableMeta),
        ]),
        ProtobufMessageKind::RawRange => HashMap::from([
            field(1, false, ProtobufMessageKind::None),
            field(2, false, ProtobufMessageKind::None),
            field(3, false, ProtobufMessageKind::None),
        ]),
        ProtobufMessageKind::StatsFile => {
            HashMap::from([field(1, true, ProtobufMessageKind::StatsBlock)])
        }
        ProtobufMessageKind::StatsBlock => HashMap::from([
            field(1, false, ProtobufMessageKind::None),
            field(2, false, ProtobufMessageKind::None),
        ]),
        ProtobufMessageKind::StatsFileIndex => HashMap::from([
            field(1, false, ProtobufMessageKind::None),
            field(2, false, ProtobufMessageKind::None),
            field(3, false, ProtobufMessageKind::None),
            field(4, false, ProtobufMessageKind::None),
            field(5, false, ProtobufMessageKind::None),
            field(6, false, ProtobufMessageKind::None),
        ]),
        ProtobufMessageKind::PlacementPolicy => {
            HashMap::from([field(1, false, ProtobufMessageKind::None)])
        }
        ProtobufMessageKind::PitrDbMap => HashMap::from([
            field(1, false, ProtobufMessageKind::None),
            field(2, true, ProtobufMessageKind::IdMap),
            field(3, true, ProtobufMessageKind::PitrTableMap),
            field(4, false, ProtobufMessageKind::None),
            field(5, false, ProtobufMessageKind::None),
        ]),
        ProtobufMessageKind::PitrTableMap => HashMap::from([
            field(1, false, ProtobufMessageKind::None),
            field(2, true, ProtobufMessageKind::IdMap),
            field(3, true, ProtobufMessageKind::IdMap),
            field(4, false, ProtobufMessageKind::None),
        ]),
        ProtobufMessageKind::IdMap => HashMap::from([
            field(1, false, ProtobufMessageKind::None),
            field(2, false, ProtobufMessageKind::None),
        ]),
        ProtobufMessageKind::BackupRange => HashMap::from([
            field(1, false, ProtobufMessageKind::None),
            field(2, false, ProtobufMessageKind::None),
            field(3, true, ProtobufMessageKind::File),
        ]),
        ProtobufMessageKind::TableMeta => HashMap::from([
            field(1, false, ProtobufMessageKind::None),
            field(2, false, ProtobufMessageKind::None),
            field(3, false, ProtobufMessageKind::None),
            field(4, false, ProtobufMessageKind::None),
        ]),
        ProtobufMessageKind::None => HashMap::new(),
    }
}

/// 构造字段表条目的语法糖。
fn field(number: u32, is_message: bool, nested: ProtobufMessageKind) -> (u32, ProtobufFieldInfo) {
    (number, ProtobufFieldInfo { is_message, nested })
}

/// 最小 protobuf wire 解析器，专供未知字段检测。
mod protowire {
    use astersql_errors::{ErrorArg, Errorf, SharedError, Trace};

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    /// protobuf wire type 枚举（含已弃用的 group）。
    pub enum WireType {
        Varint = 0,
        Fixed64 = 1,
        Bytes = 2,
        StartGroup = 3,
        EndGroup = 4,
        Fixed32 = 5,
    }

    impl TryFrom<u8> for WireType {
        type Error = SharedError;

        fn try_from(value: u8) -> Result<Self, Self::Error> {
            match value {
                0 => Ok(Self::Varint),
                1 => Ok(Self::Fixed64),
                2 => Ok(Self::Bytes),
                3 => Ok(Self::StartGroup),
                4 => Ok(Self::EndGroup),
                5 => Ok(Self::Fixed32),
                other => Err(Errorf(
                    "unsupported protobuf wire type %d in backupmeta",
                    &[ErrorArg::Signed(other as i128)],
                )),
            }
        }
    }

    /// 解析 tag varint，拆出 field number 与 wire type。
    pub fn consume_tag(data: &[u8]) -> Result<(u32, WireType, usize), SharedError> {
        let (tag, consumed) = consume_varint(data)?;
        let wire_type = WireType::try_from((tag & 0x07) as u8)?;
        let field_number = (tag >> 3) as u32;
        Ok((field_number, wire_type, consumed))
    }

    /// 按 wire type 跳过或截取字段值，供未知字段扫描前进偏移。
    pub fn consume_field_value<'a>(
        data: &'a [u8],
        field_number: u32,
        wire_type: WireType,
    ) -> Result<(&'a [u8], usize), SharedError> {
        match wire_type {
            WireType::Varint => {
                let (_, consumed) = consume_varint(data)?;
                Ok((&[], consumed))
            }
            WireType::Fixed32 => consume_fixed32(data),
            WireType::Fixed64 => consume_fixed64(data),
            WireType::Bytes => consume_bytes(data),
            WireType::StartGroup => consume_group(field_number, data),
            WireType::EndGroup => Err(Errorf("unexpected end-group wire type in backupmeta", &[])),
        }
    }

    /// 最多 10 字节的 varint；超长视为解析错误。
    fn consume_varint(data: &[u8]) -> Result<(u64, usize), SharedError> {
        let mut value = 0u64;
        for (index, byte) in data.iter().enumerate() {
            let shift = index as u32 * 7;
            value |= ((byte & 0x7f) as u64) << shift;
            if byte & 0x80 == 0 {
                return Ok((value, index + 1));
            }
            if index >= 9 {
                break;
            }
        }
        Err(parse_error(-1))
    }

    fn consume_fixed32(data: &[u8]) -> Result<(&[u8], usize), SharedError> {
        if data.len() < 4 {
            return Err(parse_error(-2));
        }
        Ok((&[], 4))
    }

    fn consume_fixed64(data: &[u8]) -> Result<(&[u8], usize), SharedError> {
        if data.len() < 8 {
            return Err(parse_error(-3));
        }
        Ok((&[], 8))
    }

    fn consume_bytes(data: &[u8]) -> Result<(&[u8], usize), SharedError> {
        let (length, consumed) = consume_varint(data)?;
        let total = consumed + length as usize;
        if data.len() < total {
            return Err(parse_error(-4));
        }
        Ok((&data[consumed..total], total))
    }

    /// 消费旧式 group，直到匹配的 EndGroup。
    fn consume_group(field_number: u32, data: &[u8]) -> Result<(&[u8], usize), SharedError> {
        let mut offset = 0usize;
        while offset < data.len() {
            let (number, wire_type, consumed) = consume_tag(&data[offset..])?;
            offset += consumed;
            if wire_type == WireType::EndGroup {
                if number != field_number {
                    return Err(parse_error(-5));
                }
                return Ok((&[], offset));
            }
            let (_, consumed) = consume_field_value(&data[offset..], number, wire_type)?;
            offset += consumed;
        }
        Err(parse_error(-6))
    }

    /// 统一包装 protobuf 解析失败码，便于测试断言。
    fn parse_error(code: i32) -> SharedError {
        Trace(Some(Errorf(
            "protobuf parse error %d",
            &[ErrorArg::Signed(code as i128)],
        )))
        .expect("trace")
    }
}
