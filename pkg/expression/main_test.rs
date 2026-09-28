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

// Expression 包测试共用的求值上下文（EvalContext）冒烟用例。
//
// 对应 Go `main_test.go`：校验默认严格 SQL 模式，以及时区、
// 类型标志与 `max_allowed_packet` 选项能否正确注入上下文。

use exprstatic::{NewEvalContext, WithLocation, WithMaxAllowedPacket, WithTypeFlags};

/// 默认求值上下文应开启严格模式，且时区为 UTC。
#[test]
fn expression_test_context_uses_strict_mode() {
    let context = NewEvalContext(Vec::new());
    // SQLMode 中的 StrictMode 影响截断/溢出时报错还是告警。
    assert!(context.SQLMode().HasStrictMode());
    assert_eq!(context.Location(), chrono_tz::UTC);
}

/// 选项构造器应把时区、类型标志与报文大小上限写入求值上下文。
#[test]
fn expression_test_context_honors_timezone_flags_and_packet_limit() {
    let location = chrono_tz::Pacific::Guadalcanal;
    // TruncateAsWarning：超长/截断场景改为告警而非硬错误。
    let flags = crate::types::Flags::default().WithTruncateAsWarning(true);
    let context = NewEvalContext(vec![
        WithLocation(location),
        WithTypeFlags(flags),
        WithMaxAllowedPacket(16 << 20),
    ]);
    assert_eq!(context.Location(), location);
    assert_eq!(context.TypeCtx().Flags(), flags);
    assert_eq!(context.GetMaxAllowedPacket(), 16 << 20);
}
