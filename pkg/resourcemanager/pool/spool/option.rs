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

// spool 线程池的功能性选项定义与加载。
//
// 对应 Go 的 `option.go`：通过函数式选项（Option pattern）配置池是否在满载时阻塞等待。

/// Options 保存 spool 的运行时开关；目前仅包含是否阻塞提交。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Options {
    /// 为 true 时，池满则轮询等待空闲槽位；为 false 时立即返回 Overload。
    pub Blocking: bool,
}

// Rust 闭包对应 Go 的 func(*Options)；这里不绑定具体生命周期，便于后续模块接线。
/// OptionFn 为修改 Options 的闭包，对应 Go 的 `func(*Options)` 选项函数。
pub type OptionFn = Box<dyn Fn(&mut Options)>;
pub type Option = OptionFn;

/// 按顺序应用选项列表，返回最终 Options（先取默认值再覆盖）。
pub fn load_options(options: &[OptionFn]) -> Options {
    let mut opts = default_option();
    for option in options {
        option(&mut opts);
    }
    opts
}

/// 返回默认选项：默认 blocking=true，与 Go 侧一致。
pub fn default_option() -> Options {
    Options { Blocking: true }
}

/// 构造设置 blocking 字段的选项闭包。
pub fn with_blocking(blocking: bool) -> OptionFn {
    Box::new(move |opts: &mut Options| {
        opts.Blocking = blocking;
    })
}

pub fn DefaultOption() -> Options {
    default_option()
}

pub fn WithBlocking(blocking: bool) -> Option {
    with_blocking(blocking)
}
