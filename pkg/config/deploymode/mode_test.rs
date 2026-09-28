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

// 本文件由 pkg/config/deploymode/mode_test.go 迁移而来，保留 Go 测试结构。
// 测试部署模式 Mode 的 JSON/TOML 编解码和全局 currentMode 切换。

// deploymode 模块的单元测试。
//
// 部署模式（deploy mode）用于区分数据库实例的运行形态：
// - `premium`：默认的高级模式；
// - `premium_reserved`：高级预留模式（`premium` 的预留资源变体）；
// - `starter`：入门模式。
//
// 本测试覆盖三部分行为：
// 1. Mode 与 JSON 字符串之间的序列化/反序列化（含大小写归一化与非法输入报错）；
// 2. Mode 从 TOML 配置字段 `deploy-mode` 的解析；
// 3. 进程级全局部署模式 `currentMode`（原子变量）的读取与切换，
//    以及仅在 nextgen 内核下才允许 Set 的约束。

#![allow(dead_code, non_snake_case)]

// kerneltype 表示内核类型：classic（经典 TiDB 内核）或 nextgen（下一代内核）。
// 通过 feature 开关 + #[path] 选择对应实现文件，与被测代码的编译条件保持一致。
#[cfg(not(feature = "nextgen"))]
#[path = "../kerneltype/classic.rs"]
pub mod kerneltype;

#[cfg(feature = "nextgen")]
#[path = "../kerneltype/nextgen.rs"]
pub mod kerneltype;

/// 测试子模块：通过 `include!` 将被测源码 mode.rs 直接嵌入，
/// 从而可以访问其中的私有项（如全局原子变量 currentMode）。
mod deploymode_tests {
    include!("mode.rs");

    // test_mode_json 对应 Go 的 TestModeJSON，覆盖合法模式序列化、大小写兼容解析和错误输入。
    /// 验证 Mode 的 JSON 编解码：
    /// - MarshalJSON 输出带引号的小写模式名；
    /// - UnmarshalJSON 接受任意大小写并归一化，且支持 premium_reserved 别名；
    /// - 未知模式名与非字符串 JSON（如数字）都应返回错误。
    #[test]
    pub fn test_mode_json() {
        let data = PremiumReserved.MarshalJSON().expect("Go require.NoError");
        assert_eq!(
            r#""premium_reserved""#,
            String::from_utf8(data).unwrap_or_default()
        );

        let data = Starter.MarshalJSON().expect("Go require.NoError");
        assert_eq!(r#""starter""#, String::from_utf8(data).unwrap_or_default());

        let mut mode = Mode(0);
        // Go 依次 json.Unmarshal 到同一个变量，验证 Parse 的小写归一化和 premium_reserved 别名。
        mode.UnmarshalJSON(br#""premium""#)
            .expect("Go require.NoError");
        assert_eq!(Premium, mode);

        mode.UnmarshalJSON(br#""premium_reserved""#)
            .expect("Go require.NoError");
        assert_eq!(PremiumReserved, mode);

        mode.UnmarshalJSON(br#""Premium_Reserved""#)
            .expect("Go require.NoError");
        assert_eq!(PremiumReserved, mode);

        mode.UnmarshalJSON(br#""Starter""#)
            .expect("Go require.NoError");
        assert_eq!(Starter, mode);

        let err = mode
            .UnmarshalJSON(br#""unknown""#)
            .expect_err("Go require.ErrorContains");
        assert!(err.contains(r#"invalid deploy mode "unknown""#));

        // Go 对数字 JSON 期望反序列化失败，因为 UnmarshalJSON 只接受字符串。
        assert!(mode.UnmarshalJSON(b"1").is_err());
    }

    // test_mode_toml 对应 Go 的 TestModeTOML，验证 TOML deploy-mode 字段字符串到 Mode 的解析。
    /// 验证从 TOML 配置解析 Mode：先用 toml crate 解析出 `deploy-mode` 字段的
    /// Value，再交给 UnmarshalTOML 转换为 Mode，同样兼容大小写。
    #[test]
    pub fn test_mode_toml() {
        // 初始值设为 Premium，随后每次解析都应覆盖为 TOML 中指定的模式。
        let mut cfg = ModeTomlConfig { Mode: Premium };

        let decoded: toml::Value =
            toml::from_str(r#"deploy-mode = "premium_reserved""#).expect("Go require.NoError");
        cfg.Mode
            .UnmarshalTOML(decoded.get("deploy-mode").expect("deploy-mode field"))
            .expect("Go require.NoError");
        assert_eq!(PremiumReserved, cfg.Mode);

        let decoded: toml::Value =
            toml::from_str(r#"deploy-mode = "Premium""#).expect("Go require.NoError");
        cfg.Mode
            .UnmarshalTOML(decoded.get("deploy-mode").expect("deploy-mode field"))
            .expect("Go require.NoError");
        assert_eq!(Premium, cfg.Mode);

        let decoded: toml::Value =
            toml::from_str(r#"deploy-mode = "Starter""#).expect("Go require.NoError");
        cfg.Mode
            .UnmarshalTOML(decoded.get("deploy-mode").expect("deploy-mode field"))
            .expect("Go require.NoError");
        assert_eq!(Starter, cfg.Mode);
    }

    // ModeTomlConfig 对应 Go 测试中内联 struct { Mode Mode `toml:"deploy-mode"` }。
    /// 承载 TOML 配置中 `deploy-mode` 字段的测试用配置结构体。
    pub struct ModeTomlConfig {
        /// 解析得到的部署模式。
        pub Mode: Mode,
    }

    // test_current_mode 对应 Go 的 TestCurrentMode，保留 nextgen 与非 nextgen 分支的断言顺序。
    /// 验证全局部署模式的读写：
    /// - 默认模式为 Premium；
    /// - 非 nextgen 内核下 IsPremiumReserved/IsStarter 恒为 false，且 Set 直接报错；
    /// - nextgen 内核下 Set 可在合法模式间切换，非法数值（如 Mode(100)）报错。
    #[test]
    pub fn test_current_mode() {
        let original = Get();

        // Go 使用 t.Cleanup 恢复 currentMode；用结构体记录资源收尾语义。
        let _cleanup = CurrentModeCleanup { original };

        // 非 nextgen 分支：直接用原子 store 绕过 Set 写入 currentMode，
        // 验证 Is* 判断在经典内核下始终返回 false，并确认 Set 被禁止。
        if !kerneltype::IsNextGen() {
            assert_eq!(Premium, Get());
            currentMode.store(PremiumReserved.0, std::sync::atomic::Ordering::SeqCst);
            assert!(!IsPremiumReserved());
            currentMode.store(Starter.0, std::sync::atomic::Ordering::SeqCst);
            assert!(!IsStarter());
            let err = Set(PremiumReserved).expect_err("Go require.ErrorContains");
            assert!(err.contains("deploy mode can only be set for nextgen TiDB"));
            return;
        }

        // nextgen 分支：依次切换 Premium -> PremiumReserved -> Starter，
        // 每次切换后核对 Get 与各 Is* 判断的一致性。
        assert_eq!(Premium, Get());
        assert!(!IsPremiumReserved());
        assert!(!IsStarter());
        Set(PremiumReserved).expect("Go require.NoError");
        assert_eq!(PremiumReserved, Get());
        assert!(IsPremiumReserved());
        assert!(!IsStarter());
        Set(Starter).expect("Go require.NoError");
        assert_eq!(Starter, Get());
        assert!(!IsPremiumReserved());
        assert!(IsStarter());

        let err = Set(Mode(100)).expect_err("Go require.ErrorContains");
        assert!(err.contains("invalid deploy mode"));
    }

    // CurrentModeCleanup 对应 Go t.Cleanup 中恢复 original currentMode 的动作。
    /// RAII 清理器：借助 Drop 在测试结束（包括断言失败提前退出）时
    /// 把全局 currentMode 恢复为进入测试前的值，避免污染其他测试。
    pub struct CurrentModeCleanup {
        /// 测试开始前保存的原始部署模式。
        pub original: Mode,
    }

    impl Drop for CurrentModeCleanup {
        fn drop(&mut self) {
            // Go: currentMode.Store(int32(original))，这里保留测试结束后的全局状态恢复。
            currentMode.store(self.original.0, std::sync::atomic::Ordering::SeqCst);
        }
    }
}
