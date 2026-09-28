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

//! Get-pre-info option definitions matching Go `lightning/pkg/importer/opts`.
//!
//! 这组选项负责把调用方的若干布尔开关聚合成一次预检配置。
//! 设计保持 Go 的“默认配置 + 可选闭包叠加”模式，
//! 这样不同调用点可以按需组合，而不必显式构造整份配置对象。

use std::sync::Arc;

/// GetPreInfoConfig stores some configs to affect behavior to get pre restore infos.
/// 当前只有两个布尔位，但结构体形式为未来扩展保留了统一入口。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GetPreInfoConfig {
    pub IgnoreDBNotExist: bool,
    pub ForceReloadCache: bool,
}

impl GetPreInfoConfig {
    /// Clone clones a new independent config object from the original one.
    /// Go method allows a nil receiver; `None` returns the default config.
    /// 因此调用方可以把“没有基线配置”与“用默认配置开始”视为同一件事。
    #[allow(non_snake_case)]
    pub fn Clone(c: Option<&GetPreInfoConfig>) -> Box<GetPreInfoConfig> {
        let mut clonedCfg = NewDefaultGetPreInfoConfig();
        if let Some(c) = c {
            *clonedCfg = c.clone();
        }
        clonedCfg
    }
}

/// NewDefaultGetPreInfoConfig returns the default get-pre-info config.
/// 默认值全部关闭，代表最保守、最不带副作用的预检行为。
#[allow(non_snake_case)]
pub fn NewDefaultGetPreInfoConfig() -> Box<GetPreInfoConfig> {
    Box::new(GetPreInfoConfig {
        IgnoreDBNotExist: false,
        ForceReloadCache: false,
    })
}

/// GetPreInfoOption defines the type for passing optional arguments for PreInfoGetter methods.
///
/// Go function values are copyable; `Arc` preserves that for `slices.Clone` callers.
/// 每个 option 只修改自己关心的字段，从而允许按顺序组合。
pub type GetPreInfoOption = Arc<dyn Fn(&mut GetPreInfoConfig) + Send + Sync>;

/// WithIgnoreDBNotExist sets whether to ignore DB not exist error when getting DB schemas.
/// 该开关通常用于宽松探测远端 schema 的场景。
#[allow(non_snake_case)]
pub fn WithIgnoreDBNotExist(ignoreDBNotExist: bool) -> GetPreInfoOption {
    Arc::new(move |c: &mut GetPreInfoConfig| {
        c.IgnoreDBNotExist = ignoreDBNotExist;
    })
}

/// ForceReloadCache sets whether to reload the cache for some caching results.
/// 该选项让调用方显式放弃缓存，强制重新采集预检信息。
#[allow(non_snake_case)]
pub fn ForceReloadCache(forceReloadCache: bool) -> GetPreInfoOption {
    Arc::new(move |c: &mut GetPreInfoConfig| {
        c.ForceReloadCache = forceReloadCache;
    })
}

/// Apply a base config plus option list (call-site pattern used by Go PreInfoGetter).
/// 应用顺序与切片顺序一致，后出现的 option 可以覆盖前面的布尔值。
#[allow(non_snake_case)]
pub fn ApplyGetPreInfoOptions(
    base: Option<&GetPreInfoConfig>,
    opts: &[GetPreInfoOption],
) -> GetPreInfoConfig {
    let mut cfg = *GetPreInfoConfig::Clone(base);
    for opt in opts {
        opt(&mut cfg);
    }
    cfg
}
