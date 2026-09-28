// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// VectorFloat32 距离、内积、范数与逐元素算术，对齐 Go 向量函数语义。
//
// 二元运算要求维度一致；溢出/NaN 报错。余弦距离在零向量时返回 NaN。

use crate::{InitVectorFloat32, VectorFloat32, errors};

/// 构造共享错误包装。
fn new_error(message: impl Into<String>) -> errors::SharedError {
    errors::New(message)
}

/// 为错误附加调用栈追踪（对应 Go errors.Trace）。
fn trace(error: errors::SharedError) -> errors::SharedError {
    errors::Trace(Some(error)).expect("tracing a present error returns an error")
}

impl VectorFloat32 {
    /// 断言两向量维度相同，否则返回维度不一致错误。
    pub fn checkIdenticalDims(&self, other: &VectorFloat32) -> Result<(), errors::SharedError> {
        if self.Len() != other.Len() {
            return Err(new_error(format!(
                "vectors have different dimensions: {} and {}",
                self.Len(),
                other.Len()
            )));
        }
        Ok(())
    }

    /// L2 平方距离 ∑(xᵢ−yᵢ)²，避免开方开销，常用于近邻检索。
    pub fn L2SquaredDistance(&self, other: &VectorFloat32) -> Result<f64, errors::SharedError> {
        self.checkIdenticalDims(other).map_err(trace)?;
        let mut distance = 0.0_f32;
        for (left, right) in self.Elements().iter().zip(other.Elements()) {
            let difference = left - right;
            distance += difference * difference;
        }
        Ok(distance as f64)
    }

    /// 欧氏距离 √∑(xᵢ−yᵢ)²。
    pub fn L2Distance(&self, other: &VectorFloat32) -> Result<f64, errors::SharedError> {
        Ok(self.L2SquaredDistance(other).map_err(trace)?.sqrt())
    }

    /// 内积 ∑ xᵢ·yᵢ。
    pub fn InnerProduct(&self, other: &VectorFloat32) -> Result<f64, errors::SharedError> {
        self.checkIdenticalDims(other).map_err(trace)?;
        let mut product = 0.0_f32;
        for (left, right) in self.Elements().iter().zip(other.Elements()) {
            product += left * right;
        }
        Ok(product as f64)
    }

    /// 负内积，便于最大内积搜索转为最小化距离。
    pub fn NegativeInnerProduct(&self, other: &VectorFloat32) -> Result<f64, errors::SharedError> {
        Ok(-self.InnerProduct(other).map_err(trace)?)
    }

    /// 余弦距离 = 1 − 余弦相似度；零范数时相似度为 NaN。
    pub fn CosineDistance(&self, other: &VectorFloat32) -> Result<f64, errors::SharedError> {
        self.checkIdenticalDims(other).map_err(trace)?;
        let mut product = 0.0_f32;
        let mut left_norm = 0.0_f32;
        let mut right_norm = 0.0_f32;
        for (left, right) in self.Elements().iter().zip(other.Elements()) {
            product += left * right;
            left_norm += left * left;
            right_norm += right * right;
        }

        // 夹紧到 [-1,1] 以抑制浮点误差导致的越界
        let mut similarity = product as f64 / ((left_norm as f64) * (right_norm as f64)).sqrt();
        if similarity.is_nan() {
            return Ok(f64::NAN);
        }
        similarity = similarity.clamp(-1.0, 1.0);
        Ok(1.0 - similarity)
    }

    /// L1（曼哈顿）距离 ∑|xᵢ−yᵢ|。
    pub fn L1Distance(&self, other: &VectorFloat32) -> Result<f64, errors::SharedError> {
        self.checkIdenticalDims(other).map_err(trace)?;
        let mut distance = 0.0_f32;
        for (left, right) in self.Elements().iter().zip(other.Elements()) {
            distance += (left - right).abs();
        }
        Ok(distance as f64)
    }

    /// L2 范数 √∑ xᵢ²。
    pub fn L2Norm(&self) -> f64 {
        self.Elements()
            .iter()
            .map(|value| *value as f64 * *value as f64)
            .sum::<f64>()
            .sqrt()
    }

    /// 逐元素加法。
    pub fn Add(&self, other: &VectorFloat32) -> Result<VectorFloat32, errors::SharedError> {
        self.binary_operation(other, |left, right| left + right)
    }

    /// 逐元素减法。
    pub fn Sub(&self, other: &VectorFloat32) -> Result<VectorFloat32, errors::SharedError> {
        self.binary_operation(other, |left, right| left - right)
    }

    /// 逐元素乘法。
    pub fn Mul(&self, other: &VectorFloat32) -> Result<VectorFloat32, errors::SharedError> {
        self.binary_operation(other, |left, right| left * right)
    }

    /// 同维逐元素运算，并检查结果是否溢出或产生 NaN。
    fn binary_operation(
        &self,
        other: &VectorFloat32,
        operation: impl Fn(f32, f32) -> f32,
    ) -> Result<VectorFloat32, errors::SharedError> {
        self.checkIdenticalDims(other).map_err(trace)?;
        let mut result = InitVectorFloat32(self.Len());
        for ((output, left), right) in result
            .ElementsMut()
            .iter_mut()
            .zip(self.Elements())
            .zip(other.Elements())
        {
            *output = operation(*left, *right);
        }

        for value in result.Elements() {
            if value.is_infinite() {
                return Err(new_error("value out of range: overflow"));
            }
            if value.is_nan() {
                return Err(new_error("value out of range: NaN"));
            }
            // Go intentionally leaves underflow checking as a TODO.
        }
        Ok(result)
    }

    /// 字典序比较分量，相等时再比维度；返回 -1 / 0 / 1。
    pub fn Compare(&self, other: &VectorFloat32) -> i32 {
        for (left, right) in self.Elements().iter().zip(other.Elements()) {
            if left < right {
                return -1;
            }
            if left > right {
                return 1;
            }
        }
        self.Len().cmp(&other.Len()) as i32
    }
}
