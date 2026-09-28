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

// Admin Pause 测试用的表结构与随机行数据生成。
//
// Admin Pause 指通过 `ADMIN PAUSE DDL JOBS` 等管理命令暂停正在执行的
// DDL Job（DDL 任务）。本模块提供测试表 DDL、分区表 DDL，以及按行数
// 生成 INSERT 语句的辅助函数，供 pause/resume/cancel 相关用例灌入数据。
// Vector 字段用于覆盖向量索引（vector index）场景；TiFlash replica
// 用于覆盖列存副本相关 DDL。

use std::sync::atomic::{AtomicU64, Ordering};

use astersql_domain_infosync::{NewMockTiFlash, SetMockTiFlash};
use astersql_testkit::{DbValue, TestKit};
use astersql_types::{time, vector};

/// SQL 执行器抽象；生产测试使用 `TestKit`，单元测试可用记录器验证调用顺序。
pub trait SqlExecutor {
    fn must_exec(&mut self, sql: &str) -> Result<(), String>;
    fn exec(&mut self, sql: &str) -> Result<(), String>;
}

impl SqlExecutor for TestKit {
    fn must_exec(&mut self, sql: &str) -> Result<(), String> {
        self.MustExec(sql, Vec::<DbValue>::new());
        Ok(())
    }

    fn exec(&mut self, sql: &str) -> Result<(), String> {
        self.Exec(sql, Vec::<DbValue>::new())
            .map(|_| ())
            .map_err(|error| error.to_string())
    }
}

/// `()` 保留旧调用方的类型兼容性，但不再静默报告成功。
impl SqlExecutor for () {
    fn must_exec(&mut self, _sql: &str) -> Result<(), String> {
        Err("adminpause SQL executor is not configured".to_owned())
    }

    fn exec(&mut self, _sql: &str) -> Result<(), String> {
        Err("adminpause SQL executor is not configured".to_owned())
    }
}

static RNG_STATE: AtomicU64 = AtomicU64::new(0x9e37_79b9_7f4a_7c15);

fn random_below(bound: usize) -> Result<usize, String> {
    if bound == 0 {
        return Err("random character set must not be empty".to_owned());
    }
    let mut current = RNG_STATE.load(Ordering::Relaxed);
    loop {
        let next = current
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1);
        match RNG_STATE.compare_exchange_weak(current, next, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => return Ok((next as usize) % bound),
            Err(actual) => current = actual,
        }
    }
}

/// 年龄字段上限，同时作为 range 分区边界参考（分区表 p5 上界为 120）。
pub const AGE_MAX: i32 = 120;

/// 测试表 `t_user` / `t_user_vec` 的一行数据模型。
// TestTableUser 对应 Go 结构体，表示 t_user/t_user_vec 测试表的一行数据。
pub struct TestTableUser {
    /// 主键 ID（自增语义由建表语句表达）。
    pub id: i32,
    /// 租户标识字符串。
    pub tenant: String,
    /// 用户名。
    pub name: String,
    /// 年龄；分区表按 age 做 range 分区。
    pub age: i32,
    /// 省份。
    pub province: String,
    /// 城市。
    pub city: String,
    /// 电话号码。
    pub phone: String,
    /// 创建时间，使用 MySQL DATETIME 文本表示。
    pub created_time: String,
    /// 更新时间，使用 MySQL DATETIME 文本表示。
    pub updated_time: String,
    /// 向量字段的 MySQL 文本表示（仅 `t_user_vec` 使用）。
    pub vec: String,
}

/// 普通用户测试表名。
pub const ADMIN_PAUSE_TEST_TABLE: &str = "t_user";
/// 带 vector 列的用户测试表名。
pub const ADMIN_PAUSE_TEST_TABLE_WITH_VEC: &str = "t_user_vec";
/// 创建普通用户测试表的 DDL。
pub const ADMIN_PAUSE_TEST_TABLE_STMT: &str = r#"CREATE TABLE if not exists t_user (
    id int(11) NOT NULL AUTO_INCREMENT,
    tenant varchar(128) NOT NULL,
    name varchar(128) NOT NULL,
    age int(11) NOT NULL,
    province varchar(32) NOT NULL DEFAULT '',
    city varchar(32) NOT NULL DEFAULT '',
    phone varchar(16) NOT NULL DEFAULT '',
    created_time datetime NOT NULL,
    updated_time datetime NOT NULL
);"#;
/// 创建带 `vector(3)` 列的用户测试表 DDL。
pub const ADMIN_PAUSE_TEST_TABLE_STMT_WITH_VEC: &str = r#"CREATE TABLE if not exists t_user_vec (
    id int(11) NOT NULL AUTO_INCREMENT,
    tenant varchar(128) NOT NULL,
    name varchar(128) NOT NULL,
    age int(11) NOT NULL,
    province varchar(32) NOT NULL DEFAULT '',
    city varchar(32) NOT NULL DEFAULT '',
    phone varchar(16) NOT NULL DEFAULT '',
    created_time datetime NOT NULL,
    updated_time datetime NOT NULL,
    vec vector(3)
);"#;

/// Range 分区用户测试表名。
pub const ADMIN_PAUSE_TEST_PARTITION_TABLE: &str = "t_user_partition";
/// 按 `age` 做 range 分区的建表 DDL（partition：按规则把表拆成多段存储）。
pub const ADMIN_PAUSE_TEST_PARTITION_TABLE_STMT: &str = r#"CREATE TABLE if not exists t_user_partition (
    id int(11) NOT NULL AUTO_INCREMENT,
    tenant varchar(128) NOT NULL,
    name varchar(128) NOT NULL,
    age int(11) NOT NULL,
    province varchar(32) NOT NULL DEFAULT '',
    city varchar(32) NOT NULL DEFAULT '',
    phone varchar(16) NOT NULL DEFAULT '',
    created_time datetime NOT NULL,
    updated_time datetime NOT NULL
) partition by range( age ) (
    partition p0 values less than (20),
    partition p1 values less than (40),
    partition p2 values less than (60),
    partition p3 values less than (80),
    partition p4 values less than (100),
    partition p5 values less than (120),
    partition p6 values less than (160));"#;

/// 按给定字符集生成固定长度字符串。
// generateString 对应 Go 的 rand.Intn(len(letterRunes)) 选择逻辑。
pub fn generate_string(letter_runes: &[char], length: usize) -> Result<String, String> {
    let mut out = String::with_capacity(length);
    for _ in 0..length {
        let picked = letter_runes[random_below(letter_runes.len())?];
        out.push(picked);
    }
    Ok(out)
}

/// 用字母数字字符集生成 tenant / name / province / city 等字段值。
// generateName 使用字母和数字字符集生成 tenant/name/province/city。
pub fn generate_name(length: usize) -> Result<String, String> {
    let letter_runes: Vec<char> = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789"
        .chars()
        .collect();
    generate_string(&letter_runes, length)
}

/// 仅用数字字符集生成电话号码字符串。
// generatePhone 只使用数字字符集生成电话号码。
pub fn generate_phone(length: usize) -> Result<String, String> {
    let number_runes: Vec<char> = "0123456789".chars().collect();
    generate_string(&number_runes, length)
}

impl TestTableUser {
    /// 按 Go 的随机边界填充一行测试属性。
    // generateAttributes 对应 Go 方法：按 id 和随机字段填充一行测试数据。
    pub fn generate_attributes(&mut self, id: i32) -> Result<(), String> {
        self.id = id;
        self.tenant = generate_name(random_below(127)?)?;
        self.name = generate_name(random_below(127)?)?;
        self.age = random_below(AGE_MAX as usize)? as i32;
        self.province = generate_name(random_below(32)?)?;
        self.city = generate_name(random_below(32)?)?;
        self.phone = generate_phone(14)?;
        self.created_time = time::CurrentTime(time::mysql::TypeDatetime).String();
        self.updated_time = time::CurrentTime(time::mysql::TypeDatetime).String();
        self.vec = vector::InitVectorFloat32(3).String();
        Ok(())
    }

    /// 拼接批量 `INSERT` SQL；`t_user_vec` 额外写入 vector 列。
    // insertStmt 对应 Go 方法：拼接批量 INSERT SQL，vec 表多带一个 vector 字段。
    pub fn insert_stmt(&mut self, table_name: &str, count: usize) -> String {
        let mut sql = format!(
            "INSERT INTO {}(tenant, name, age, province, city, phone, created_time, updated_time) VALUES ",
            table_name
        );
        if table_name == ADMIN_PAUSE_TEST_TABLE_WITH_VEC {
            sql = format!(
                "INSERT INTO {}(tenant, name, age, province, city, phone, created_time, updated_time, vec) VALUES ",
                table_name
            );
        }

        // 逐行生成 VALUES 元组；普通表与 vector 表列数不同。
        for n in 0..count {
            let _ = self.generate_attributes(n as i32);
            // Go source ignores generateAttributes errors.
            let tuple = if table_name == ADMIN_PAUSE_TEST_TABLE {
                format!(
                    "('{}', '{}', {}, '{}', '{}', '{}', '{}', '{}')",
                    self.tenant,
                    self.name,
                    self.age,
                    self.province,
                    self.city,
                    self.phone,
                    self.created_time,
                    self.updated_time
                )
            } else {
                format!(
                    "('{}', '{}', {}, '{}', '{}', '{}', '{}', '{}', '{}')",
                    self.tenant,
                    self.name,
                    self.age,
                    self.province,
                    self.city,
                    self.phone,
                    self.created_time,
                    self.updated_time,
                    self.vec
                )
            };
            sql.push_str(&tuple);
            if n != count - 1 {
                sql.push_str(", ");
            }
        }
        sql
    }
}

/// 创建普通 admin pause 测试表，并按 `row_count` 生成插入语句。
// generateTblUser 创建普通 admin pause 测试表，并按 rowCount 插入随机行。
pub fn generate_tbl_user<E: SqlExecutor>(stmt_kit: &mut E, row_count: usize) -> Result<(), String> {
    stmt_kit.must_exec(ADMIN_PAUSE_TEST_TABLE_STMT)?;
    if row_count == 0 {
        return Ok(());
    }
    let mut tu = TestTableUser::default();
    stmt_kit.must_exec(&tu.insert_stmt(ADMIN_PAUSE_TEST_TABLE, row_count))?;
    Ok(())
}

/// 创建 vector 表、配置 TiFlash 副本，并保留 mock TiFlash 收尾顺序。
///
/// TiFlash 是列存引擎副本；此处用 SQL 字符串与注释保留 Go 侧资源生命周期。
// generateTblUserWithVec 创建 vector 表、设置 TiFlash replica，并在 defer 中关闭 mock TiFlash。
pub fn generate_tbl_user_with_vec<E: SqlExecutor>(
    stmt_kit: &mut E,
    row_count: usize,
) -> Result<(), String> {
    stmt_kit.must_exec(ADMIN_PAUSE_TEST_TABLE_STMT_WITH_VEC)?;
    if row_count == 0 {
        return Ok(());
    }
    stmt_kit.must_exec("alter table t_user_vec set tiflash replica 3 location labels 'a','b';")?;
    // Go ignores SetMockTiFlash when the global infosyncer is unavailable. The Rust mock owns no
    // status server, so dropping its local Arc replaces Go's deferred HTTP-server close.
    let _ = SetMockTiFlash(NewMockTiFlash());
    let mut tu = TestTableUser::default();
    stmt_kit.must_exec(&tu.insert_stmt(ADMIN_PAUSE_TEST_TABLE_WITH_VEC, row_count))?;
    Ok(())
}

/// 创建 range 分区测试表（函数名保留 Go 侧 `Parition` 拼写）。
// generateTblUserParition 保留 Go 源函数名中的拼写，用于创建 range partition 测试表。
pub fn generate_tbl_user_parition<E: SqlExecutor>(stmt_kit: &mut E) -> Result<(), String> {
    stmt_kit.must_exec(ADMIN_PAUSE_TEST_PARTITION_TABLE_STMT)?;
    Ok(())
}

/// 空字段默认值，供插入前再调用 `generate_attributes` 填充。
impl Default for TestTableUser {
    fn default() -> Self {
        Self {
            id: 0,
            tenant: String::new(),
            name: String::new(),
            age: 0,
            province: String::new(),
            city: String::new(),
            phone: String::new(),
            created_time: String::new(),
            updated_time: String::new(),
            vec: String::new(),
        }
    }
}
