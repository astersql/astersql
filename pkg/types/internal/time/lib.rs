// Copyright 2026 AsterSQL.

// 时间类型核心位布局与完整 TIME/DATETIME 实现门面。
//
// `CoreTime` 将年月日时分秒微秒打包进 u64，位宽与 Go `types.CoreTime`
// 一致；完整解析与运算由挂接的 `time.rs` 提供。

#![allow(non_snake_case, non_upper_case_globals, dead_code)]

pub use parser_mysql::r#type as mysql;

/// MySQL 时间打包值：高位到低位依次为年(14)、月(4)、日(5)、时(5)、分(6)、秒(6)、微秒(20)。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CoreTime(pub u64);

impl CoreTime {
    /// 按位偏移与位宽读取字段。
    fn field(self, offset: u64, width: u64) -> u64 {
        (self.0 >> offset) & ((1_u64 << width) - 1)
    }

    /// 年份（14 bit）。
    pub fn Year(self) -> i32 {
        self.field(50, 14) as i32
    }
    /// 月份（4 bit）。
    pub fn Month(self) -> i32 {
        self.field(46, 4) as i32
    }
    /// 日（5 bit）。
    pub fn Day(self) -> i32 {
        self.field(41, 5) as i32
    }
    /// 小时（5 bit）。
    pub fn Hour(self) -> i32 {
        self.field(36, 5) as i32
    }
    /// 分钟（6 bit）。
    pub fn Minute(self) -> i32 {
        self.field(30, 6) as i32
    }
    /// 秒（6 bit）。
    pub fn Second(self) -> i32 {
        self.field(24, 6) as i32
    }
    /// 微秒（20 bit）。
    pub fn Microsecond(self) -> i32 {
        self.field(4, 20) as i32
    }

    /// 写入年份字段。
    pub fn setYear(&mut self, value: u16) {
        self.set_field(50, 14, value as u64);
    }
    /// 写入月份字段。
    pub fn setMonth(&mut self, value: u8) {
        self.set_field(46, 4, value as u64);
    }
    /// 写入日字段。
    pub fn setDay(&mut self, value: u8) {
        self.set_field(41, 5, value as u64);
    }
    /// 写入小时字段。
    pub fn setHour(&mut self, value: u8) {
        self.set_field(36, 5, value as u64);
    }
    /// 写入分钟字段。
    pub fn setMinute(&mut self, value: u8) {
        self.set_field(30, 6, value as u64);
    }
    /// 写入秒字段。
    pub fn setSecond(&mut self, value: u8) {
        self.set_field(24, 6, value as u64);
    }
    /// 写入微秒字段。
    pub fn setMicrosecond(&mut self, value: u32) {
        self.set_field(4, 20, value as u64);
    }

    /// 用 mask 清旧值后写入新字段，避免影响相邻位。
    fn set_field(&mut self, offset: u64, width: u64, value: u64) {
        let mask = ((1_u64 << width) - 1) << offset;
        self.0 = (self.0 & !mask) | ((value << offset) & mask);
    }
}

/// TIME/DATETIME/DURATION 完整实现。
#[path = "../../time.rs"]
mod time_impl;

pub use time_impl::*;
