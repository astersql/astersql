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

//! Precheck builder option definitions matching Go `lightning/pkg/importer/opts`.
//!
//! 该文件把“如何组装 precheck builder”抽象成可复用闭包。
//! 与直接传一长串参数相比，这种模式更容易按调用场景组合预检信息选项与
//! mydump loader 选项，同时保持 Go 包同样的使用手感。

use crate::get_pre_info_opts::GetPreInfoOption;
use crate::mydump::MDLoaderSetupOption;

/// PrecheckItemBuilderConfig defines the config used in a precheck builder,
/// which affects the behavior for executing precheck items.
/// 两组切片分别承载预检信息采集选项和 mydump 初始化选项。
#[derive(Clone, Default)]
pub struct PrecheckItemBuilderConfig {
    pub PreInfoGetterOptions: Vec<GetPreInfoOption>,
    pub MDLoaderSetupOptions: Vec<MDLoaderSetupOption>,
}

/// PrecheckItemBuilderOption defines the options when constructing a precheck builder,
/// which affects the behavior for executing precheck items.
/// 每个 option 都是对配置对象的一次定向写入。
pub type PrecheckItemBuilderOption = Box<dyn Fn(&mut PrecheckItemBuilderConfig) + Send + Sync>;

/// WithPreInfoGetterOptions generates a precheck item builder option
/// to control the get pre info behaviors.
///
/// Go uses `slices.Clone` so later mutation of the caller's slice cannot affect the config.
/// Rust 这里通过先 `to_vec()` 再在闭包中 `clone()` 维持相同语义。
#[allow(non_snake_case)]
pub fn WithPreInfoGetterOptions(opts: &[GetPreInfoOption]) -> PrecheckItemBuilderOption {
    let cloned = opts.to_vec();
    Box::new(move |c: &mut PrecheckItemBuilderConfig| {
        c.PreInfoGetterOptions = cloned.clone();
    })
}

/// WithMDLoaderSetupOptions generates a precheck item builder option
/// to control the mydumper loader setup behaviors.
///
/// Same clone semantics as Go `slices.Clone(opts)`.
/// 因此同一个 builder option 可以被多次应用而互不串扰。
#[allow(non_snake_case)]
pub fn WithMDLoaderSetupOptions(opts: &[MDLoaderSetupOption]) -> PrecheckItemBuilderOption {
    let cloned = opts.to_vec();
    Box::new(move |c: &mut PrecheckItemBuilderConfig| {
        c.MDLoaderSetupOptions = cloned.clone();
    })
}
