// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

//! 日志备份数据文件上的键搜索。
//! 对应 Go `br/pkg/stream/search.go`：扫描 `v1/backupmeta/*.meta`，
//! 按键前缀与可选 TS 窗口过滤 DataFile，校验 Sha256 后迭代 KV。
//! WriteCF/DefaultCF 结果经 `mergeCFEntries` 按 startTs 配对合并，
//! 最终按 CommitTs 排序输出 `StreamKVInfo`。
//! 当前 Rust 路径为串行读文件；Go 侧可并发，语义应对齐。
//! 比较器可插拔，默认前缀匹配满足运维按表前缀排查。

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use astersql_br_pkg_utils_consts::{DefaultCF, WriteCF};
use base64::Engine;
use sha2::{Digest, Sha256};

use crate::decode_kv::{Iterator, NewEventIterator};
use crate::meta_kv::RawWriteCFValue;
use crate::stubs::backuppb::{DataFileInfo, Metadata};
use crate::stubs::errors::Error;
use crate::stubs::{Storage, codec};

/// 键匹配策略；`src` 为候选键，`dst` 为搜索目标。
pub trait Comparator {
    fn Compare(&self, src: &[u8], dst: &[u8]) -> bool;
}

/// 默认实现：`src` 是否以 `dst` 为前缀。
struct startWithComparator;

/// 构造前缀比较器，与 Go `NewStartWithComparator` 一致。
pub fn NewStartWithComparator() -> Box<dyn Comparator> {
    Box::new(startWithComparator)
}

impl Comparator for startWithComparator {
    /// 字节前缀匹配。
    fn Compare(&self, src: &[u8], dst: &[u8]) -> bool {
        src.starts_with(dst)
    }
}

/// 单条搜索命中的展示结构（JSON 友好字段多为 hex/base64 字符串）。
#[derive(Clone, Debug, Default)]
pub struct StreamKVInfo {
    /// 解码后的业务键（大写 hex）。
    pub Key: String,
    /// 含 Ts 后缀的完整编码键（hex）。
    pub EncodedKey: String,
    /// WriteCF 写入类型字节；DefaultCF 为 0。
    pub WriteType: u8,
    pub StartTs: u64,
    pub CommitTs: u64,
    pub CFName: String,
    /// DefaultCF 值或从 Default 合并来的 base64。
    pub Value: String,
    /// WriteCF shortValue 的 base64；无则空。
    pub ShortValue: String,
}

/// 在外部存储上按键搜索日志备份。
/// `searchKey` 在构造时已 `EncodeBytes`，与文件内编码键对齐。
pub struct StreamBackupSearch {
    storage: Arc<dyn Storage>,
    comparator: Box<dyn Comparator>,
    searchKey: Vec<u8>,
    /// 0 表示不限制下界。
    startTs: u64,
    /// 0 表示不限制上界。
    endTs: u64,
}

/// 创建搜索器；内部先对 raw searchKey 做 EncodeBytes。
pub fn NewStreamBackupSearch(
    storage: Arc<dyn Storage>,
    comparator: Box<dyn Comparator>,
    searchKey: Vec<u8>,
) -> StreamBackupSearch {
    let encoded_key = codec::EncodeBytes(Vec::new(), &searchKey);
    StreamBackupSearch {
        storage,
        comparator,
        searchKey: encoded_key,
        startTs: 0,
        endTs: 0,
    }
}

impl StreamBackupSearch {
    /// 设置提交/版本时间下界（含）。
    pub fn SetStartTS(&mut self, startTs: u64) {
        self.startTs = startTs;
    }

    /// 设置时间上界（含）。
    pub fn SetEndTs(&mut self, endTs: u64) {
        self.endTs = endTs;
    }

    /// 从一份 Metadata 筛出可能覆盖 searchKey 的数据文件。
    /// 跳过 IsMeta；键区间与可选 TS 窗口任一不重叠则丢弃。
    fn resolveMetaData(&self, metaData: &Metadata, out: &mut Vec<DataFileInfo>) {
        for file in &metaData.Files {
            if file.IsMeta {
                continue;
            }
            // searchKey < StartKey → 文件区间整体偏后。
            if self.searchKey.as_slice().cmp(&file.StartKey) == std::cmp::Ordering::Less {
                continue;
            }
            // searchKey > EndKey → 文件区间整体偏前。
            if self.searchKey.as_slice().cmp(&file.EndKey) == std::cmp::Ordering::Greater {
                continue;
            }
            // 文件最大 Ts 仍小于下界 → 整文件过旧。
            if self.startTs > 0 && file.MaxTs < self.startTs {
                continue;
            }
            // 文件最小 Ts 已大于上界 → 整文件过新。
            if self.endTs > 0 && file.MinTs > self.endTs {
                continue;
            }
            out.push(file.clone());
        }
    }

    /// 列出 backupmeta、解析命中文件并搜索，汇总结果。
    /// 多文件结果简单拼接，不在此去重。
    pub fn Search(&self) -> Result<Vec<StreamKVInfo>, Error> {
        let mut data_files = Vec::new();
        for (path, _) in self
            .storage
            .ListFiles("v1/backupmeta")
            .map_err(Error::new)?
        {
            // 只处理 `.meta` 后缀的元数据文件。
            if !path.ends_with(".meta") {
                continue;
            }
            let b = self.storage.ReadFile(&path).map_err(Error::new)?;
            let m: Metadata = serde_json::from_slice(&b).map_err(|e| Error::new(format!("{e}")))?;
            self.resolveMetaData(&m, &mut data_files);
        }

        let mut raw_entries = Vec::new();
        for data_file in data_files {
            self.searchFromDataFile(&data_file, &mut raw_entries)?;
        }

        // Go Search 在所有文件读取完成后才按 CF 建表并合并。DefaultCF 与
        // WriteCF 通常位于不同数据文件，不能在单文件边界内提前合并。
        let mut default_cf_entries = HashMap::new();
        let mut write_cf_entries = HashMap::new();
        for entry in raw_entries {
            if entry.CFName == WriteCF {
                write_cf_entries.insert(entry.EncodedKey.clone(), entry);
            } else if entry.CFName == DefaultCF {
                default_cf_entries.insert(entry.EncodedKey.clone(), entry);
            }
        }
        Ok(self.mergeCFEntries(default_cf_entries, write_cf_entries))
    }

    /// 读取单文件：校验校验和，迭代 KV，并按 CF 产出待全局合并的条目。
    pub(crate) fn searchFromDataFile(
        &self,
        dataFile: &DataFileInfo,
        out: &mut Vec<StreamKVInfo>,
    ) -> Result<(), Error> {
        let buff = self.storage.ReadFile(&dataFile.Path).map_err(|e| {
            Error::new(format!(
                "read data file error, file: {}: {e}",
                dataFile.Path
            ))
        })?;

        // 与 meta 中记录的 Sha256 不一致则拒绝，防止静默读坏文件。
        let checksum = Sha256::digest(&buff);
        if checksum.as_slice() != dataFile.GetSha256() {
            return Err(Error::new(format!(
                "validate checksum failed, file: {}",
                dataFile.Path
            )));
        }

        let mut iter = NewEventIterator(buff);
        let mut default_cf_entries: HashMap<String, StreamKVInfo> = HashMap::new();
        let mut write_cf_entries: HashMap<String, StreamKVInfo> = HashMap::new();

        while iter.Valid() {
            iter.Next();
            if let Some(err) = iter.GetError() {
                return Err(Error::new(err.to_string()));
            }

            let mut k = iter.Key().to_vec();
            let v = iter.Value().to_vec();
            // 未命中比较器则跳过（如前缀不符）。
            if !self.comparator.Compare(&k, &self.searchKey) {
                continue;
            }

            // 末 8 字节为降序 uint Ts；剥除后再 DecodeBytes 得业务键。
            let (_, ts) = codec::DecodeUintDesc(&k[k.len() - 8..]).map_err(|e| {
                Error::new(format!(
                    "decode ts from key error, file: {}: {e}",
                    dataFile.Path
                ))
            })?;
            k.truncate(k.len() - 8);

            let (_, raw_key) = codec::DecodeBytes(&k, None).map_err(|e| {
                Error::new(format!(
                    "decode raw key error, file: {}: {e}",
                    dataFile.Path
                ))
            })?;

            if dataFile.Cf == WriteCF {
                let mut raw_write_cf_value = RawWriteCFValue::default();
                raw_write_cf_value.ParseFrom(&v).map_err(|e| {
                    Error::new(format!(
                        "parse raw write cf value error, file: {}: {e}",
                        dataFile.Path
                    ))
                })?;

                // shortValue 用 base64 暴露；无 shortValue 时留给 DefaultCF 合并。
                let value_str = if raw_write_cf_value.HasShortValue() {
                    base64::engine::general_purpose::STANDARD
                        .encode(raw_write_cf_value.GetShortValue())
                } else {
                    String::new()
                };

                // Map 键用完整编码键 hex，避免同业务键多版本互相覆盖时丢失。
                write_cf_entries.insert(
                    hex::encode(iter.Key()),
                    StreamKVInfo {
                        WriteType: raw_write_cf_value.GetWriteType(),
                        CFName: dataFile.Cf.clone(),
                        CommitTs: ts,
                        StartTs: raw_write_cf_value.GetStartTs(),
                        Key: hex::encode(&raw_key).to_uppercase(),
                        EncodedKey: hex::encode(iter.Key()),
                        ShortValue: value_str,
                        Value: String::new(),
                    },
                );
            } else if dataFile.Cf == DefaultCF {
                // DefaultCF：Ts 记入 StartTs，值 base64。
                default_cf_entries.insert(
                    hex::encode(iter.Key()),
                    StreamKVInfo {
                        CFName: dataFile.Cf.clone(),
                        StartTs: ts,
                        Key: hex::encode(&raw_key).to_uppercase(),
                        EncodedKey: hex::encode(iter.Key()),
                        Value: base64::engine::general_purpose::STANDARD.encode(v),
                        WriteType: 0,
                        CommitTs: 0,
                        ShortValue: String::new(),
                    },
                );
            }
        }

        out.extend(write_cf_entries.into_values());
        out.extend(default_cf_entries.into_values());
        Ok(())
    }

    /// 合并两 CF：无 shortValue 的 Write 按 (rawKey, startTs) 回查 Default。
    /// 已合并的 Default 不再单独输出；最终按 CommitTs 升序。
    pub fn mergeCFEntries(
        &self,
        defaultCFEntries: HashMap<String, StreamKVInfo>,
        writeCFEntries: HashMap<String, StreamKVInfo>,
    ) -> Vec<StreamKVInfo> {
        let mut entries: Vec<StreamKVInfo> =
            Vec::with_capacity(defaultCFEntries.len() + writeCFEntries.len());
        let mut merged_default_cf_keys = HashSet::with_capacity(16);

        for mut entry in writeCFEntries.into_values() {
            if entry.ShortValue.is_empty() {
                // 重建 DefaultCF 键：EncodeBytes(raw) || EncodeUintDesc(startTs)。
                if let Ok(key_bytes) = hex::decode(&entry.Key) {
                    let encoded_key = codec::EncodeBytes(Vec::new(), &key_bytes);
                    let default_cf_key =
                        hex::encode(codec::EncodeUintDesc(encoded_key, entry.StartTs));
                    if let Some(default_cf_entry) = defaultCFEntries.get(&default_cf_key) {
                        entry.Value = default_cf_entry.Value.clone();
                        merged_default_cf_keys.insert(default_cf_key);
                    }
                }
            }
            entries.push(entry);
        }

        // 未被 Write 引用的 Default 条目仍输出（如独立 default 写入）。
        for (key, entry) in defaultCFEntries {
            if merged_default_cf_keys.contains(&key) {
                continue;
            }
            entries.push(entry);
        }

        // DefaultCF 单独条目 CommitTs=0，排序时靠前。
        entries.sort_by(|i, j| i.CommitTs.cmp(&j.CommitTs));
        entries
    }
}

/// 将原始搜索键编码为与文件内键相同的 memcomparable 形式。
pub fn EncodeSearchKey(raw: &[u8]) -> Vec<u8> {
    codec::EncodeBytes(Vec::new(), raw)
}
