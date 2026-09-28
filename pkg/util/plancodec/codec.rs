// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

// 文本执行计划编解码：压缩存储、树形缩进还原与 task/ID 编解码。
//
// 对应 Go `pkg/util/plancodec` 的 codec。计划行以 tab 分隔、换行分隔节点；
// 持久化时 Snappy+base64。PhysicalID 与算子类型字符串互转；root/cop 任务类型
// 编码为 `0` / `1_<store>`。PlanDecoder 通过池复用降低分配。

use crate::id::{PhysicalIDToTypeString, TypeStringToPhysicalID};
use crate::{kv, texttree};
use base64::Engine;
use protobuf::Message;
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

/// root 任务类型编码。
const rootTaskType: &str = "0";
/// cop 任务类型编码前缀。
const copTaskType: &str = "1";
/// 计划 ID / task 段内分隔符。
const idSeparator: &str = "_";
const lineBreaker: char = '\n';
const lineBreakerStr: &str = "\n";
const separator: char = '\t';
const separatorStr: &str = "\t";

/// PlanDiscardedEncoded indicates that the plan was discarded because it was too long.
/// 文本计划过长丢弃时的编码哨兵。
pub const PlanDiscardedEncoded: &str = "[discard]";
/// 解码侧展示的“计划因过长丢弃”文案。
pub(crate) const PLAN_DISCARDED_DECODED: &str = "(plan discarded because too long)";

/// 编解码过程错误：base64/snappy/protobuf/UTF-8/非法计划/panic。
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Raw Go error string; Display is only a UTF-8 presentation boundary.
    #[error("{}", String::from_utf8_lossy(.0))]
    GoMessage(Vec<u8>),
    #[error("illegal base64 data at input byte {0}")]
    Base64(usize),
    #[error("snappy: corrupt input")]
    Snappy(#[from] snap::Error),
    #[error("protobuf operation failed: {0}")]
    Protobuf(#[from] protobuf::ProtobufError),
    #[error("plan is not valid UTF-8: {0}")]
    Utf8(#[from] std::str::Utf8Error),
    #[error("{0}")]
    InvalidPlan(String),
    #[error("DecodePlan panicked")]
    DecodePlanPanicked,
}

impl Error {
    /// Return the exact Go error bytes, including non-UTF-8 fields.
    pub fn as_bytes(&self) -> std::borrow::Cow<'_, [u8]> {
        match self {
            Self::GoMessage(bytes) => std::borrow::Cow::Borrowed(bytes),
            _ => std::borrow::Cow::Owned(self.to_string().into_bytes()),
        }
    }
}

/// BinaryPlanDiscardedEncoded returns the protobuf sentinel used for a discarded binary plan.
///
/// 构造二进制“过长丢弃”哨兵：ExplainData.discarded_due_to_too_long=true 后再 Compress。
pub fn BinaryPlanDiscardedEncoded() -> String {
    let mut binary = tipb::ExplainData::new();
    binary.set_discarded_due_to_too_long(true);
    match binary.write_to_bytes() {
        Ok(proto) => Compress(&proto),
        Err(_) => String::new(),
    }
}

/// PlanDecoder 实例池，避免频繁分配缩进缓冲。
static DECODER_POOL: LazyLock<Mutex<Vec<PlanDecoder>>> = LazyLock::new(|| Mutex::new(Vec::new()));

fn take_decoder() -> PlanDecoder {
    DECODER_POOL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .pop()
        .unwrap_or_default()
}

fn put_decoder(decoder: PlanDecoder) {
    DECODER_POOL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .push(decoder);
}

/// DecodePlan returns the rendered Go string as bytes, without imposing UTF-8.
///
/// 解压并解码为带表头的可读计划树；捕获 panic 转为 DecodePlanPanicked。
pub fn DecodePlan(planString: impl AsRef<[u8]>) -> Result<Vec<u8>, Error> {
    let planString = planString.as_ref();
    let decoded = std::panic::catch_unwind(|| {
        if planString.is_empty() {
            return Ok(Vec::new());
        }
        let mut decoder = take_decoder();
        decoder.buf.clear();
        decoder.addHeader = true;
        let result =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| decoder.decode(planString)));
        put_decoder(decoder);
        result.unwrap_or_else(|panic| std::panic::resume_unwind(panic))
    });
    match decoded {
        Ok(result) => result,
        Err(_) => {
            log::error!("DecodePlan panic");
            Err(Error::DecodePlanPanicked)
        }
    }
}

/// DecodeNormalizedPlan preserves arbitrary input bytes and omits the header.
///
/// 解码未压缩的归一化计划（不加表头）。
pub fn DecodeNormalizedPlan(planString: impl AsRef<[u8]>) -> Result<Vec<u8>, Error> {
    let planString = planString.as_ref();
    if planString.is_empty() {
        return Ok(Vec::new());
    }
    let mut decoder = take_decoder();
    decoder.buf.clear();
    decoder.addHeader = false;
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        decoder.buildPlanTree(planString)
    }));
    put_decoder(decoder);
    result.unwrap_or_else(|panic| std::panic::resume_unwind(panic))
}

/// 有状态计划解码器：深度、缩进字符、字段对齐与可选表头。
#[derive(Default)]
struct PlanDecoder {
    buf: Vec<u8>,
    depths: Vec<isize>,
    indents: Vec<Vec<char>>,
    planInfos: Vec<PlanInfo>,
    addHeader: bool,
    cacheParentIdent: HashMap<isize, usize>,
}

/// 单行计划节点：深度与字段列表。
#[derive(Clone, Default)]
struct PlanInfo {
    depth: isize,
    fields: Vec<Vec<u8>>,
}

impl PlanDecoder {
    /// 解压后构建树；若为丢弃哨兵则直接返回固定文案。
    fn decode(&mut self, planString: &[u8]) -> Result<Vec<u8>, Error> {
        let bytes = match Decompress(planString) {
            Ok(bytes) => bytes,
            Err(_error) if planString == PlanDiscardedEncoded.as_bytes() => {
                return Ok(PLAN_DISCARDED_DECODED.as_bytes().to_vec());
            }
            Err(error) => return Err(error),
        };
        self.buildPlanTree(&bytes)
    }

    /// 按行解析 PlanInfo，补缩进与列对齐后拼成多行文本。
    fn buildPlanTree(&mut self, planString: &[u8]) -> Result<Vec<u8>, Error> {
        let nodes: Vec<&[u8]> = planString.split(|&b| b == b'\n').collect();
        if self.depths.capacity() < nodes.len() {
            self.depths = Vec::with_capacity(nodes.len());
            self.planInfos = Vec::with_capacity(nodes.len());
            self.indents = Vec::with_capacity(nodes.len());
        }
        self.depths.clear();
        self.planInfos.clear();
        for node in nodes {
            if let Some(info) = decodePlanInfo(node)? {
                self.depths.push(info.depth);
                self.planInfos.push(info);
            }
        }

        if self.addHeader {
            self.addPlanHeader();
        }
        self.initPlanTreeIndents();
        self.cacheParentIdent.clear();
        // 为每个非根节点找父并填充中间竖线。
        for child_index in 1..self.depths.len() {
            let parent_index = self.findParentIndex(child_index);
            self.fillIndent(parent_index, child_index);
        }
        self.alignFields();

        self.buf.clear();
        for (row_index, info) in self.planInfos.iter().enumerate() {
            if row_index > 0 {
                self.buf.push(b'\n');
            }
            self.buf.push(b'\t');
            for ch in &self.indents[row_index] {
                self.buf
                    .extend_from_slice(ch.encode_utf8(&mut [0; 4]).as_bytes());
            }
            for (field_index, field) in info.fields.iter().enumerate() {
                if field_index > 0 {
                    self.buf.push(b'\t');
                }
                self.buf.extend_from_slice(field);
            }
        }
        Ok(self.buf.clone())
    }

    /// 在首行插入与数据列数对齐的表头。
    fn addPlanHeader(&mut self) {
        if self.planInfos.is_empty() {
            return;
        }
        let mut header = PlanInfo {
            depth: 0,
            fields: [
                "id",
                "task",
                "estRows",
                "operator info",
                "actRows",
                "execution info",
                "memory",
                "disk",
            ]
            .into_iter()
            .map(|s| s.as_bytes().to_vec())
            .collect(),
        };
        if self.planInfos[0].fields.len() < header.fields.len() {
            header.fields.truncate(self.planInfos[0].fields.len());
        }
        self.planInfos.insert(0, header);
        self.depths.insert(0, 0);
    }

    /// 按深度预填缩进，末两格放 LastNode + Identifier。
    fn initPlanTreeIndents(&mut self) {
        self.indents.clear();
        for &depth in &self.depths {
            // Go panics when make([]rune, 2*depth) has a negative length.
            let length =
                usize::try_from(depth.wrapping_mul(2)).expect("negative plan indentation length");
            let mut indent = vec![' '; length];
            if length > 0 {
                let len = indent.len();
                indent[len - 2] = texttree::TreeLastNode;
                indent[len - 1] = texttree::TreeNodeIdentifier;
            }
            self.indents.push(indent);
        }
    }

    /// 查找父节点下标；用深度→下标缓存加速。
    fn findParentIndex(&mut self, childIndex: usize) -> usize {
        self.cacheParentIdent
            .insert(self.depths[childIndex], childIndex);
        let parent_depth = self.depths[childIndex].wrapping_sub(1);
        if let Some(&parent_index) = self.cacheParentIdent.get(&parent_depth) {
            return parent_index;
        }
        for index in (1..childIndex).rev() {
            if self.depths[index] == parent_depth {
                self.cacheParentIdent.insert(parent_depth, index);
                return index;
            }
        }
        0
    }

    /// 将父到子之间的 LastNode 改为 MiddleNode，其余补 TreeBody 竖线。
    fn fillIndent(&mut self, parentIndex: usize, childIndex: usize) {
        let depth = self.depths[childIndex];
        if depth == 0 {
            return;
        }
        let indent_index = depth.wrapping_mul(2).wrapping_sub(2) as usize;
        for index in ((parentIndex + 1)..childIndex).rev() {
            if self.indents[index][indent_index] == texttree::TreeLastNode {
                self.indents[index][indent_index] = texttree::TreeMiddleNode;
                break;
            }
            self.indents[index][indent_index] = texttree::TreeBody;
        }
    }

    /// 各行列数对齐并用空格右填充（除最后一列）。
    fn alignFields(&mut self) {
        let Some(max_fields) = self.planInfos.iter().map(|info| info.fields.len()).max() else {
            return;
        };
        for info in &mut self.planInfos {
            info.fields.resize(max_fields, Vec::new());
        }
        if max_fields == 0 {
            return;
        }
        for column in 0..max_fields - 1 {
            let max_length = self.getMaxFieldLength(column);
            for row in 0..self.planInfos.len() {
                let field_length = self.getPlanFieldLen(row, column);
                self.planInfos[row].fields[column]
                    .extend(std::iter::repeat_n(b' ', max_length - field_length));
            }
        }
    }

    fn getMaxFieldLength(&self, column: usize) -> usize {
        self.planInfos
            .iter()
            .enumerate()
            .map(|(row, _)| self.getPlanFieldLen(row, column))
            .max()
            .unwrap_or(0)
    }

    /// 第 0 列宽度含缩进字符数。
    fn getPlanFieldLen(&self, row: usize, column: usize) -> usize {
        let field_length = self.planInfos[row].fields[column].len();
        if column == 0 {
            field_length + self.indents[row].len()
        } else {
            field_length
        }
    }
}

/// 解析一行 tab 分隔计划：depth、物理 ID→类型名、task、其余字段原样。
fn decodePlanInfo(line: &[u8]) -> Result<Option<PlanInfo>, Error> {
    let values: Vec<&[u8]> = line.split(|&b| b == b'\t').collect();
    if values.len() < 2 {
        return Ok(None);
    }
    let mut info = PlanInfo::default();
    for (index, value) in values.into_iter().enumerate() {
        let context = |label: &[u8], error: Vec<u8>| {
            Error::GoMessage(
                [
                    b"decode plan: ".as_slice(),
                    line,
                    b", ",
                    label,
                    b": ",
                    value,
                    b", error: ",
                    &error,
                ]
                .concat(),
            )
        };
        match index {
            0 => info.depth = go_atoi(value).map_err(|e| context(b"depth", e))?,
            1 => {
                let ids: Vec<&[u8]> = value.split(|&b| b == b'_').collect();
                if ids.len() != 1 && ids.len() != 2 {
                    return Err(Error::GoMessage(
                        [
                            b"decode plan: ".as_slice(),
                            line,
                            b" error, invalid plan id: ",
                            value,
                        ]
                        .concat(),
                    ));
                }
                let id = go_atoi(ids[0]).map_err(|e| context(b"plan id", e))?;
                let mut name = PhysicalIDToTypeString(id).into_bytes();
                if ids.len() == 2 {
                    name.push(b'_');
                    name.extend_from_slice(ids[1]);
                }
                info.fields.push(name);
            }
            2 => info
                .fields
                .push(decodeTaskType(value).map_err(|e| context(b"task type", e))?),
            _ => info.fields.push(value.to_vec()),
        }
    }
    Ok(Some(info))
}

/// EncodePlanNode writes one plan node using the Go wire-compatible tab-separated format.
///
/// 按 Go 线格式写入一个计划节点（depth/id/task/estRows/info + 可选运行时字段）。
#[allow(clippy::too_many_arguments)]
pub fn EncodePlanNode(
    depth: isize,
    pid: impl AsRef<[u8]>,
    planType: impl AsRef<[u8]>,
    rowCount: f64,
    taskTypeInfo: impl AsRef<[u8]>,
    explainInfo: impl AsRef<[u8]>,
    actRows: impl AsRef<[u8]>,
    analyzeInfo: impl AsRef<[u8]>,
    memoryInfo: impl AsRef<[u8]>,
    diskInfo: impl AsRef<[u8]>,
    buf: &mut Vec<u8>,
) {
    let explain_info = escapeString(explainInfo.as_ref());
    let (actRows, analyzeInfo, memoryInfo, diskInfo) = (
        actRows.as_ref(),
        analyzeInfo.as_ref(),
        memoryInfo.as_ref(),
        diskInfo.as_ref(),
    );
    buf.extend_from_slice(depth.to_string().as_bytes());
    buf.push(b'\t');
    buf.extend_from_slice(&encodeID(planType, pid.as_ref()));
    buf.push(b'\t');
    buf.extend_from_slice(taskTypeInfo.as_ref());
    buf.push(b'\t');
    if rowCount == f64::INFINITY {
        buf.extend_from_slice(b"+Inf");
    } else if rowCount == f64::NEG_INFINITY {
        buf.extend_from_slice(b"-Inf");
    } else if rowCount.is_nan() {
        buf.extend_from_slice(b"NaN");
    } else if rowCount.round() == rowCount {
        buf.extend_from_slice(format!("{rowCount:.0}").as_bytes());
    } else {
        buf.extend_from_slice(format!("{rowCount:.2}").as_bytes());
    }
    buf.push(b'\t');
    buf.extend_from_slice(&explain_info);
    if !actRows.is_empty()
        || !analyzeInfo.is_empty()
        || !memoryInfo.is_empty()
        || !diskInfo.is_empty()
    {
        for field in [actRows, analyzeInfo, memoryInfo, diskInfo] {
            buf.push(b'\t');
            buf.extend_from_slice(field);
        }
    }
    buf.push(b'\n');
}

/// 转义字段内的 tab/换行，避免破坏线格式。
fn escapeString(value: &[u8]) -> Vec<u8> {
    let mut result = Vec::with_capacity(value.len());
    for &byte in value {
        match byte {
            b'\t' => result.extend_from_slice(b"\\t"),
            b'\n' => result.extend_from_slice(b"\\n"),
            _ => result.push(byte),
        }
    }
    result
}

/// NormalizePlanNode writes the normalized subset of one plan node.
///
/// 写入归一化计划节点子集（depth、物理 ID、task、explainInfo）。
pub fn NormalizePlanNode(
    depth: isize,
    planType: impl AsRef<[u8]>,
    taskTypeInfo: impl AsRef<[u8]>,
    explainInfo: impl AsRef<[u8]>,
    buf: &mut Vec<u8>,
) {
    buf.extend_from_slice(depth.to_string().as_bytes());
    buf.push(b'\t');
    buf.extend_from_slice(
        TypeStringToPhysicalID(std::str::from_utf8(planType.as_ref()).unwrap_or(""))
            .to_string()
            .as_bytes(),
    );
    buf.push(b'\t');
    buf.extend_from_slice(taskTypeInfo.as_ref());
    buf.push(b'\t');
    buf.extend_from_slice(explainInfo.as_ref());
    buf.push(b'\n');
}

/// 物理类型 ID 与实例 id 拼成 `typeId_instanceId`。
fn encodeID(planType: impl AsRef<[u8]>, id: &[u8]) -> Vec<u8> {
    [
        TypeStringToPhysicalID(std::str::from_utf8(planType.as_ref()).unwrap_or(""))
            .to_string()
            .as_bytes(),
        b"_",
        id,
    ]
    .concat()
}

impl From<kv::StoreType> for u8 {
    fn from(store: kv::StoreType) -> Self {
        store as u8
    }
}

/// EncodeTaskType encodes root/cop and store type exactly like the Go implementation.
///
/// root→`0`；cop→`1_<storeType 数值>`。
pub fn EncodeTaskType(isRoot: bool, storeType: impl Into<u8>) -> String {
    let storeType = storeType.into();
    if isRoot {
        rootTaskType.to_owned()
    } else {
        format!("{}{}{}", copTaskType, idSeparator, storeType)
    }
}

/// EncodeTaskTypeForNormalize omits TiKV's store type for normalized plans.
///
/// 归一化编码：TiKV cop 省略 store 后缀，仅写 `1`。
pub fn EncodeTaskTypeForNormalize(isRoot: bool, storeType: impl Into<u8>) -> String {
    let storeType = storeType.into();
    if isRoot {
        rootTaskType.to_owned()
    } else if storeType == kv::StoreType::TiKV as u8 {
        copTaskType.to_owned()
    } else {
        format!("{}{}{}", copTaskType, idSeparator, storeType)
    }
}

/// 解码 task 字段为 `root` / `cop` / `cop[tikv|tiflash|tidb]`。
fn decodeTaskType(value: &[u8]) -> Result<Vec<u8>, Vec<u8>> {
    let segments: Vec<&[u8]> = value.split(|&b| b == b'_').collect();
    if segments[0] == b"0" {
        return Ok(b"root".to_vec());
    }
    if segments.len() == 1 {
        return Ok(b"cop".to_vec());
    }
    let name = match go_atoi(segments[1])? as u8 {
        0 => "tikv",
        1 => "tiflash",
        2 => "tidb",
        _ => "unspecified",
    };
    Ok(format!("cop[{name}]").into_bytes())
}

/// Compress compresses with Snappy and then encodes with standard base64.
///
/// Snappy 压缩后再标准 base64。
pub fn Compress(input: &[u8]) -> String {
    let compressed = snap::raw::Encoder::new()
        .compress_vec(input)
        .expect("snappy compression only fails when the input is too large");
    base64::engine::general_purpose::STANDARD.encode(compressed)
}

/// Decompress reverses standard base64 and Snappy encoding.
///
/// 标准 base64 解码后再 Snappy 解压。
pub fn Decompress(value: impl AsRef<[u8]>) -> Result<Vec<u8>, Error> {
    let compressed = decode_base64(value.as_ref())?;
    // Go reads a uint64 Uvarint then bounds it to uint32. snap reads at most
    // five bytes, so normalize only the header before using its block decoder.
    let mut length = 0_u64;
    for (index, &byte) in compressed.iter().take(10).enumerate() {
        if index == 9 && byte > 1 {
            break;
        }
        length |= ((byte & 0x7f) as u64) << (index * 7);
        if byte < 0x80 {
            if length > u32::MAX as u64 {
                break;
            }
            let mut canonical = Vec::with_capacity(compressed.len());
            while length >= 0x80 {
                canonical.push(length as u8 | 0x80);
                length >>= 7;
            }
            canonical.push(length as u8);
            canonical.extend_from_slice(&compressed[index + 1..]);
            return Ok(snap::raw::Decoder::new().decompress_vec(&canonical)?);
        }
    }
    Err(Error::Snappy(snap::Error::Header))
}

// strconv.Atoi uses platform-sized signed integers and quotes the original bytes.
fn go_atoi(value: &[u8]) -> Result<isize, Vec<u8>> {
    let error = |reason: &str| {
        format!("strconv.Atoi: parsing {}: {reason}", quote_go_bytes(value)).into_bytes()
    };
    let (negative, digits) = match value.first() {
        Some(b'-') => (true, &value[1..]),
        Some(b'+') => (false, &value[1..]),
        _ => (false, value),
    };
    if digits.is_empty() {
        return Err(error("invalid syntax"));
    }
    let mut magnitude = 0_u64;
    for &digit in digits {
        if !digit.is_ascii_digit() {
            return Err(error("invalid syntax"));
        }
        magnitude = magnitude
            .checked_mul(10)
            .and_then(|n| n.checked_add((digit - b'0') as u64))
            .ok_or_else(|| error("value out of range"))?;
    }
    let limit = isize::MAX as u64 + u64::from(negative);
    if magnitude > limit {
        return Err(error("value out of range"));
    }
    Ok(if negative {
        (magnitude as isize).wrapping_neg()
    } else {
        magnitude as isize
    })
}

include!("go_quote_printable.rs");

fn quote_go_bytes(mut value: &[u8]) -> String {
    let mut quoted = String::from("\"");
    while !value.is_empty() {
        let valid = match std::str::from_utf8(value) {
            Ok(s) => s,
            Err(e) if e.valid_up_to() > 0 => {
                std::str::from_utf8(&value[..e.valid_up_to()]).unwrap()
            }
            Err(_) => {
                quoted.push_str(&format!("\\x{:02x}", value[0]));
                value = &value[1..];
                continue;
            }
        };
        let ch = valid.chars().next().unwrap();
        value = &value[ch.len_utf8()..];
        match ch {
            '"' => quoted.push_str("\\\""),
            '\\' => quoted.push_str("\\\\"),
            '\x07' => quoted.push_str("\\a"),
            '\x08' => quoted.push_str("\\b"),
            '\x0c' => quoted.push_str("\\f"),
            '\n' => quoted.push_str("\\n"),
            '\r' => quoted.push_str("\\r"),
            '\t' => quoted.push_str("\\t"),
            '\x0b' => quoted.push_str("\\v"),
            ' ' => quoted.push(' '),
            _ => {
                let code = ch as u32;
                let index = GO_PRINTABLE_RANGES.partition_point(|&(start, _)| start <= code);
                let printable = index > 0 && code <= GO_PRINTABLE_RANGES[index - 1].1;
                if printable {
                    quoted.push(ch);
                } else if (ch as u32) < 0x20 || ch == '\x7f' {
                    quoted.push_str(&format!("\\x{:02x}", ch as u32));
                } else if (ch as u32) < 0x10000 {
                    quoted.push_str(&format!("\\u{:04x}", ch as u32));
                } else {
                    quoted.push_str(&format!("\\U{:08x}", ch as u32));
                }
            }
        }
    }
    quoted.push('"');
    quoted
}

// The base64 quantum decoder below is adapted from Go encoding/base64.
// Copyright 2009 The Go Authors.
//
// Redistribution and use in source and binary forms, with or without
// modification, are permitted provided that the following conditions are
// met:
//
//    * Redistributions of source code must retain the above copyright
// notice, this list of conditions and the following disclaimer.
//    * Redistributions in binary form must reproduce the above
// copyright notice, this list of conditions and the following disclaimer
// in the documentation and/or other materials provided with the
// distribution.
//    * Neither the name of Google LLC nor the names of its
// contributors may be used to endorse or promote products derived from
// this software without specific prior written permission.
//
// THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS
// "AS IS" AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT
// LIMITED TO, THE IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR
// A PARTICULAR PURPOSE ARE DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT
// OWNER OR CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL,
// SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT
// LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES; LOSS OF USE,
// DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON ANY
// THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT
// (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE
// OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.

// Go encoding/base64 decodeQuantum, with StdEncoding padding and non-strict bits.
// Keep original input offsets, including CR/LF, in CorruptInputError.
fn decode_base64(src: &[u8]) -> Result<Vec<u8>, Error> {
    let mut result = Vec::with_capacity(src.len() / 4 * 3);
    let mut si = 0;
    while si < src.len() {
        let mut quantum = [0_u8; 4];
        let mut j = 0;
        while j < 4 {
            if si == src.len() {
                if j == 0 {
                    return Ok(result);
                }
                return Err(Error::Base64(si - j));
            }
            let byte = src[si];
            si += 1;
            let digit = match byte {
                b'A'..=b'Z' => byte - b'A',
                b'a'..=b'z' => byte - b'a' + 26,
                b'0'..=b'9' => byte - b'0' + 52,
                b'+' => 62,
                b'/' => 63,
                b'\r' | b'\n' => continue,
                b'=' if j >= 2 => {
                    if j == 2 {
                        while si < src.len() && matches!(src[si], b'\r' | b'\n') {
                            si += 1;
                        }
                        if si == src.len() {
                            return Err(Error::Base64(si));
                        }
                        if src[si] != b'=' {
                            return Err(Error::Base64(si - 1));
                        }
                        si += 1;
                    }
                    while si < src.len() && matches!(src[si], b'\r' | b'\n') {
                        si += 1;
                    }
                    if si < src.len() {
                        return Err(Error::Base64(si));
                    }
                    break;
                }
                _ => return Err(Error::Base64(si - 1)),
            };
            quantum[j] = digit;
            j += 1;
        }
        let value = ((quantum[0] as u32) << 18)
            | ((quantum[1] as u32) << 12)
            | ((quantum[2] as u32) << 6)
            | quantum[3] as u32;
        result.push((value >> 16) as u8);
        if j >= 3 {
            result.push((value >> 8) as u8);
        }
        if j == 4 {
            result.push(value as u8);
        }
    }
    Ok(result)
}
