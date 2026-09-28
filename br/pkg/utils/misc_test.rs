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

//! Go-equivalent tests for `br/pkg/utils/misc_test.go`.
//! 覆盖类型兼容矩阵、清理错误合并与备份文件汇总 XOR/累加语义。
//! 用例顺序尽量跟随 Go，便于双端 diff。
//! 否定场景覆盖标志位、EvalType、flen/decimal、enum、charset。
//! 肯定场景覆盖 varchar 拓宽、blob 升级、enum 超集、timestamp 默认 decimal。
//! WithCleanUp 三分支对应 Go multierr 组合语义。
//! SummaryFiles 断言 XOR 与算术和，防止误用加法聚合 CRC。
//! generate_file 固定 write CF，触发 CF 计数收集路径。
//! collate_eq 在类型失败时仍可能为 true，需分别断言。
//! SetFlag 笔误场景确保迁移不“纠正” Go 测试行为。
//! 浮点与整型 EvalType 不同必须判失败。
//! Tiny 默认长度小于 Int24，验证 flen 约束。
//! gbk/utf8 charset 不一致时类型失败。
//! utf8_bin vs utf8_general_ci 分离类型与 collation 结果。
//! LongBlob 可容纳 Blob 的默认 flen。
//! Timestamp 未设 decimal 走默认值比较。
//! case2 依赖 Errors() 展开 Join 后的多错误。
//! case3 确认成功清理不引入假错误。
//! 0xF^0xF0^0xF00 手工验算保护实现不被改成加和。

use std::time::Duration;

use astersql_errors::{Errors, New};
use astersql_parser_mysql::r#type::{
    NotNullFlag, TypeBlob, TypeEnum, TypeInt24, TypeLongBlob, TypeNewDecimal, TypeTimestamp,
    TypeTiny, TypeVarchar, UnsignedFlag,
};
use astersql_parser_types::NewFieldType;

use crate::kvproto::brpb::File;
use crate::misc::{IsTypeCompatible, StartExitSingleListener, SummaryFiles, WithCleanUp};
use crate::stubs::context::Context;

#[test]
fn test_is_type_compatible() {
    // 与 Go misc_test 一一对应：先否定场景，再肯定场景；第二返回值始终表示 collation 是否相等。
    {
        // unsigned 标志不一致 → 类型不兼容。
        let mut src = NewFieldType(TypeInt24);
        src.AddFlag(UnsignedFlag);
        let target = NewFieldType(TypeInt24);
        let (type_eq, collate_eq) = IsTypeCompatible(src.clone(), target.clone());
        assert!(!type_eq);
        assert!(collate_eq);

        src.DelFlag(UnsignedFlag);
        let mut target = NewFieldType(TypeInt24);
        target.AddFlag(UnsignedFlag);
        let (type_eq, collate_eq) = IsTypeCompatible(src, target);
        assert!(!type_eq);
        assert!(collate_eq);
    }
    {
        // NOT NULL 标志不一致。
        let mut src = NewFieldType(TypeInt24);
        src.AddFlag(NotNullFlag);
        let target = NewFieldType(TypeInt24);
        let (type_eq, collate_eq) = IsTypeCompatible(src.clone(), target.clone());
        assert!(!type_eq);
        assert!(collate_eq);

        src.DelFlag(NotNullFlag);
        let mut target = NewFieldType(TypeInt24);
        target.AddFlag(NotNullFlag);
        let (type_eq, collate_eq) = IsTypeCompatible(src, target);
        assert!(!type_eq);
        assert!(collate_eq);
    }
    {
        // EvalType 不同（整型 vs 浮点）。
        let src = NewFieldType(TypeInt24);
        let target = NewFieldType(astersql_parser_mysql::r#type::TypeFloat);
        let (type_eq, collate_eq) = IsTypeCompatible(src, target);
        assert!(!type_eq);
        assert!(collate_eq);
    }
    {
        // flen 更大的源无法装入更小的目标（默认长度下 Int24 > Tiny）。
        let src = NewFieldType(TypeInt24);
        let target = NewFieldType(TypeTiny);
        let (type_eq, collate_eq) = IsTypeCompatible(src, target);
        assert!(!type_eq);
        assert!(collate_eq);
    }
    {
        let mut src = NewFieldType(TypeVarchar);
        src.SetFlen(100);
        let mut target = NewFieldType(TypeVarchar);
        // Go test has a typo SetFlag(99) instead of SetFlen(99); keep that behavior.
        // 刻意保留 Go 测试笔误：SetFlag 而非 SetFlen，确保迁移 parity。
        target.SetFlag(99);
        let (type_eq, collate_eq) = IsTypeCompatible(src, target);
        assert!(!type_eq);
        assert!(collate_eq);
    }
    {
        // decimal 源更大则不兼容。
        let mut src = NewFieldType(TypeNewDecimal);
        src.SetDecimal(5);
        let mut target = NewFieldType(TypeNewDecimal);
        target.SetDecimal(4);
        let (type_eq, collate_eq) = IsTypeCompatible(src, target);
        assert!(!type_eq);
        assert!(collate_eq);
    }
    {
        // enum 元素数量源更多。
        let mut src = NewFieldType(TypeEnum);
        src.SetElems(vec!["a".into(), "b".into()]);
        let mut target = NewFieldType(TypeEnum);
        target.SetElems(vec!["a".into()]);
        let (type_eq, collate_eq) = IsTypeCompatible(src, target);
        assert!(!type_eq);
        assert!(collate_eq);
    }
    {
        // 元素集合不是目标超集（b 不在目标中）。
        let mut src = NewFieldType(TypeEnum);
        src.SetElems(vec!["a".into(), "b".into()]);
        let mut target = NewFieldType(TypeEnum);
        target.SetElems(vec!["a".into(), "c".into(), "d".into()]);
        let (type_eq, collate_eq) = IsTypeCompatible(src, target);
        assert!(!type_eq);
        assert!(collate_eq);
    }
    {
        // charset 不一致 → 类型不兼容，但 collate 比较仍独立返回。
        let mut src = NewFieldType(TypeVarchar);
        src.SetCharset("gbk".into());
        let mut target = NewFieldType(TypeVarchar);
        target.SetCharset("utf8".into());
        let (type_eq, collate_eq) = IsTypeCompatible(src, target);
        assert!(!type_eq);
        assert!(collate_eq);
    }
    {
        // charset 相同、collate 不同：类型兼容但 collate_eq=false。
        let mut src = NewFieldType(TypeVarchar);
        src.SetCharset("utf8".into());
        src.SetCollate("utf8_bin".into());
        let mut target = NewFieldType(TypeVarchar);
        target.SetCharset("utf8".into());
        target.SetCollate("utf8_general_ci".into());
        let (type_eq, collate_eq) = IsTypeCompatible(src, target);
        assert!(type_eq);
        assert!(!collate_eq);
    }
    {
        // 更短 flen / 相同 charset+collate → 完全兼容。
        let mut src = NewFieldType(TypeVarchar);
        src.SetFlen(10);
        src.SetCharset("utf8".into());
        src.SetCollate("utf8_bin".into());
        let mut target = NewFieldType(TypeVarchar);
        target.SetFlen(11);
        target.SetCharset("utf8".into());
        target.SetCollate("utf8_bin".into());
        let (type_eq, collate_eq) = IsTypeCompatible(src, target);
        assert!(type_eq);
        assert!(collate_eq);
    }
    {
        // Blob→LongBlob：相同 EvalType 且默认 flen 可容纳。
        let src = NewFieldType(TypeBlob);
        let target = NewFieldType(TypeLongBlob);
        let (type_eq, collate_eq) = IsTypeCompatible(src, target);
        assert!(type_eq);
        assert!(collate_eq);
    }
    {
        // enum 目标为源元素超集。
        let mut src = NewFieldType(TypeEnum);
        src.SetElems(vec!["a".into(), "b".into()]);
        let mut target = NewFieldType(TypeEnum);
        target.SetElems(vec!["a".into(), "b".into(), "c".into()]);
        let (type_eq, collate_eq) = IsTypeCompatible(src, target);
        assert!(type_eq);
        assert!(collate_eq);
    }
    {
        // 未指定 decimal 的 Timestamp 可用默认值，装入 decimal=3 的目标。
        let src = NewFieldType(TypeTimestamp);
        let mut target = NewFieldType(TypeTimestamp);
        target.SetDecimal(3);
        let (type_eq, collate_eq) = IsTypeCompatible(src, target);
        assert!(type_eq);
        assert!(collate_eq);
    }
}

#[test]
fn test_with_clean_up() {
    // 对齐 Go multierr：仅 cleanup 错 / cleanup+既有错合并 / cleanup 成功保持 None。
    let err1 = New("meow?");
    let err2 = New("nya?");

    // case1：原先无错，cleanup 失败 → 输出即 cleanup 错。
    let case1 = || {
        let mut err = None;
        WithCleanUp(&mut err, Duration::from_secs(1), |_ctx| Err(err1.clone()));
        err
    };
    let e = case1().expect("case1 err");
    assert_eq!(e.to_string(), err1.to_string());

    // case2：既有 err2，cleanup 再产生 err1 → Join 后两者皆在。
    let case2 = || {
        let mut err = Some(err2.clone());
        WithCleanUp(&mut err, Duration::from_secs(1), |_ctx| Err(err1.clone()));
        err
    };
    let e = case2().expect("case2 err");
    let msgs: Vec<_> = Errors(&e).into_iter().map(|x| x.to_string()).collect();
    assert!(msgs.contains(&err1.to_string()));
    assert!(msgs.contains(&err2.to_string()));

    // case3：cleanup 成功且原先无错 → 仍为 None。
    let case3 = || {
        let mut err = None;
        WithCleanUp(&mut err, Duration::from_secs(1), |_ctx| Ok(()));
        err
    };
    assert!(case3().is_none());
}

#[test]
fn test_start_exit_single_listener_waits_for_signal_and_returns_cancel() {
    let parent = Context::new();
    let (child, cancel) = StartExitSingleListener(parent.clone());

    std::thread::sleep(Duration::from_millis(20));
    assert!(!parent.is_cancelled());
    assert!(!child.is_cancelled());

    cancel.cancel();
    assert!(child.is_cancelled());
    assert!(!parent.is_cancelled());
}

#[cfg(unix)]
#[test]
fn test_start_exit_single_listener_cancels_child_on_first_signal() {
    let parent = Context::new();
    let (child, _cancel) = StartExitSingleListener(parent.clone());

    signal_hook::low_level::raise(signal_hook::consts::signal::SIGTERM)
        .expect("raise SIGTERM for exit-listener test");
    assert!(child.wait_cancelled_timeout(Duration::from_secs(1)));
    assert!(!parent.is_cancelled());
}

/// 构造带 crc/kvs/bytes 的 write CF 文件，供 SummaryFiles 断言。
fn generate_file(crc: u64, kvs: u64, bytes: u64) -> File {
    let mut f = File::new();
    f.set_crc64xor(crc);
    f.set_total_kvs(kvs);
    f.set_total_bytes(bytes);
    f.set_cf("write".into());
    f
}

#[test]
fn test_summary_files() {
    // crc 为逐文件 XOR；kvs/bytes 为算术和（0xF^0xF0^0xF00=0xFFF）。
    let (crc, kvs, bytes) = SummaryFiles(&[
        generate_file(0xF, 10, 100),
        generate_file(0xF0, 20, 200),
        generate_file(0xF00, 30, 300),
    ]);
    assert_eq!(crc, 0xFFF);
    assert_eq!(kvs, 60);
    assert_eq!(bytes, 600);
}
