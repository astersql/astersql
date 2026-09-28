// Copyright 2026 AsterSQL.

// printer crate 根模块：聚合测试用配置、内核类型、部署模式、MySQL 版本占位
// 与版本信息全局态，并再导出 `printer` 子模块 API。

#![allow(non_snake_case, non_upper_case_globals, dead_code)]

#[cfg(test)]
pub mod testsetup {
    pub use ::testsetup::*;
}

use std::sync::{LazyLock, RwLock};

/// 全局配置桩：存储引擎名与删表前检查开关。
pub mod config {
    use super::{LazyLock, RwLock};
    use serde::Serialize;

    /// 可序列化的精简配置，供 PrintTiDBInfo 打印。
    #[derive(Clone, Serialize)]
    pub struct Config {
        /// 存储引擎标识（如 unistore / tikv）。
        pub store: String,
    }

    static CONFIG: LazyLock<RwLock<Config>> = LazyLock::new(|| {
        RwLock::new(Config {
            store: "unistore".to_owned(),
        })
    });
    static CHECK_TABLE_BEFORE_DROP: RwLock<bool> = RwLock::new(false);

    /// 返回当前全局配置快照。
    pub fn GetGlobalConfig() -> Config {
        CONFIG.read().unwrap().clone()
    }

    /// 返回删表前是否检查表存在的开关。
    pub fn CheckTableBeforeDrop() -> bool {
        *CHECK_TABLE_BEFORE_DROP.read().unwrap()
    }

    /// 测试辅助：覆写 store 与 CheckTableBeforeDrop。
    pub fn set_for_test(store: &str, check_table_before_drop: bool) {
        CONFIG.write().unwrap().store = store.to_owned();
        *CHECK_TABLE_BEFORE_DROP.write().unwrap() = check_table_before_drop;
    }
}

/// Classic / Next Generation 内核类型开关。
pub mod kerneltype {
    use std::sync::atomic::{AtomicBool, Ordering};

    static NEXTGEN: AtomicBool = AtomicBool::new(false);

    /// 是否处于 NextGen 内核模式。
    pub fn IsNextGen() -> bool {
        NEXTGEN.load(Ordering::SeqCst)
    }

    /// 返回人类可读内核类型名称。
    pub fn Name() -> &'static str {
        if IsNextGen() {
            "Next Generation"
        } else {
            "Classic"
        }
    }

    /// 测试辅助：设置 NextGen 开关。
    pub fn set_nextgen_for_test(value: bool) {
        NEXTGEN.store(value, Ordering::SeqCst);
    }
}

/// 部署模式（premium / starter 等）全局态。
pub mod deploymode {
    use super::RwLock;

    static MODE: RwLock<&str> = RwLock::new("premium");

    /// 部署模式包装，提供 String() 与 Go 对齐。
    pub struct Mode(&'static str);

    impl Mode {
        /// 返回模式字符串副本。
        pub fn String(&self) -> String {
            self.0.to_owned()
        }
    }

    /// 读取当前部署模式。
    pub fn Get() -> Mode {
        Mode(*MODE.read().unwrap())
    }

    /// 测试辅助：覆写部署模式。
    pub fn set_for_test(value: &'static str) {
        *MODE.write().unwrap() = value;
    }
}

/// MySQL/TiDB 发布版本占位与 NextGen/TiDBX 版本换算。
pub mod mysql {
    const LEGACY_TIDB_RELEASE_VERSION_PLACEHOLDER: &str = "v8.4.0-this-is-a-placeholder";

    /// 经典路径下的 TiDB 发布版本占位字符串。
    pub static mut TiDBReleaseVersion: &str = LEGACY_TIDB_RELEASE_VERSION_PLACEHOLDER;

    /// NextGen 下将默认占位版本规范化为 v26.x 占位；其它原样返回。
    pub fn NormalizeTiDBReleaseVersionForNextGen(release: &str) -> String {
        if release == LEGACY_TIDB_RELEASE_VERSION_PLACEHOLDER {
            "v26.3.0-this-is-a-placeholder".to_owned()
        } else {
            release.to_owned()
        }
    }

    /// 将 `vMAJOR.MINOR.PATCH` 转为 TiDBX `CLOUD.YYYYMM.patch` 形式。
    pub fn BuildTiDBXReleaseVersion(release: &str) -> Result<String, String> {
        let raw = release.strip_prefix('v').ok_or("missing v prefix")?;
        let version = semver::Version::parse(raw).map_err(|error| error.to_string())?;
        // major 解释为相对 2000 的年份偏移，minor 为月份。
        let year = 2000 + version.major;
        if !(2025..=2099).contains(&year) || !(1..=12).contains(&version.minor) {
            return Err("invalid TiDBX release version".to_owned());
        }
        let pre = if version.pre.is_empty() {
            String::new()
        } else {
            format!("-{}", version.pre)
        };
        Ok(format!(
            "CLOUD.{year}{:02}.{}{pre}",
            version.minor, version.patch
        ))
    }
}

/// Race 检测开关占位（Rust 侧固定为 false）。
pub mod israce {
    /// 是否启用 race 检测；迁移基线固定关闭。
    pub const RaceEnabled: bool = false;
}

/// 编译期注入的 TiDB 版本元信息（edition / git / 企业扩展 hash）。
pub mod versioninfo {
    use super::RwLock;

    /// 发行版标识（Community / Enterprise 等）。
    pub static TiDBEdition: RwLock<&str> = RwLock::new("Community");
    /// Git commit hash。
    pub static TiDBGitHash: RwLock<&str> = RwLock::new("None");
    /// Git 分支。
    pub static TiDBGitBranch: RwLock<&str> = RwLock::new("None");
    /// UTC 构建时间。
    pub static TiDBBuildTS: RwLock<&str> = RwLock::new("None");
    /// 企业扩展仓库 commit；空表示未链接。
    pub static TiDBEnterpriseExtensionGitHash: RwLock<&str> = RwLock::new("");

    /// 测试辅助：一次性覆写全部版本字段。
    pub fn set_for_test(
        edition: &'static str,
        git_hash: &'static str,
        git_branch: &'static str,
        build_ts: &'static str,
        enterprise_hash: &'static str,
    ) {
        *TiDBEdition.write().unwrap() = edition;
        *TiDBGitHash.write().unwrap() = git_hash;
        *TiDBGitBranch.write().unwrap() = git_branch;
        *TiDBBuildTS.write().unwrap() = build_ts;
        *TiDBEnterpriseExtensionGitHash.write().unwrap() = enterprise_hash;
    }
}

#[path = "printer.rs"]
mod printer;
pub use printer::*;

#[cfg(test)]
#[path = "main_test.rs"]
mod main_test;
#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
#[cfg(test)]
#[path = "printer_test.rs"]
mod printer_test;
