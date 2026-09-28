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

// Lightning 行编码抽象层。
//
// 定义将 SQL 行（Datum 序列）编码为可写入引擎的 KV/行对象所需的核心类型与 trait：
// - `EncodingBuilder` / `Encoder`：按表元数据与会话选项构造编码器并编码单行；
// - `Rows` / `Row`：编码结果缓冲，以及按数据行与索引行分类追加并累计校验和（checksum）；
// - `SessionOptions`：编码时依赖的 SQL Mode、时间戳、系统变量等会话上下文。

use std::any::Any;
use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use std::time::{Duration, Instant};

use verification::KVChecksum;

/// 编码过程可取消标记的轻量上下文。
#[derive(Clone, Default)]
pub struct Context {
    /// 为 true 时上层应停止继续编码/写入。
    pub cancelled: bool,
    cancel_check: Option<Arc<dyn Fn() -> bool + Send + Sync>>,
    deadline: Option<Instant>,
}

impl Context {
    pub fn cancelled() -> Self {
        Self {
            cancelled: true,
            cancel_check: None,
            deadline: None,
        }
    }

    pub fn with_timeout(timeout: Duration) -> Self {
        Self {
            cancelled: false,
            cancel_check: None,
            deadline: Some(Instant::now() + timeout),
        }
    }

    pub fn with_cancel_check(check: Arc<dyn Fn() -> bool + Send + Sync>) -> Self {
        Self {
            cancelled: false,
            cancel_check: Some(check),
            deadline: None,
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled
            || self
                .deadline
                .is_some_and(|deadline| Instant::now() >= deadline)
            || self.cancel_check.as_ref().is_some_and(|check| check())
    }
}

impl fmt::Debug for Context {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Context")
            .field("cancelled", &self.is_cancelled())
            .finish()
    }
}

/// 编码器日志附加字段（键值对形式）。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Logger {
    pub fields: Vec<(String, String)>,
}

/// 单列值的通用表示，对应 SQL 表达式求值结果中的 Datum。
#[derive(Clone, Debug, PartialEq)]
pub enum Datum {
    Null,
    MinNotNull,
    MaxValue,
    Int(i64),
    UInt(u64),
    Float(f64),
    Bytes(Vec<u8>),
    String(String),
    Json(String),
    BinaryLiteral(Vec<u8>),
    Bit(Vec<u8>),
    Enum { name: String, value: u64 },
    Set { name: String, value: u64 },
    Decimal(String),
    Timestamp(String),
    Duration(String),
}

impl Default for Datum {
    fn default() -> Self {
        Self::Null
    }
}

/// 编码所需的表元数据抽象（表名、列定义）。
pub trait Table: Send + Sync {
    fn name(&self) -> &str;
    fn columns(&self) -> &[Column];
    fn as_any(&self) -> &dyn Any;
}

/// 列的 SQL 存储类型。
///
/// `Auto` 用于旧调用方：编码时按 Datum 的实际类型写入，解码字符串类值时按
/// `charset` 判断文本或二进制。其余分支为 canonical tablecodec 提供字段类型信息。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ColumnType {
    #[default]
    Auto,
    Int,
    UInt,
    Float,
    Bytes,
    String,
    Json,
    BinaryLiteral,
    Bit,
    Enum,
    Set,
    Decimal,
    Timestamp,
    Duration,
}

/// 列元数据：SQL 存储类型、是否生成列、自增、自随机（AUTO_RANDOM）、主键等。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Column {
    pub name: String,
    /// MySQL 字符集；严格 SQL mode 下用于校验字符串输入。
    pub charset: String,
    /// SQL 存储类型；用于 canonical row/index codec 的类型恢复。
    pub column_type: ColumnType,
    /// ENUM/SET 的声明元素，按 MySQL 的 1-based 数值恢复名称。
    pub elements: Vec<String>,
    /// 生成列（GENERATED COLUMN）：值由表达式计算，而非直接来自导入数据。
    pub generated: bool,
    /// AUTO_INCREMENT 列。
    pub auto_increment: bool,
    /// AUTO_RANDOM 列：在自增基础上混入随机分片位，减轻热点。
    pub auto_random: bool,
    pub primary_key: bool,
}

/// 构造编码器时的完整配置。
#[derive(Clone, Default)]
pub struct EncodingConfig {
    pub SessionOptions: SessionOptions,
    /// 数据源路径（用于日志或错误定位）。
    pub Path: String,
    pub Table: Option<Arc<dyn Table>>,
    pub Logger: Logger,
    /// 为 true 时 AUTO_ROW_ID 不做分片混洗，直接使用原始 rowID。
    pub UseIdentityAutoRowID: bool,
}

/// 编码器工厂：按配置创建 `Encoder`，并提供空行缓冲。
pub trait EncodingBuilder: Send + Sync {
    fn NewEncoder(
        &self,
        ctx: &Context,
        config: &EncodingConfig,
    ) -> Result<Box<dyn Encoder>, EncodeError>;
    fn MakeEmptyRows(&self) -> Box<dyn Rows>;
}

/// 单行编码器：将 Datum 行编码为可分类追加的 `Row`。
pub trait Encoder: Send {
    fn Close(&mut self);
    /// 编码一行。
    ///
    /// - `row`：按源文件列顺序的原始值；
    /// - `rowID`：隐式行号（用于自动分配 handle / AUTO_RANDOM 等）；
    /// - `columnPermutation`：源列到表列的下标置换；
    /// - `offset`：源文件字节偏移，便于错误定位。
    fn Encode(
        &mut self,
        row: &[Datum],
        rowID: i64,
        columnPermutation: &[i32],
        offset: i64,
    ) -> Result<Box<dyn Row>, EncodeError>;
    fn as_any(&self) -> &dyn Any;
}

/// 编码会话选项：SQL Mode、时间戳、系统变量、最小提交时间戳（MinCommitTS）等。
///
/// MinCommitTS 用于保证导入事务的提交时间戳不低于该值，避免与在线写入的可见性冲突。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SessionOptions {
    pub SQLMode: u64,
    pub Timestamp: i64,
    pub SysVars: HashMap<String, String>,
    pub LogicalImportPrepStmt: bool,
    pub AutoRandomSeed: i64,
    pub IndexID: i64,
    pub MinCommitTS: u64,
}

/// 多行缓冲容器；`Clear` 清空内容但可复用容量。
pub trait Rows: Send {
    fn Clear(self: Box<Self>) -> Box<dyn Rows>;
    fn as_any(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;
}

/// 已编码的单行：按数据行与索引行拆分追加，并更新对应 KV 校验和。
pub trait Row: Send {
    fn ClassifyAndAppend(
        &self,
        data: &mut Box<dyn Rows>,
        dataChecksum: &mut KVChecksum,
        indices: &mut Box<dyn Rows>,
        indexChecksum: &mut KVChecksum,
    );
    fn Size(&self) -> u64;
    fn as_any(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;
}

/// 编码失败错误，消息为人类可读字符串。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EncodeError(pub String);

impl fmt::Display for EncodeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for EncodeError {}
