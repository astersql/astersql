// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

//! 恢复键重写规则：生成、校验、改写 Range/文件键，对齐 Go `rewrite_rule.go`。
//! RewriteRules 聚合 SST import 规则、keyspace、TS 过滤与表 ID 重映射提示。
//! getDetailRule 控制粗粒度表前缀 vs 细粒度 record/index 前缀；
//! Validate/GetRewrite*Keys 要求起止键落在同一表且能匹配同一 NewKeyPrefix。
//! Rewrite rules for restore key rewriting matching `rewrite_rule.go`.
//! 本文件是 restore utils 的核心：merge/import/log_client 均依赖此处规则语义。
//! 注释说明约束与 Go 对齐点，不改变任何可执行行为。

use std::collections::HashMap;
// Display 实现依赖 redact，避免日志打印明文键。
use std::fmt;

// InvalidRewrite：规则缺失/不一致；TableIDMismatch：区间跨表。
use astersql_br_pkg_errors::{ErrRestoreInvalidRewrite, ErrRestoreTableIDMismatch};
// Range 来自 rtree，与 merge 共用区间表示。
use astersql_br_pkg_rtree::Range;
use astersql_errors::{Annotate, Annotatef, Errorf, SharedError};

// ID 映射与 CF 名复用 misc，避免规则生成与合并逻辑分叉。
use crate::misc::{DefaultCFName, GetIndexIDMap, GetTableIDMap, WriteCFName};
use crate::stubs::{
    self, AppliedFile, backuppb, codec, import_sstpb, log, logutil, model, redact, tablecodec, util,
};

/// 将静态 BR 错误包装为 SharedError，供 Annotate/Annotatef 使用。
fn br_err(err: &'static astersql_errors::Error) -> SharedError {
    SharedError::new(err.clone())
}

/// 表级键重写规则集合：Data 为 import_sstpb 规则列表，附带 keyspace/TS/重映射提示。
/// RewriteRules contains rules for rewriting keys of tables.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RewriteRules {
    /// 旧前缀→新前缀的 SST 导入规则（可含时间戳重写字段）。
    pub Data: Vec<import_sstpb::RewriteRule>,
    /// 备份侧 keyspace；展示/跨 keyspace 恢复时使用。
    pub OldKeyspace: Vec<u8>,
    /// 恢复目标 keyspace。
    pub NewKeyspace: Vec<u8>,
    /// 主新表 ID（单表规则路径会填充）。
    pub NewTableID: i64,
    /// default CF 过滤用的前移 start TS（PiTR 场景）。
    pub ShiftStartTs: u64,
    /// 逻辑备份/日志起点 TS；write CF 的 IgnoreBefore。
    pub StartTs: u64,
    /// 恢复点 TS；作为 IgnoreAfter。
    pub RestoredTs: u64,
    /// 旧→新物理表 ID 提示列表（含分区）。
    pub TableIDRemapHint: Vec<TableIDRemap>,
}

impl RewriteRules {
    /// StartTs 与 RestoredTs 均非 0 时视为已配置时间过滤窗口。
    pub fn HasSetTs(&self) -> bool {
        self.StartTs != 0 && self.RestoredTs != 0
    }

    /// 一次性写入 ShiftStartTs/StartTs/RestoredTs，供 SetTimeRangeFilter 读取。
    pub fn SetTsRange(&mut self, shiftStartTs: u64, startTs: u64, restoredTs: u64) {
        self.ShiftStartTs = shiftStartTs;
        self.StartTs = startTs;
        self.RestoredTs = restoredTs;
    }

    /// 就地改写 Data 中 OldKeyPrefix 的表前缀 from→to；有改动返回 true。
    /// 用于上游二次重映射源表 ID，而不重建整套规则。
    pub fn RewriteSourceTableID(&mut self, from: i64, to: i64) -> bool {
        // 表前缀字节比较：只改 OldKeyPrefix，NewKeyPrefix 保持目标侧不变。
        let toPrefix = tablecodec::EncodeTablePrefix(to);
        let fromPrefix = tablecodec::EncodeTablePrefix(from);
        let mut rewritten = false;
        for rule in &mut self.Data {
            if rule.OldKeyPrefix.starts_with(&fromPrefix) {
                // 保留前缀后的索引/record 后缀字节。
                let suffix = rule.OldKeyPrefix[fromPrefix.len()..].to_vec();
                rule.OldKeyPrefix = [toPrefix.clone(), suffix].concat();
                rewritten = true;
            }
        }
        // 任一规则被改写即返回 true，供调用方决定是否继续。
        rewritten
    }

    /// 深拷贝 Go `Clone` 明确选择的字段；时间范围字段保持零值。
    pub fn Clone(&self) -> RewriteRules {
        // ProtoV1Clone 保证 protobuf 消息深拷贝语义与 Go proto.Clone 对齐。
        let data = self.Data.iter().map(util::ProtoV1Clone).collect();
        RewriteRules {
            Data: data,
            TableIDRemapHint: self.TableIDRemapHint.clone(),
            OldKeyspace: self.OldKeyspace.clone(),
            NewKeyspace: self.NewKeyspace.clone(),
            NewTableID: self.NewTableID,
            // Go Clone intentionally omits the time-range fields.
            ShiftStartTs: 0,
            StartTs: 0,
            RestoredTs: 0,
        }
    }

    /// 字段级相等：keyspace、表 ID、TS、RemapHint 与每条规则的关键前缀/时间戳。
    pub fn Equal(&self, rhs: &RewriteRules) -> bool {
        if self.NewKeyspace != rhs.NewKeyspace
            || self.OldKeyspace != rhs.OldKeyspace
            || self.NewTableID != rhs.NewTableID
            // TS 三元组任一不同即不相等。
            || self.ShiftStartTs != rhs.ShiftStartTs
            || self.StartTs != rhs.StartTs
            || self.RestoredTs != rhs.RestoredTs
        {
            return false;
        }
        // RemapHint 按位置一一比较 Origin/Rewritten。
        if self.TableIDRemapHint.len() != rhs.TableIDRemapHint.len() {
            return false;
        }
        for (i, remap) in self.TableIDRemapHint.iter().enumerate() {
            if remap.Origin != rhs.TableIDRemapHint[i].Origin
                || remap.Rewritten != rhs.TableIDRemapHint[i].Rewritten
            {
                return false;
            }
        }
        // Data 比较前缀与时间戳过滤字段，忽略其它 protobuf 默认字段差异。
        if self.Data.len() != rhs.Data.len() {
            return false;
        }
        for (i, rule) in self.Data.iter().enumerate() {
            let rhsRule = &rhs.Data[i];
            // 规则相等看前缀与 Ignore/NewTimestamp，不比其它元数据。
            if rule.NewKeyPrefix != rhsRule.NewKeyPrefix
                || rule.OldKeyPrefix != rhsRule.OldKeyPrefix
                || rule.NewTimestamp != rhsRule.NewTimestamp
                // Ignore* 参与 Equal，保证 Clone/SetTs 后可回归。
                || rule.IgnoreAfterTimestamp != rhsRule.IgnoreAfterTimestamp
                || rule.IgnoreBeforeTimestamp != rhsRule.IgnoreBeforeTimestamp
            {
                return false;
            }
        }
        true
    }

    /// 仅追加 other.Data；不合并 TS/keyspace（与 Go Append 一致）。
    pub fn Append(&mut self, other: RewriteRules) {
        self.Data.extend(other.Data);
    }
}

/// 重写过程中的表 ID 映射提示：Origin 为备份侧，Rewritten 为集群侧。
/// TableIDRemap presents a remapping of table id during rewriting.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TableIDRemap {
    pub Origin: i64,
    pub Rewritten: i64,
}

/// 按 CF 类型把表级 TS 窗口写入单条文件规则的 IgnoreBefore/After。
/// 未设置 TS 时直接 Ok；未知 CF 返回错误。
pub fn SetTimeRangeFilter(
    tableRules: &RewriteRules,
    fileRule: &mut import_sstpb::RewriteRule,
    cfName: &str,
) -> Result<(), SharedError> {
    // cfName 用 contains 以兼容文件名中嵌入 CF 名的历史格式。
    // 无 TS 窗口：跳过过滤字段填充，保持文件规则原样。
    if !tableRules.HasSetTs() {
        return Ok(());
    }

    // default：取 Shift 与 Start 较小者，覆盖 default CF 更早可见版本；
    // write：用 StartTs；其它 CF 拒绝。
    let ignoreBeforeTs = if cfName.contains(DefaultCFName) {
        tableRules.ShiftStartTs.min(tableRules.StartTs)
    } else if cfName.contains(WriteCFName) {
        tableRules.StartTs
    } else {
        let msg = format!("unsupported column family type: {cfName}");
        return Err(Errorf(&msg, &[]));
    };

    fileRule.IgnoreBeforeTimestamp = ignoreBeforeTs;
    fileRule.IgnoreAfterTimestamp = tableRules.RestoredTs;
    // IgnoreAfter 统一为 RestoredTs，闭合恢复窗口上界。
    Ok(())
}

/// 空的「旧表 ID → 规则」映射，供调用方惰性填充。
pub fn EmptyRewriteRulesMap() -> HashMap<i64, RewriteRules> {
    HashMap::new()
}

/// 空规则：Data 为空，其余字段 Default。
pub fn EmptyRewriteRule() -> RewriteRules {
    RewriteRules {
        // 显式清空 Data，其它字段走 Default（含 0 TS）。
        Data: Vec::new(),
        ..Default::default()
    }
}

/// 由新旧表元数据生成聚合 RewriteRules（含全部分区物理 ID）。
/// getDetailRule=true 时为 record+各索引前缀；false 时仅表级 EncodeTablePrefix。
pub fn GetRewriteRules(
    newTable: &model::TableInfo,
    oldTable: &model::TableInfo,
    newTimeStamp: u64,
    getDetailRule: bool,
) -> RewriteRules {
    let tableIDs = GetTableIDMap(newTable, oldTable);
    let indexIDs = GetIndexIDMap(newTable, oldTable);
    let mut remaps: Vec<TableIDRemap> = Vec::new();
    let mut dataRules: Vec<import_sstpb::RewriteRule> = Vec::new();

    for (oldTableID, newTableID) in tableIDs {
        remaps.push(TableIDRemap {
            Origin: oldTableID,
            Rewritten: newTableID,
        });
        // 细粒度：行前缀 + 每个索引一对 Old/New；时间戳统一 newTimeStamp。
        if getDetailRule {
            // record 前缀规则：覆盖行数据键空间。
            dataRules.push(import_sstpb::RewriteRule {
                OldKeyPrefix: tablecodec::GenTableRecordPrefix(oldTableID),
                NewKeyPrefix: tablecodec::GenTableRecordPrefix(newTableID),
                NewTimestamp: newTimeStamp,
                ..Default::default()
            });
            for (oldIndexID, newIndexID) in &indexIDs {
                // 索引前缀规则：oldIndex→newIndex，绑定到对应物理表 ID。
                dataRules.push(import_sstpb::RewriteRule {
                    OldKeyPrefix: tablecodec::EncodeTableIndexPrefix(oldTableID, *oldIndexID),
                    NewKeyPrefix: tablecodec::EncodeTableIndexPrefix(newTableID, *newIndexID),
                    NewTimestamp: newTimeStamp,
                    ..Default::default()
                });
            }
        } else {
            // 粗粒度：整表前缀一条规则，覆盖 record/index 子空间。
            // 性能更好，但无法单独重映射某一索引 ID。
            dataRules.push(import_sstpb::RewriteRule {
                OldKeyPrefix: tablecodec::EncodeTablePrefix(oldTableID),
                NewKeyPrefix: tablecodec::EncodeTablePrefix(newTableID),
                NewTimestamp: newTimeStamp,
                ..Default::default()
            });
        }
    }

    RewriteRules {
        Data: dataRules,
        TableIDRemapHint: remaps,
        ..Default::default()
    }
}

/// 按旧物理表 ID 分桶的规则映射；每桶含完整 RemapHint 克隆。
pub fn GetRewriteRulesMap(
    newTable: &model::TableInfo,
    oldTable: &model::TableInfo,
    newTimeStamp: u64,
    getDetailRule: bool,
) -> HashMap<i64, RewriteRules> {
    let mut rules: HashMap<i64, RewriteRules> = HashMap::new();
    let tableIDs = GetTableIDMap(newTable, oldTable);
    let indexIDs = GetIndexIDMap(newTable, oldTable);
    // remaps 在循环中累积，每个桶都 clone 全量 hint（对齐 Go）。
    let mut remaps: Vec<TableIDRemap> = Vec::new();

    for (oldTableID, newTableID) in tableIDs {
        remaps.push(TableIDRemap {
            Origin: oldTableID,
            Rewritten: newTableID,
        });
        // 每旧表独立 Data；细/粗粒度分支与 GetRewriteRules 相同。
        let mut dataRules: Vec<import_sstpb::RewriteRule> = Vec::new();
        if getDetailRule {
            dataRules.push(import_sstpb::RewriteRule {
                OldKeyPrefix: tablecodec::GenTableRecordPrefix(oldTableID),
                NewKeyPrefix: tablecodec::GenTableRecordPrefix(newTableID),
                NewTimestamp: newTimeStamp,
                ..Default::default()
            });
            // 索引 ID 映射跨所有物理表复用同一 indexIDs。
            for (oldIndexID, newIndexID) in &indexIDs {
                dataRules.push(import_sstpb::RewriteRule {
                    OldKeyPrefix: tablecodec::EncodeTableIndexPrefix(oldTableID, *oldIndexID),
                    NewKeyPrefix: tablecodec::EncodeTableIndexPrefix(newTableID, *newIndexID),
                    NewTimestamp: newTimeStamp,
                    ..Default::default()
                });
            }
        } else {
            dataRules.push(import_sstpb::RewriteRule {
                OldKeyPrefix: tablecodec::EncodeTablePrefix(oldTableID),
                NewKeyPrefix: tablecodec::EncodeTablePrefix(newTableID),
                NewTimestamp: newTimeStamp,
                ..Default::default()
            });
        }

        // key=旧物理表 ID，value 含截至当前的全量 RemapHint。
        rules.insert(
            oldTableID,
            RewriteRules {
                Data: dataRules,
                TableIDRemapHint: remaps.clone(),
                ..Default::default()
            },
        );
    }

    rules
}

/// 单表（无分区展开）规则：直接指定 old/new 表 ID 与索引映射。
/// 设置 NewTableID，供上游快速读取目标表。
pub fn GetRewriteRuleOfTable(
    oldTableID: i64,
    newTableID: i64,
    indexIDs: HashMap<i64, i64>,
    getDetailRule: bool,
) -> RewriteRules {
    let mut dataRules: Vec<import_sstpb::RewriteRule> = Vec::new();
    // 单表场景 RemapHint 仅一条。
    let remaps = vec![TableIDRemap {
        Origin: oldTableID,
        Rewritten: newTableID,
    }];
    if getDetailRule {
        // 注意：此处未填 NewTimestamp（与 Go GetRewriteRuleOfTable 一致）。
        dataRules.push(import_sstpb::RewriteRule {
            OldKeyPrefix: tablecodec::GenTableRecordPrefix(oldTableID),
            NewKeyPrefix: tablecodec::GenTableRecordPrefix(newTableID),
            ..Default::default()
        });
        for (oldIndexID, newIndexID) in indexIDs {
            dataRules.push(import_sstpb::RewriteRule {
                OldKeyPrefix: tablecodec::EncodeTableIndexPrefix(oldTableID, oldIndexID),
                NewKeyPrefix: tablecodec::EncodeTableIndexPrefix(newTableID, newIndexID),
                ..Default::default()
            });
        }
    } else {
        dataRules.push(import_sstpb::RewriteRule {
            OldKeyPrefix: tablecodec::EncodeTablePrefix(oldTableID),
            NewKeyPrefix: tablecodec::EncodeTablePrefix(newTableID),
            ..Default::default()
        });
    }

    RewriteRules {
        Data: dataRules,
        NewTableID: newTableID,
        TableIDRemapHint: remaps,
        // NewTableID 仅在此路径填充，聚合 GetRewriteRules 保持 0。
        ..Default::default()
    }
}

/// 校验文件起止键都能匹配规则，且两边 NewKeyPrefix 一致。
/// 用于导入前发现脏备份或不兼容 BR 版本。
pub fn ValidateFileRewriteRule(
    file: &backuppb::File,
    rewriteRules: Option<&RewriteRules>,
) -> Result<(), SharedError> {
    // 先按 raw 键匹配；有规则但未命中则直接失败。
    let (_start_key, startRule) = rewriteRawKey(&file.GetStartKey(), rewriteRules);
    if rewriteRules.is_some() && startRule.is_none() {
        // tableID 仅用于潜在日志扩展；当前错误文案保持与 Go 一致。
        let _tableID = tablecodec::DecodeTableID(&file.GetStartKey());
        // 起始键无规则：备份可能缺表或规则未覆盖。
        log::Error("cannot find rewrite rule for file start key");
        return Err(Annotate(
            Some(br_err(&ErrRestoreInvalidRewrite)),
            "cannot find rewrite rule",
        )
        .expect("annotate"));
    }

    let (_end_key, endRule) = rewriteRawKey(&file.GetEndKey(), rewriteRules);
    if rewriteRules.is_some() && endRule.is_none() {
        let _tableID = tablecodec::DecodeTableID(&file.GetEndKey());
        // 结束键解码表 ID，便于对照 start 侧诊断。
        // 结束键同样必须可匹配，避免半区间改写。
        log::Error("cannot find rewrite rule for file end key");
        return Err(Annotate(
            Some(br_err(&ErrRestoreInvalidRewrite)),
            "cannot find rewrite rule",
        )
        .expect("annotate"));
    }

    // 起止规则的 NewKeyPrefix 必须相同，否则区间会跨表撕裂。
    let start_prefix = startRule
        .as_ref()
        .map(|r| r.GetNewKeyPrefix())
        .unwrap_or_default();
    let end_prefix = endRule
        .as_ref()
        .map(|r| r.GetNewKeyPrefix())
        .unwrap_or_default();
    if start_prefix != end_prefix {
        // 明确提示可能是脏数据或 BR 版本不兼容。
        log::Error("unexpected rewrite rules");
        let msg = format!(
            "rewrite rule mismatch, the backup data may be dirty or from incompatible versions of BR, startKey rule: {:X?} => {:X?}, endKey rule: {:X?} => {:X?}",
            startRule
                .as_ref()
                .map(|r| r.OldKeyPrefix.clone())
                .unwrap_or_default(),
            start_prefix,
            endRule
                .as_ref()
                .map(|r| r.OldKeyPrefix.clone())
                .unwrap_or_default(),
            end_prefix,
        );
        return Err(
            Annotatef(Some(br_err(&ErrRestoreInvalidRewrite)), &msg, &[]).expect("annotate"),
        );
    }
    // 起止 NewKeyPrefix 一致才视为文件可安全导入。
    Ok(())
}

/// 已 EncodeBytes 的键：先解码再走 raw 重写；无规则时原样返回编码键。
fn rewriteEncodedKey(
    key: &[u8],
    rewriteRules: Option<&RewriteRules>,
) -> (Option<Vec<u8>>, Option<import_sstpb::RewriteRule>) {
    if rewriteRules.is_none() {
        return (Some(key.to_vec()), None);
    }
    if !key.is_empty() {
        // 解码失败视为无法重写（返回 None,None）。
        if let Ok((_rem, rawKey)) = codec::DecodeBytes(key, None) {
            return rewriteRawKey(&rawKey, rewriteRules);
        }
        return (None, None);
    }
    (None, None)
}

/// 原始（未 memcomparable 编码）键：匹配前缀后 RewriteAndEncodeRawKey。
/// 无规则时仍 EncodeBytes，保持与下游 SST 接口一致。
fn rewriteRawKey(
    key: &[u8],
    rewriteRules: Option<&RewriteRules>,
) -> (Option<Vec<u8>>, Option<import_sstpb::RewriteRule>) {
    if rewriteRules.is_none() {
        return (Some(codec::EncodeBytes(Vec::new(), key)), None);
    }
    if !key.is_empty() {
        let rule = matchOldPrefix(key, rewriteRules.unwrap());
        return (RewriteAndEncodeRawKey(key, rule.as_ref()), rule);
    }
    (None, None)
}

/// 用规则替换 OldKeyPrefix→NewKeyPrefix，再 EncodeBytes；无规则返回 None。
pub fn RewriteAndEncodeRawKey(
    key: &[u8],
    rule: Option<&import_sstpb::RewriteRule>,
) -> Option<Vec<u8>> {
    let Some(rule) = rule else {
        // Nil protobuf getters return empty prefixes in Go; replacing empty
        // with empty leaves the key unchanged before EncodeBytes.
        return Some(codec::EncodeBytes(Vec::new(), key));
    };
    // Go uses bytes.Replace(key, old, new, 1): replace the first occurrence,
    // including inserting at offset zero when the old prefix is empty.
    let match_offset = if rule.OldKeyPrefix.is_empty() {
        Some(0)
    } else {
        key.windows(rule.OldKeyPrefix.len())
            .position(|window| window == rule.OldKeyPrefix)
    };
    let ret = if let Some(offset) = match_offset {
        [
            key[..offset].to_vec(),
            rule.NewKeyPrefix.clone(),
            key[offset + rule.OldKeyPrefix.len()..].to_vec(),
        ]
        .concat()
    } else {
        key.to_vec()
    };
    Some(codec::EncodeBytes(Vec::new(), &ret))
}

/// 线性扫描 Data，返回第一条 OldKeyPrefix 前缀匹配的规则。
fn matchOldPrefix(key: &[u8], rewriteRules: &RewriteRules) -> Option<import_sstpb::RewriteRule> {
    for rule in &rewriteRules.Data {
        if key.starts_with(&rule.OldKeyPrefix) {
            return Some(rule.clone());
        }
    }
    // 无匹配时返回 None，由上层决定报错或跳过。
    None
}

/// 由旧 tableID 构造 record 前缀并匹配规则，解码 NewKeyPrefix 得新表 ID；未命中返回 0。
pub fn GetRewriteTableID(tableID: i64, rewriteRules: &RewriteRules) -> i64 {
    let tableKey = tablecodec::GenTableRecordPrefix(tableID);
    let rule = matchOldPrefix(&tableKey, rewriteRules);
    rule.map(|r| tablecodec::DecodeTableID(&r.NewKeyPrefix))
        .unwrap_or(0)
}

/// 为文件查找匹配规则：要求起止同表；先试 raw 再试 encoded。
pub fn FindMatchedRewriteRule(
    file: &dyn AppliedFile,
    rules: &RewriteRules,
) -> Option<import_sstpb::RewriteRule> {
    let startID = tablecodec::DecodeTableID(&file.GetStartKey());
    let endID = tablecodec::DecodeTableID(&file.GetEndKey());
    // 跨表文件无法用单一规则描述。
    if startID != endID {
        return None;
    }
    let (_key, mut rule) = rewriteRawKey(&file.GetStartKey(), Some(rules));
    if rule.is_none() {
        // SST raw 未命中时，尝试日志类已编码键。
        let (_key, encoded_rule) = rewriteEncodedKey(&file.GetStartKey(), Some(rules));
        rule = encoded_rule;
    }
    // 可能仍为 None：调用方需自行处理未覆盖文件。
    rule
}

/// 调试展示：redact 后的 keyspace 与每条 Old=>New 前缀，避免日志泄露明文键。
impl fmt::Display for RewriteRules {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut out = String::new();
        out.push('[');
        // 非空 keyspace 时先打印 ks 映射。
        if !self.OldKeyspace.is_empty() {
            // keyspace 映射用 =[ks]=> 标记，区别于普通前缀 =>。
            out.push_str(&redact::Key(&self.OldKeyspace));
            out.push_str(" =[ks]=> ");
            out.push_str(&redact::Key(&self.NewKeyspace));
        }
        for (i, d) in self.Data.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            // 多条规则逗号分隔，前缀均经 redact。
            out.push_str(&redact::Key(&d.OldKeyPrefix));
            out.push_str(" => ");
            out.push_str(&redact::Key(&d.NewKeyPrefix));
        }
        out.push(']');
        // 方括号包裹完整规则摘要。
        write!(f, "{out}")
    }
}

/// 重写文件起止 raw 键；有规则时两端都必须命中，否则 InvalidRewrite。
pub fn GetRewriteRawKeys(
    file: &dyn AppliedFile,
    rewriteRules: Option<&RewriteRules>,
) -> Result<(Option<Vec<u8>>, Option<Vec<u8>>), SharedError> {
    let startID = tablecodec::DecodeTableID(&file.GetStartKey());
    let endID = tablecodec::DecodeTableID(&file.GetEndKey());
    if startID == endID {
        let (startKey, rule) = rewriteRawKey(&file.GetStartKey(), rewriteRules);
        // 提供了规则却未匹配：视为备份/规则不一致。
        if rewriteRules.is_some() && rule.is_none() {
            // 错误信息带上 self Display，方便对照规则列表。
            let msg = format!(
                "cannot find raw rewrite rule for start key, startKey: {}; self = {}",
                redact::Key(&file.GetStartKey()),
                rewriteRules.unwrap()
            );
            return Err(
                Annotatef(Some(br_err(&ErrRestoreInvalidRewrite)), &msg, &[]).expect("annotate"),
            );
        }
        let (endKey, rule) = rewriteRawKey(&file.GetEndKey(), rewriteRules);
        if rewriteRules.is_some() && rule.is_none() {
            let msg = format!(
                "cannot find raw rewrite rule for end key, endKey: {}",
                redact::Key(&file.GetEndKey())
            );
            return Err(
                Annotatef(Some(br_err(&ErrRestoreInvalidRewrite)), &msg, &[]).expect("annotate"),
            );
        }
        let _ = rule;
        return Ok((startKey, endKey));
    }

    // 起止解码出的表 ID 不同：拒绝改写。
    log::Error("table ids dont matched");
    Err(Annotate(Some(br_err(&ErrRestoreInvalidRewrite)), "invalid table id").expect("annotate"))
}

/// 同 GetRewriteRawKeys，但输入键已 EncodeBytes（日志备份路径）。
pub fn GetRewriteEncodedKeys(
    file: &dyn AppliedFile,
    rewriteRules: Option<&RewriteRules>,
) -> Result<(Option<Vec<u8>>, Option<Vec<u8>>), SharedError> {
    // DecodeTableID 对编码键仍可读表前缀（依赖 keydecoder/tablecodec 行为）。
    let startID = tablecodec::DecodeTableID(&file.GetStartKey());
    let endID = tablecodec::DecodeTableID(&file.GetEndKey());
    if startID == endID {
        let (startKey, rule) = rewriteEncodedKey(&file.GetStartKey(), rewriteRules);
        if rewriteRules.is_some() && rule.is_none() {
            let msg = format!(
                "cannot find encode rewrite rule for start key, startKey: {}; rewrite rules: {}",
                redact::Key(&file.GetStartKey()),
                rewriteRules.unwrap()
            );
            return Err(
                Annotatef(Some(br_err(&ErrRestoreInvalidRewrite)), &msg, &[]).expect("annotate"),
            );
        }
        let (endKey, rule) = rewriteEncodedKey(&file.GetEndKey(), rewriteRules);
        if rewriteRules.is_some() && rule.is_none() {
            // encoded 路径错误同样附带规则 Display。
            let msg = format!(
                "cannot find encode rewrite rule for end key, endKey: {}; rewrite rules: {}",
                redact::Key(&file.GetEndKey()),
                rewriteRules.unwrap()
            );
            return Err(
                Annotatef(Some(br_err(&ErrRestoreInvalidRewrite)), &msg, &[]).expect("annotate"),
            );
        }
        let _ = rule;
        return Ok((startKey, endKey));
    }

    // 与 raw 路径相同的跨表拒绝语义。
    log::Error("table ids dont matched");
    Err(Annotate(Some(br_err(&ErrRestoreInvalidRewrite)), "invalid table id").expect("annotate"))
}

/// 在 Range 键上做前缀替换（不 EncodeBytes）；供合并阶段 RewriteRange 使用。
fn replacePrefix(
    s: &[u8],
    rewriteRules: &RewriteRules,
) -> (Vec<u8>, Option<import_sstpb::RewriteRule>) {
    for rule in &rewriteRules.Data {
        if s.starts_with(&rule.OldKeyPrefix) {
            return (
                [
                    rule.NewKeyPrefix.clone(),
                    s[rule.OldKeyPrefix.len()..].to_vec(),
                ]
                .concat(),
                Some(rule.clone()),
            );
        }
    }
    // 未命中：返回原键，由调用方决定是否告警。
    (s.to_vec(), None)
}

/// 按规则改写 Range 起止键；无规则时原样返回克隆。
/// 起止表 ID 不一致返回 ErrRestoreTableIDMismatch；缺规则仅 Warn 仍继续。
pub fn RewriteRange(
    rg: &mut Range,
    rewriteRules: Option<&RewriteRules>,
) -> Result<Range, SharedError> {
    if rewriteRules.is_none() {
        return Ok(rg.clone());
    }
    let rewriteRules = rewriteRules.unwrap();
    let startID = tablecodec::DecodeTableID(&rg.StartKey);
    let endID = tablecodec::DecodeTableID(&rg.EndKey);
    if startID != endID {
        // 与文件键路径不同：此处错误类型为 TableIDMismatch。
        log::Warn("table id does not match");
        return Err(Annotate(
            Some(br_err(&ErrRestoreTableIDMismatch)),
            "table id mismatch",
        )
        .expect("annotate"));
    }

    // 先改 StartKey，再改 EndKey；中间可打 Debug 日志。
    let (newStartKey, rule) = replacePrefix(&rg.StartKey, rewriteRules);
    rg.StartKey = newStartKey;
    if rule.is_none() {
        // 缺规则不硬失败：合并树仍可能插入，上层再校验。
        log::Warn("cannot find rewrite rule");
    } else {
        log::Debug("rewrite start key");
        let _ = logutil::RewriteRule(rule.as_ref().unwrap());
    }

    // 保留旧 EndKey 供日志上下文（与 Go 侧 debug 字段对应）。
    let oldKey = rg.EndKey.clone();
    let (newEndKey, end_rule) = replacePrefix(&rg.EndKey, rewriteRules);
    rg.EndKey = newEndKey;
    if end_rule.is_none() {
        log::Warn("cannot find rewrite rule");
    } else {
        log::Debug("rewrite end key");
        let _ = (oldKey, logutil::RewriteRule(end_rule.as_ref().unwrap()));
    }
    // 返回改写后的克隆，调用方可用返回值或原地 rg。
    Ok(rg.clone())
}
