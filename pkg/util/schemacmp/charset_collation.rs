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

// 字符集与排序规则（collation）上的格（lattice）实现。
//
// 对应 Go schemacmp 中 charset/collation 的偏序：`utf8mb4` 高于 `utf8`/`latin1`，
// `utf8mb3` 归一为 `utf8`；后缀不同的 collation 不可比。`Join` 求上确界，用于
// DDL 兼容性检查时合并两侧 schema 的字符集属性。

use crate::{
    AnyValue, IncompatibleError, Lattice, LatticeBox, LatticeRef, incompatibleCharsetError,
    incompatibleCollationError, typeMismatchError,
};
use std::any::Any;

/// MySQL 3 字节 utf8 别名，构造时归一为 `utf8`。
const CHARSET_UTF8MB3: &str = "utf8mb3";
/// 经典 utf8（最多 3 字节码点）。
const CHARSET_UTF8: &str = "utf8";
/// 完整 Unicode utf8（4 字节），格序最高。
const CHARSET_UTF8MB4: &str = "utf8mb4";
/// Latin1 单字节字符集。
const CHARSET_LATIN1: &str = "latin1";

/// 字符集格元素：保存归一化后的 charset 名。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct charsetLattice {
    /// 小写归一后的字符集名（`utf8mb3` 已变为 `utf8`）。
    pub value: String,
}

/// 由字符串构造字符集格：转小写，并将 `utf8mb3` 映射为 `utf8`。
pub fn Charset(cs: impl AsRef<str>) -> charsetLattice {
    let normalized = cs.as_ref().to_lowercase();
    charsetLattice {
        value: if normalized == CHARSET_UTF8MB3 {
            CHARSET_UTF8.to_owned()
        } else {
            normalized
        },
    }
}

impl Lattice for charsetLattice {
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
    fn Unwrap(&self) -> AnyValue {
        AnyValue::new(self.value.clone())
    }

    /// 比较两字符集：相等为 0；`utf8mb4` 高于 `utf8`/`latin1`；其余报不相容。
    fn Compare(&self, other: LatticeRef<'_>) -> Result<i32, IncompatibleError> {
        let Some(other) = other.as_any().downcast_ref::<Self>() else {
            return Err(typeMismatchError(self, other));
        };
        if self.value == other.value {
            return Ok(0);
        }
        // utf8mb4 可覆盖 utf8 / latin1（超集关系）。
        if self.value == CHARSET_UTF8MB4
            && matches!(other.value.as_str(), CHARSET_UTF8 | CHARSET_LATIN1)
        {
            return Ok(1);
        }
        if other.value == CHARSET_UTF8MB4
            && matches!(self.value.as_str(), CHARSET_UTF8 | CHARSET_LATIN1)
        {
            return Ok(-1);
        }
        Err(incompatibleCharsetError(&self.value, &other.value))
    }

    /// 求上确界：可比较时取较大者；`utf8` 与 `latin1` 特例合并为 `utf8mb4`。
    fn Join(&self, other: LatticeRef<'_>) -> Result<LatticeBox, IncompatibleError> {
        let Some(other) = other.as_any().downcast_ref::<Self>() else {
            return Err(typeMismatchError(self, other));
        };
        match self.Compare(other) {
            Ok(cmp) if cmp >= 0 => Ok(self.clone_box()),
            Ok(_) => Ok(other.clone_box()),
            // utf8 ↔ latin1 无直接偏序，但 join 到公共上界 utf8mb4。
            Err(_error)
                if matches!(
                    (self.value.as_str(), other.value.as_str()),
                    (CHARSET_UTF8, CHARSET_LATIN1) | (CHARSET_LATIN1, CHARSET_UTF8)
                ) =>
            {
                Ok(Box::new(Charset(CHARSET_UTF8MB4)))
            }
            Err(error) => Err(error),
        }
    }

    fn clone_box(&self) -> LatticeBox {
        Box::new(self.clone())
    }
}

/// 排序规则格：字符集部分 + `_` 后的后缀（如 `bin`、`general_ci`）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct collationLattice {
    charset: charsetLattice,
    suffix: String,
}

/// 由 collation 名构造格：按首个 `_` 拆成 charset 与后缀；无 `_` 则后缀为空。
pub fn Collation(collation: impl AsRef<str>) -> collationLattice {
    let collation = collation.as_ref();
    let (charset, suffix) = collation.split_once('_').unwrap_or((collation, ""));
    collationLattice {
        charset: Charset(charset),
        suffix: suffix.to_lowercase(),
    }
}

impl collationLattice {
    /// 还原为 `charset` 或 `charset_suffix` 形式字符串。
    fn unwrapString(&self) -> String {
        if self.suffix.is_empty() {
            self.charset.value.clone()
        } else {
            format!("{}_{}", self.charset.value, self.suffix)
        }
    }
}

impl Lattice for collationLattice {
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
    fn Unwrap(&self) -> AnyValue {
        AnyValue::new(self.unwrapString())
    }

    /// 后缀不同则不相容；否则委托字符集格比较。
    fn Compare(&self, other: LatticeRef<'_>) -> Result<i32, IncompatibleError> {
        let Some(other) = other.as_any().downcast_ref::<Self>() else {
            return Err(typeMismatchError(self, other));
        };
        if self.suffix != other.suffix {
            return Err(incompatibleCollationError(
                &self.unwrapString(),
                &other.unwrapString(),
            ));
        }
        self.charset.Compare(&other.charset)
    }

    /// 后缀不同直接失败；否则 join 字符集后保留原后缀。
    fn Join(&self, other: LatticeRef<'_>) -> Result<LatticeBox, IncompatibleError> {
        let Some(other) = other.as_any().downcast_ref::<Self>() else {
            return Err(typeMismatchError(self, other));
        };
        match self.Compare(other) {
            Ok(cmp) if cmp >= 0 => Ok(self.clone_box()),
            Ok(_) => Ok(other.clone_box()),
            Err(error) if self.suffix != other.suffix => Err(error),
            Err(error) => {
                // 字符集可 join 时合成新 collation，后缀不变。
                let charset = self.charset.Join(&other.charset).map_err(|_| error)?;
                let charset = charset
                    .as_any()
                    .downcast_ref::<charsetLattice>()
                    .unwrap()
                    .clone();
                Ok(Box::new(Self {
                    charset,
                    suffix: self.suffix.clone(),
                }))
            }
        }
    }

    fn clone_box(&self) -> LatticeBox {
        Box::new(self.clone())
    }
}
