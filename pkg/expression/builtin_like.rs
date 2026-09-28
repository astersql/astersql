// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
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

// LIKE 模式匹配内置函数（标量求值）。
//
// 对应 Go `builtin_like.go`：`expr LIKE pattern ESCAPE escape`。
// 使用校对规则（collation）提供的通配符匹配器；模式与转义在求值上下文中为常量时可缓存编译结果。

use std::sync::{Arc, Mutex};

use crate::collate;
use collate_dependency::{Collator, WildcardPattern};

use crate::legacy_vectorized_runtime::{ConstLevel, EvalContext, EvalError, ExprRef, Result, Row};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 下推到 TiKV / 表达式签名枚举中的 LIKE 标识码。
pub enum ScalarFuncSig {
    LikeSig,
}

/// LIKE 函数类：校验参数个数并构造 `builtinLikeSig`。
pub struct likeFunctionClass {
    func_name: String,
}

impl likeFunctionClass {
    pub fn new(func_name: impl Into<String>) -> Self {
        Self {
            func_name: func_name.into(),
        }
    }

    pub fn getFunction(&self, args: Vec<ExprRef>) -> Result<builtinLikeSig> {
        if args.len() != 3 {
            return Err(EvalError::Message(format!(
                "{} expects 3 arguments, got {}",
                self.func_name,
                args.len()
            )));
        }
        builtinLikeSig::new(args)
    }
}

/// LIKE 签名实现：持有参数、校对器与可选的已编译模式缓存。
pub struct builtinLikeSig {
    pub(crate) args: Vec<ExprRef>,
    pub(crate) collator: Arc<dyn Collator>,
    // Go treats this matcher as a runtime cache. It is intentionally absent
    // from Clone and guarded because an expression may be shared by sessions.
    pattern_cache: Mutex<Option<Box<dyn WildcardPattern>>>,
    return_flen: i32,
    pb_code: ScalarFuncSig,
}

impl builtinLikeSig {
    /// 使用默认 binary 校对器构造。
    pub fn new(args: Vec<ExprRef>) -> Result<Self> {
        Self::with_collator(args, Arc::new(collate::binCollator::default()))
    }

    /// 指定校对器构造；LIKE 必须恰好 3 个参数（串、模式、转义字符）。
    pub fn with_collator(args: Vec<ExprRef>, collator: Arc<dyn Collator>) -> Result<Self> {
        if args.len() != 3 {
            return Err(EvalError::Message(format!(
                "LIKE expects 3 arguments, got {}",
                args.len()
            )));
        }
        Ok(Self {
            args,
            collator,
            pattern_cache: Mutex::new(None),
            return_flen: 1,
            pb_code: ScalarFuncSig::LikeSig,
        })
    }

    /// 克隆表达式；模式缓存不共享，避免跨会话竞争。
    pub fn Clone(&self) -> Self {
        Self {
            args: self.args.clone(),
            collator: Arc::from(self.collator.Clone()),
            pattern_cache: Mutex::new(None),
            return_flen: self.return_flen,
            pb_code: self.pb_code,
        }
    }

    /// 返回值显示宽度（布尔/整型结果为 1）。
    pub fn return_flen(&self) -> i32 {
        self.return_flen
    }

    /// Protobuf / 下推用的标量函数签名码。
    pub fn pb_code(&self) -> ScalarFuncSig {
        self.pb_code
    }

    /// 测试辅助：模式缓存是否已编译。
    pub fn cache_initialized(&self) -> bool {
        self.pattern_cache
            .lock()
            .expect("LIKE pattern cache poisoned")
            .is_some()
    }

    // evalInt keeps Go's null/error order and only uses the shared matcher
    // when both pattern and escape are constant in the evaluation context.
    // 保持 Go 的 NULL/错误求值顺序；仅当模式与转义在上下文中为常量时使用共享匹配器。
    pub fn evalInt(&self, ctx: &EvalContext, row: Row) -> Result<Option<i64>> {
        let Some(value) = self.args[0].EvalString(ctx, row)? else {
            return Ok(None);
        };
        let Some(pattern_text) = self.args[1].EvalString(ctx, row)? else {
            return Ok(None);
        };
        let Some(escape) = self.args[2].EvalInt(ctx, row)? else {
            return Ok(None);
        };

        // 常量模式：编译一次写入 Mutex 缓存；非常量则每行临时 Compile。
        let matched = if self.args[1].ConstLevel() >= ConstLevel::ConstOnlyInContext
            && self.args[2].ConstLevel() >= ConstLevel::ConstOnlyInContext
        {
            let mut cache = self
                .pattern_cache
                .lock()
                .expect("LIKE pattern cache poisoned");
            if cache.is_none() {
                let mut pattern = self.collator.Pattern();
                pattern.Compile(&pattern_text, escape as u8);
                *cache = Some(pattern);
            }
            cache
                .as_ref()
                .expect("initialized LIKE cache")
                .DoMatch(&value)
        } else {
            let mut pattern = self.collator.Pattern();
            pattern.Compile(&pattern_text, escape as u8);
            pattern.DoMatch(&value)
        };
        Ok(Some(i64::from(matched)))
    }
}
