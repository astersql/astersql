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

// Hash Join build 侧的 Row Table 构建器。
//
// 将 build chunk 中的行按 join key 序列化、哈希、映射到分区，并写入
// row table segment（紧凑的按行存储）。过滤失败或 key 为 NULL 的行可按
// `keep_filtered_rows` 选择丢弃或保留。对应 Go 的 `rowTableBuilder`。

// hash join build 侧如何序列化 key、分区、预分配 row table segment 并写入行数据。
//
// preAllocHelper 对应 Go 的同名结构体：为每个分区累计预分配容量和有效 key 位置。
// pub struct PreAllocHelper {
//     pub total_row_num: i64,
//     pub valid_row_num: i64,
//     pub raw_data_len: i64,
//     pub hash_values_buf: Vec<u64>,
//     pub valid_join_key_pos_buf: Vec<usize>,
// }
//
// impl PreAllocHelper {
// reset 对应 Go 方法：只清零计数，不释放已缓存的 buffer。
//     pub fn reset(&mut self) {
//         self.total_row_num = 0;
//         self.valid_row_num = 0;
//         self.raw_data_len = 0;
//     }
//
// initBuf 对应 Go 方法：首次分配辅助 buffer，之后复用容量。
//     pub fn init_buf(&mut self) {
//         if self.hash_values_buf.capacity() == 0 {
//             self.hash_values_buf = Vec::with_capacity(1024);
//             self.valid_join_key_pos_buf = Vec::with_capacity(1024);
//         }
//         self.hash_values_buf.clear();
//         self.valid_join_key_pos_buf.clear();
//     }
// }
//
// rowTableBuilder 对应 Go 结构体：保存单个 build chunk 转 row table 时需要复用的临时向量。
// pub struct RowTableBuilder {
//     pub build_key_index: Vec<usize>,
//     pub build_key_types: Vec<types::FieldType>,
//     pub has_nullable_key: bool,
//     pub has_filter: bool,
//     pub keep_filtered_rows: bool,
//
//     pub serialized_key_vector_buffer: Vec<Vec<u8>>,
//     pub part_idx_vector: Vec<usize>,
//     pub sel_rows: Vec<usize>,
//     pub used_rows: Vec<usize>,
//     pub hash_value: Vec<u64>,
//     pub first_seg_row_size_hint: usize,
// filterVector 和 nullKeyVector 按物理行下标访问，因为 Go 的 VectorizedFilter 返回值基于物理行。
//     pub filter_vector: Vec<bool>,
//     pub null_key_vector: Vec<bool>,
//
//     pub serialized_key_lens: Vec<usize>,
//     pub serialized_keys_buffer: Vec<u8>,
//
// respill 行时需要重新计算 hash；Go 这里缓存 hash.Hash64 和临时序列化 buffer。
//     pub hash: Option<Fnv64>,
//     pub rehash_buf: Vec<u8>,
//
//     pub null_map: Vec<u8>,
//     pub partition_number: usize,
//
//     pub helpers: Vec<PreAllocHelper>,
//     pub part_id_for_each_row: Vec<usize>,
// }
//
// createRowTableBuilder 对应 Go 构造函数：按分区数初始化 helper 和 null map。
// pub fn create_row_table_builder(
//     build_key_index: Vec<usize>,
//     build_key_types: Vec<types::FieldType>,
//     partition_number: usize,
//     has_nullable_key: bool,
//     has_filter: bool,
//     keep_filtered_rows: bool,
//     null_map_length: usize,
// ) -> Box<RowTableBuilder> {
//     Box::new(RowTableBuilder {
//         build_key_index,
//         build_key_types,
//         has_nullable_key,
//         has_filter,
//         keep_filtered_rows,
//         partition_number,
//         null_map: vec![0; null_map_length],
//         helpers: (0..partition_number).map(|_| PreAllocHelper {
//             total_row_num: 0,
//             valid_row_num: 0,
//             raw_data_len: 0,
//             hash_values_buf: Vec::new(),
//             valid_join_key_pos_buf: Vec::new(),
//         }).collect(),
//         serialized_key_vector_buffer: Vec::new(),
//         part_idx_vector: Vec::new(),
//         sel_rows: Vec::new(),
//         used_rows: Vec::new(),
//         hash_value: Vec::new(),
//         first_seg_row_size_hint: 0,
//         filter_vector: Vec::new(),
//         null_key_vector: Vec::new(),
//         serialized_key_lens: Vec::new(),
//         serialized_keys_buffer: Vec::new(),
//         hash: None,
//         rehash_buf: Vec::new(),
//         part_id_for_each_row: Vec::new(),
//     })
// }
//
// impl RowTableBuilder {
// initHashValueAndPartIndexForOneChunk 对应 Go 方法：按序列化 key 计算 hash 并映射分区。
//     pub fn init_hash_value_and_part_index_for_one_chunk(&mut self, partition_mask_offset: usize, partition_number: usize) {
//         let mut h = fnv::new64();
//         let mut fake_part_index = 0_u64;
//         for (logical_row_index, physical_row_index) in self.used_rows.iter().copied().enumerate() {
//             if (!self.filter_vector.is_empty() && !self.filter_vector[physical_row_index])
//                 || (!self.null_key_vector.is_empty() && self.null_key_vector[physical_row_index])
//             {
// 被过滤或 join key 为 NULL 的行不能参与 hash 匹配，用轮转 fake 分区保持分布。
//                 self.hash_value[logical_row_index] = fake_part_index;
//                 self.part_idx_vector[logical_row_index] = fake_part_index as usize;
//                 fake_part_index = (fake_part_index + 1) % partition_number as u64;
//                 continue;
//             }
//             h.write(&self.serialized_key_vector_buffer[logical_row_index]);
//             let hash = h.sum64();
//             self.hash_value[logical_row_index] = hash;
//             self.part_idx_vector[logical_row_index] = (hash >> partition_mask_offset) as usize;
//             h.reset();
//         }
//     }
//
// checkMaxElementSize 对应 Go 方法：检查 join key 和 row table 列是否包含超过 uint32 长度的元素。
//     pub fn check_max_element_size(&self, chk: &chunk::Chunk, hash_join_ctx: &HashJoinCtxV2) -> (bool, usize) {
//         for &col_idx in &self.build_key_index {
//             let column = chk.column(col_idx);
//             if column.contains_very_large_element() {
//                 return (true, col_idx);
//             }
//         }
//         for &col_idx in &hash_join_ctx.hash_table_meta.row_columns_order {
//             let column = chk.column(col_idx);
//             if column.contains_very_large_element() {
//                 return (true, col_idx);
//             }
//         }
//         (false, 0)
//     }
//
// processOneChunk 对应 Go 的 build chunk 主入口：重置 buffer、过滤、序列化 key、分区并写入 row table。
//     pub fn process_one_chunk(
//         &mut self,
//         chk: &chunk::Chunk,
//         type_ctx: types::Context,
//         hash_join_ctx: &mut HashJoinCtxV2,
//         worker_id: usize,
//     ) -> Result<(), Error> {
//         let (element_size_exceed_limit, col_idx) = self.check_max_element_size(chk, hash_join_ctx);
//         if element_size_exceed_limit {
//             return Err(Error::new(format!("row table build failed: column contains element larger than 4GB, column index: {}", col_idx)));
//         }
//
//         self.reset_buffer(chk);
//         if self.used_rows.is_empty() {
//             return Ok(());
//         }
//
//         self.first_seg_row_size_hint =
//             std::cmp::max(1, ((self.used_rows.len() as f64 / hash_join_ctx.partition_number as f64) * 1.2) as usize);
//         if self.has_filter {
//             self.filter_vector = expression::vectorized_filter(
//                 hash_join_ctx.sess_ctx.get_expr_ctx().get_eval_ctx(),
//                 hash_join_ctx.sess_ctx.get_session_vars().enable_vectorized_expression,
//                 hash_join_ctx.build_filter,
//                 chunk::new_iterator4_chunk(chk),
//                 std::mem::take(&mut self.filter_vector),
//             )?;
//         }
//         check_sql_killer(&mut hash_join_ctx.sess_ctx.get_session_vars().sql_killer, "killedDuringBuild")?;
//
// 1. split partition：Go 先把 join key 批量序列化，后续 hash 与 row table 写入都复用结果。
//         self.serialized_keys_buffer = codec::serialize_keys(
//             type_ctx,
//             chk,
//             &self.build_key_types,
//             &self.build_key_index,
//             &self.used_rows,
//             &self.filter_vector,
//             &mut self.null_key_vector,
//             &hash_join_ctx.hash_table_meta.serialize_modes,
//             &mut self.serialized_key_vector_buffer,
//             &mut self.serialized_key_lens,
//             std::mem::take(&mut self.serialized_keys_buffer),
//         )?;
//
//         for key in &self.serialized_key_vector_buffer {
//             if key.len() > u32::MAX as usize {
//                 return Err(Error::new("row table build failed: join key contains element larger than 4GB"));
//             }
//         }
//         check_sql_killer(&mut hash_join_ctx.sess_ctx.get_session_vars().sql_killer, "killedDuringBuild")?;
//
//         self.init_hash_value_and_part_index_for_one_chunk(hash_join_ctx.partition_mask_offset, hash_join_ctx.partition_number);
//
// 2. build rowtable：真正追加 row segment，失败时不把 segment 挂进 hash table context。
//         self.append_to_row_table(chk, hash_join_ctx, worker_id)
//     }
//
// ResetBuffer 对应 Go 方法：根据 chunk 选择向量调整各类临时 buffer。
//     pub fn reset_buffer(&mut self, chk: &chunk::Chunk) {
//         self.used_rows = chk.sel().unwrap_or_default();
//         let logical_rows = chk.num_rows();
//         let physical_rows = chk.column(0).rows();
//
//         if self.used_rows.is_empty() {
//             if logical_rows <= fake_sel_length() {
//                 self.sel_rows = fake_sel()[0..logical_rows].to_vec();
//             } else {
//                 self.sel_rows = (0..logical_rows).collect();
//             }
//             self.used_rows = self.sel_rows.clone();
//         }
//
//         resize_vec(&mut self.part_idx_vector, logical_rows, 0);
//         resize_vec(&mut self.hash_value, logical_rows, 0);
//         if self.has_filter {
//             resize_vec(&mut self.filter_vector, physical_rows, false);
//         }
//         if self.has_nullable_key {
//             resize_vec(&mut self.null_key_vector, physical_rows, false);
//             for v in &mut self.null_key_vector {
//                 *v = false;
//             }
//         }
//
//         self.serialized_key_vector_buffer.clear();
//         self.serialized_key_vector_buffer.resize(logical_rows, Vec::new());
//         resize_vec(&mut self.serialized_key_lens, logical_rows, 0);
//     }
//
// initRehashUtil 对应 Go 方法：懒加载 respill 重算 hash 时需要的 hash 和 buffer。
//     pub fn init_rehash_util(&mut self) {
//         if self.rehash_buf.is_empty() {
//             self.hash = Some(fnv::new64());
//             self.rehash_buf = vec![0; serialization::UINT64_LEN];
//         }
//     }
//
// preAllocForSegmentsInSpill 对应 Go 方法：对 spill 恢复行重新分区，并预估每个 segment 容量。
//     pub fn pre_alloc_for_segments_in_spill(
//         &mut self,
//         segs: &mut [Box<RowTableSegment>],
//         chk: &chunk::Chunk,
//         hash_join_ctx: &mut HashJoinCtxV2,
//         partition_number: usize,
//     ) -> Result<(), Error> {
//         for helper in &mut self.helpers {
//             helper.reset();
//             helper.init_buf();
//         }
//
//         let row_num = chk.num_rows();
//         resize_vec(&mut self.part_id_for_each_row, row_num, 0);
//         let mut fake_part_index = 0_u64;
//
//         for i in 0..row_num {
//             if i % 200 == 0 {
// 恢复 build 阶段每 200 行检查一次 SQL killer，避免长批次无法取消。
//                 check_sql_killer(&mut hash_join_ctx.sess_ctx.get_session_vars().sql_killer, "killedDuringRestoreBuild")?;
//             }
//
//             let row = chk.get_row(i);
//             let valid_join_key = row.get_bytes(1);
//             let old_hash_value = row.get_uint64(0);
//             let has_valid_join_key = valid_join_key.first().copied().unwrap_or(0) != 0;
//
//             let (new_hash_value, part_id) = if has_valid_join_key {
//                 let (new_hash_value, part_id) =
//                     self.regenerate_hash_value_and_part_index(old_hash_value, hash_join_ctx.partition_mask_offset)?;
//                 self.helpers[part_id].valid_join_key_pos_buf.push(self.helpers[part_id].hash_values_buf.len());
//                 (new_hash_value, part_id)
//             } else {
//                 let part_id = fake_part_index as usize;
//                 let new_hash_value = fake_part_index;
//                 fake_part_index = (fake_part_index + 1) % partition_number as u64;
//                 (new_hash_value, part_id)
//             };
//
//             self.part_id_for_each_row[i] = part_id;
//             self.helpers[part_id].hash_values_buf.push(new_hash_value);
//             self.helpers[part_id].total_row_num += 1;
//             self.helpers[part_id].raw_data_len += row.get_raw_len(2) as i64;
//         }
//
//         let mut total_mem_usage = 0_i64;
//         for helper in &self.helpers {
//             total_mem_usage += helper.raw_data_len
//                 + (helper.total_row_num + helper.hash_values_buf.len() as i64) * serialization::UINT64_LEN as i64
//                 + helper.valid_join_key_pos_buf.len() as i64 * serialization::INT_LEN as i64;
//         }
//         hash_join_ctx.hash_table_context.memory_tracker.consume(total_mem_usage);
//
//         for (part_id, helper) in self.helpers.iter().enumerate() {
//             segs[part_id].raw_data = Vec::with_capacity(helper.raw_data_len as usize);
//             segs[part_id].row_start_offset = Vec::with_capacity(helper.total_row_num as usize);
//             segs[part_id].hash_values = vec![0; helper.hash_values_buf.len()];
//             segs[part_id].valid_join_key_pos = vec![0; helper.valid_join_key_pos_buf.len()];
//         }
//         Ok(())
//     }
//
// processOneRestoredChunk 对应 Go 的 spill 恢复入口：构建临时 segment，成功后再挂入上下文。
//     pub fn process_one_restored_chunk(
//         &mut self,
//         chk: &chunk::Chunk,
//         hash_join_ctx: &mut HashJoinCtxV2,
//         worker_id: usize,
//         partition_number: usize,
//     ) -> Result<(), Error> {
// 必须在 preAllocForSegmentsInSpill 前调用，和 Go 注释保持一致。
//         self.init_rehash_util();
//
//         let mut segs: Vec<Box<RowTableSegment>> =
//             (0..self.partition_number).map(|_| new_row_table_segment()).collect();
//
//         self.pre_alloc_for_segments_in_spill(&mut segs, chk, hash_join_ctx, partition_number)?;
//
//         for i in 0..chk.num_rows() {
//             if i % 200 == 0 {
//                 check_sql_killer(&mut hash_join_ctx.sess_ctx.get_session_vars().sql_killer, "killedDuringRestoreBuild")?;
//             }
//
//             let part_id = self.part_id_for_each_row[i];
//             let row = chk.get_row(i);
//             let row_data = row.get_bytes(2);
//             segs[part_id].row_start_offset.push(segs[part_id].raw_data.len() as u64);
//             segs[part_id].raw_data.extend_from_slice(row_data);
//         }
//
//         for (part_id, helper) in self.helpers.iter().enumerate() {
//             segs[part_id].hash_values.copy_from_slice(&helper.hash_values_buf);
//             segs[part_id].valid_join_key_pos.copy_from_slice(&helper.valid_join_key_pos_buf);
//         }
//
// Go 用 defer 在 err == nil 时 append；在所有步骤成功后显式追加。
//         for (part_idx, seg) in segs.into_iter().enumerate() {
//             hash_join_ctx.hash_table_context.append_row_segment(worker_id, part_idx, seg);
//         }
//         Ok(())
//     }
//
// regenerateHashValueAndPartIndex 对应 Go 方法：对旧 hash 做 rehash 并计算新分区。
//     pub fn regenerate_hash_value_and_part_index(
//         &mut self,
//         hash_value: u64,
//         partition_mask_offset: usize,
//     ) -> Result<(u64, usize), Error> {
//         let new_hash_val = rehash(hash_value, &mut self.rehash_buf, self.hash.as_mut().unwrap());
//         Ok((new_hash_val, generate_partition_index(new_hash_val, partition_mask_offset) as usize))
//     }
//
// fillSerializedKeyAndKeyLengthIfNeeded 对应 Go 方法：根据 row table meta 写入 key 长度和 key 数据。
//     pub fn fill_serialized_key_and_key_length_if_needed(
//         &self,
//         row_table_meta: &JoinTableMeta,
//         has_valid_key: bool,
//         logical_row_index: usize,
//         seg: &mut RowTableSegment,
//     ) -> i64 {
//         let mut append_row_length = 0_i64;
//         if !row_table_meta.is_join_keys_fixed_length {
// 非定长 key 需要先写 key_length；即使 key 内联，也要给后续解析提供边界。
//             let length = if has_valid_key {
//                 self.serialized_key_vector_buffer[logical_row_index].len() as u32
//             } else {
//                 0
//             };
//             seg.raw_data.extend_from_slice(&length.to_ne_bytes()[0..size_of_element_size()]);
//             append_row_length += size_of_element_size() as i64;
//         }
//
//         if !row_table_meta.is_join_keys_inlined {
//             if has_valid_key {
//                 seg.raw_data.extend_from_slice(&self.serialized_key_vector_buffer[logical_row_index]);
//                 append_row_length += self.serialized_key_vector_buffer[logical_row_index].len() as i64;
//             } else if row_table_meta.is_join_keys_fixed_length {
// 没有有效 key 但 key 定长时，Go 仍写入 fake key 保持行布局固定。
//                 seg.raw_data.extend_from_slice(&row_table_meta.fake_key_byte);
//                 append_row_length += row_table_meta.join_keys_length as i64;
//             }
//         }
//         append_row_length
//     }
//
// calculateSerializedKeyAndKeyLength 对应 Go 方法：只计算上一个函数会追加的字节数。
//     pub fn calculate_serialized_key_and_key_length(
//         &self,
//         row_table_meta: &JoinTableMeta,
//         has_valid_key: bool,
//         logical_row_index: usize,
//     ) -> i64 {
//         let mut append_row_length = 0_i64;
//         if !row_table_meta.is_join_keys_fixed_length {
//             append_row_length += size_of_element_size() as i64;
//         }
//         if !row_table_meta.is_join_keys_inlined {
//             if has_valid_key {
//                 append_row_length += self.serialized_key_vector_buffer[logical_row_index].len() as i64;
//             } else if row_table_meta.is_join_keys_fixed_length {
//                 append_row_length += row_table_meta.join_keys_length as i64;
//             }
//         }
//         append_row_length
//     }
//
// preAllocForSegments 对应普通 build 阶段预分配：按分区累计行数、有效 key 数和 rawData 容量。
//     pub fn pre_alloc_for_segments(
//         &mut self,
//         segs: &mut [Box<RowTableSegment>],
//         chk: &chunk::Chunk,
//         hash_join_ctx: &mut HashJoinCtxV2,
//     ) {
//         for helper in &mut self.helpers {
//             helper.reset();
//         }
//
//         let row_table_meta = hash_join_ctx.hash_table_meta;
//         for (logical_row_index, physical_row_index) in self.used_rows.iter().copied().enumerate() {
//             let has_valid_key = (!self.has_filter || self.filter_vector[physical_row_index])
//                 && (!self.has_nullable_key || !self.null_key_vector[physical_row_index]);
//             if !has_valid_key && !self.keep_filtered_rows {
//                 continue;
//             }
//
//             let row = chk.get_row(logical_row_index);
//             let part_idx = self.part_idx_vector[logical_row_index];
//             self.helpers[part_idx].total_row_num += 1;
//             if has_valid_key {
//                 self.helpers[part_idx].valid_row_num += 1;
//             }
//
//             let mut row_length = fake_addr_place_holder_len() as i64 + row_table_meta.null_map_length as i64;
//             row_length += self.calculate_serialized_key_and_key_length(row_table_meta, has_valid_key, logical_row_index);
//             row_length += calculate_row_data_length(row_table_meta, &row);
//             row_length += calculate_fake_length(row_length);
//             self.helpers[part_idx].raw_data_len += row_length;
//         }
//
//         let mut total_mem_usage = 0_i64;
//         for helper in &self.helpers {
//             total_mem_usage += helper.raw_data_len
//                 + (helper.total_row_num + helper.total_row_num) * serialization::UINT64_LEN as i64
//                 + helper.valid_row_num * serialization::INT_LEN as i64;
//         }
//         hash_join_ctx.hash_table_context.memory_tracker.consume(total_mem_usage);
//
//         for (part_idx, seg) in segs.iter_mut().enumerate() {
//             seg.raw_data = Vec::with_capacity(self.helpers[part_idx].raw_data_len as usize);
//             seg.hash_values = Vec::with_capacity(self.helpers[part_idx].total_row_num as usize);
//             seg.row_start_offset = Vec::with_capacity(self.helpers[part_idx].total_row_num as usize);
//             seg.valid_join_key_pos = Vec::with_capacity(self.helpers[part_idx].valid_row_num as usize);
//         }
//     }
//
// appendToRowTable 对应普通 build 阶段写入 row table segment 的主流程。
//     pub fn append_to_row_table(
//         &mut self,
//         chk: &chunk::Chunk,
//         hash_join_ctx: &mut HashJoinCtxV2,
//         worker_id: usize,
//     ) -> Result<(), Error> {
//         let mut segs: Vec<Box<RowTableSegment>> =
//             (0..self.partition_number).map(|_| new_row_table_segment()).collect();
//
//         self.pre_alloc_for_segments(&mut segs, chk, hash_join_ctx);
//         let row_table_meta = hash_join_ctx.hash_table_meta;
//
//         for (logical_row_index, physical_row_index) in self.used_rows.iter().copied().enumerate() {
//             if logical_row_index % 10 == 0 || logical_row_index == self.used_rows.len() - 1 {
// Go 普通 build 更频繁检查 SQL killer，保证大 chunk 写入时可取消。
//                 check_sql_killer(&mut hash_join_ctx.sess_ctx.get_session_vars().sql_killer, "killedDuringBuild")?;
//             }
//
//             let has_valid_key = (!self.has_filter || self.filter_vector[physical_row_index])
//                 && (!self.has_nullable_key || !self.null_key_vector[physical_row_index]);
//             if !has_valid_key && !self.keep_filtered_rows {
//                 continue;
//             }
//
// need append the row to rowTable
//             let row = chk.get_row(logical_row_index);
//             let part_idx = self.part_idx_vector[logical_row_index];
//             let seg = &mut segs[part_idx];
//             if has_valid_key {
//                 seg.valid_join_key_pos.push(seg.hash_values.len());
//             }
//             seg.hash_values.push(self.hash_value[logical_row_index]);
//             seg.row_start_offset.push(seg.raw_data.len() as u64);
//
//             let mut row_length = 0_i64;
//             row_length += fill_next_row_ptr(seg) as i64;
//             row_length += fill_null_map(row_table_meta, &row, seg, &mut self.null_map) as i64;
//             row_length += self.fill_serialized_key_and_key_length_if_needed(row_table_meta, has_valid_key, logical_row_index, seg);
//             row_length += fill_row_data(row_table_meta, &row, seg);
//
// Go 用 fakeAddrPlaceHolder 补齐 8 字节对齐，便于后续按指针宽度访问。
//             if row_length % 8 != 0 {
//                 let pad = (8 - row_length % 8) as usize;
//                 seg.raw_data.extend_from_slice(&fake_addr_place_holder()[0..pad]);
//             }
//         }
//
// 与 Go defer err == nil 逻辑等价：所有写入成功后再挂入 hash table context。
//         for (part_idx, seg) in segs.into_iter().enumerate() {
//             hash_join_ctx.hash_table_context.append_row_segment(worker_id, part_idx, seg);
//         }
//         Ok(())
//     }
// }
//
// resizeSlice 对应 Go 泛型函数：容量够则复用，不够则新建。
// pub fn resize_vec<T: Clone>(s: &mut Vec<T>, new_size: usize, default_value: T) {
//     s.resize(new_size, default_value);
// }
//
// fillNullMap 对应 Go 函数：按 rowColumnsOrder 把 NULL 位图写入 segment rawData。
// pub fn fill_null_map(
//     row_table_meta: &JoinTableMeta,
//     row: &chunk::Row,
//     seg: &mut RowTableSegment,
//     bitmap: &mut [u8],
// ) -> usize {
//     let null_map_length = row_table_meta.null_map_length;
//     if null_map_length > 0 {
//         for b in &mut bitmap[0..null_map_length] {
//             *b = 0;
//         }
//
//         for (col_index_in_row_table, col_index_in_row) in row_table_meta.row_columns_order.iter().copied().enumerate() {
//             let col_index_in_bitmap = col_index_in_row_table + row_table_meta.col_offset_in_null_map;
//             if row.is_null(col_index_in_row) {
//                 bitmap[col_index_in_bitmap / 8] |= 1 << (7 - col_index_in_bitmap % 8);
//             }
//         }
//         seg.raw_data.extend_from_slice(&bitmap[0..null_map_length]);
//         return null_map_length;
//     }
//     0
// }
//
// fillNextRowPtr 对应 Go 函数：在行首写入 next_row_ptr 占位。
// pub fn fill_next_row_ptr(seg: &mut RowTableSegment) -> usize {
//     seg.raw_data.extend_from_slice(fake_addr_place_holder());
//     fake_addr_place_holder_len()
// }
//
// fillRowData 对应 Go 函数：按 row table 列顺序追加定长或变长列数据。
// pub fn fill_row_data(row_table_meta: &JoinTableMeta, row: &chunk::Row, seg: &mut RowTableSegment) -> i64 {
//     let mut append_row_length = 0_i64;
//     for (index, col_idx) in row_table_meta.row_columns_order.iter().copied().enumerate() {
//         if row_table_meta.columns_size[index] > 0 {
// fixed size：定长列直接追加 raw bytes。
//             seg.raw_data.extend_from_slice(row.get_raw(col_idx));
//             append_row_length += row_table_meta.columns_size[index] as i64;
//         } else {
// length, raw_data：变长列先写 uint32 长度，再写原始数据。
//             let raw = row.get_raw(col_idx);
//             let length = raw.len() as u32;
//             seg.raw_data.extend_from_slice(&length.to_ne_bytes()[0..size_of_element_size()]);
//             seg.raw_data.extend_from_slice(raw);
//             append_row_length += length as i64 + size_of_element_size() as i64;
//         }
//     }
//     append_row_length
// }
//
// calculateRowDataLength 对应 Go 函数：只计算 fillRowData 将追加的字节数。
// pub fn calculate_row_data_length(row_table_meta: &JoinTableMeta, row: &chunk::Row) -> i64 {
//     let mut append_row_length = 0_i64;
//     for (index, col_idx) in row_table_meta.row_columns_order.iter().copied().enumerate() {
//         if row_table_meta.columns_size[index] > 0 {
//             append_row_length += row_table_meta.columns_size[index] as i64;
//         } else {
//             append_row_length += row.get_raw_len(col_idx) as i64 + size_of_element_size() as i64;
//         }
//     }
//     append_row_length
// }
//
// calculateFakeLength 对应 Go 函数：计算补齐到 8 字节对齐需要的假字节数。
// pub fn calculate_fake_length(row_length: i64) -> i64 {
//     (8 - row_length % 8) % 8
// }
// */
use crate::join_row_table::{RowTable, RowTableSegment, SIZE_OF_NEXT_PTR};
use crate::join_table_meta::{EncodedRow, JoinTableMeta, KeyMode};
use std::sync::atomic::AtomicBool;

/// Join 行中的单元格取值，覆盖 NULL 与常见标量类型。
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Int(i64),
    UInt(u64),
    Float(f64),
    Bytes(Vec<u8>),
    Text(String),
}

/// 简化的列式 chunk：外层为行，内层为各列 `Value`。
pub type Chunk = Vec<Vec<Value>>;

/// 将 build chunk 写入分区 row table 的构建器。
#[derive(Clone, Debug)]
pub struct RowTableBuilder {
    /// Build 侧 join key 列下标。
    pub build_key_indices: Vec<usize>,
    /// 分区数，必须为 2 的幂。
    pub partition_number: usize,
    /// Join key 是否可能为 NULL。
    pub has_nullable_key: bool,
    /// Build 侧是否带过滤条件。
    pub has_filter: bool,
    /// 过滤失败 / NULL key 行是否仍写入 row table。
    pub keep_filtered_rows: bool,
    /// Null map 字节长度。
    pub null_map_length: usize,
    /// 每行计算出的 hash 值。
    pub hash_values: Vec<u64>,
    /// 每行所属分区下标。
    pub partition_indices: Vec<usize>,
    /// 每行是否可作为有效 join key。
    pub valid_keys: Vec<bool>,
    /// 每行序列化后的 join key 字节。
    pub serialized_keys: Vec<Vec<u8>>,
}

impl RowTableBuilder {
    /// 构造构建器；`partition_number` 须为非零 2 的幂。
    pub fn new(
        build_key_indices: Vec<usize>,
        partition_number: usize,
        has_nullable_key: bool,
        has_filter: bool,
        keep_filtered_rows: bool,
        null_map_length: usize,
    ) -> Result<Self, String> {
        if partition_number == 0 || !partition_number.is_power_of_two() {
            return Err("partition number must be a non-zero power of two".into());
        }
        Ok(Self {
            build_key_indices,
            partition_number,
            has_nullable_key,
            has_filter,
            keep_filtered_rows,
            null_map_length,
            hash_values: Vec::new(),
            partition_indices: Vec::new(),
            valid_keys: Vec::new(),
            serialized_keys: Vec::new(),
        })
    }
    /// 按行数重置哈希、分区与序列化缓冲。
    pub fn reset_buffer(&mut self, row_count: usize) {
        self.hash_values.clear();
        self.hash_values.resize(row_count, 0);
        self.partition_indices.clear();
        self.partition_indices.resize(row_count, 0);
        self.valid_keys.clear();
        self.valid_keys.resize(row_count, true);
        self.serialized_keys.clear();
        self.serialized_keys.resize_with(row_count, Vec::new);
    }
    /// 处理一个 build chunk：过滤、序列化 key、分区并写入 row table。
    pub fn process_chunk(
        &mut self,
        chunk: &Chunk,
        meta: &JoinTableMeta,
        filter_result: Option<&[bool]>,
        partition_mask_offset: u32,
    ) -> Result<RowTable, String> {
        self.reset_buffer(chunk.len());
        let mut partitions: Vec<RowTableSegment> = (0..self.partition_number)
            .map(|_| RowTableSegment::default())
            .collect();
        let mut fake_partition_index = 0_usize;
        for (row_index, row) in chunk.iter().enumerate() {
            if row.len() < meta.build_types.len() {
                return Err(format!("row {row_index} has insufficient columns"));
            }
            let passed_filter = filter_result
                .and_then(|result| result.get(row_index))
                .copied()
                .unwrap_or(true);
            let has_null_key = self
                .build_key_indices
                .iter()
                .any(|index| matches!(row[*index], Value::Null));
            // 过滤失败或 nullable key 含 NULL 的行不能参与哈希匹配。
            let valid = passed_filter && !(self.has_nullable_key && has_null_key);
            self.valid_keys[row_index] = valid;
            if !valid && !self.keep_filtered_rows {
                continue;
            }
            let key = serialize_key(row, &self.build_key_indices, meta.key_mode);
            let (hash, partition) = if valid {
                let hash = hash_bytes(&key);
                // 用 hash 高位映射到 2 的幂分区。
                let partition =
                    ((hash >> partition_mask_offset) as usize) & (self.partition_number - 1);
                (hash, partition)
            } else {
                // 与 Go 一致：保留的过滤/NULL-key 行没有可用 hash，使用递增假值
                // 轮询分区，避免大量无效行集中到同一个 partition。
                let partition = fake_partition_index;
                fake_partition_index = (fake_partition_index + 1) % self.partition_number;
                (partition as u64, partition)
            };
            self.hash_values[row_index] = hash;
            self.partition_indices[row_index] = partition;
            self.serialized_keys[row_index] = key.clone();
            let encoded = encode_row(row, meta, key);
            let segment = &mut partitions[partition];
            segment.raw_data.extend_from_slice(&encoded.bytes);
            segment.hash_values.push(hash);
            segment.partition_indices.push(partition);
            segment.next_rows.push(None);
            segment.rows.push(encoded);
            if valid {
                segment
                    .valid_join_key_positions
                    .push(segment.rows.len() - 1);
                segment.valid_key_count += 1;
            }
        }
        let mut table = RowTable::default();
        for mut segment in partitions {
            if !segment.rows.is_empty() {
                segment.init_tagged_bits();
                table.segments_mut().push(segment);
            }
        }
        Ok(table)
    }
    /// 处理 spill 恢复后的 chunk；分区数必须与构建器一致。
    pub fn process_restored_chunk(
        &mut self,
        chunk: &Chunk,
        meta: &JoinTableMeta,
        partition_number: usize,
    ) -> Result<RowTable, String> {
        if partition_number != self.partition_number {
            return Err("restored partition count mismatch".into());
        }
        self.process_chunk(chunk, meta, None, 0)
    }
    /// 根据已有 hash 与 mask offset 重新计算分区下标（respill 场景）。
    pub fn regenerate_hash_and_partition(
        &self,
        hash_value: u64,
        partition_mask_offset: u32,
    ) -> Result<(u64, usize), String> {
        if self.partition_number == 0 {
            return Err("partition number is zero".into());
        }
        Ok((
            hash_value,
            ((hash_value >> partition_mask_offset) as usize) & (self.partition_number - 1),
        ))
    }
    /// 检查 chunk 中最大行编码长度是否不超过 `maximum`。
    pub fn check_max_element_size(
        &self,
        chunk: &Chunk,
        meta: &JoinTableMeta,
        maximum: usize,
    ) -> (bool, usize) {
        let max = chunk
            .iter()
            .map(|row| {
                calculate_row_data_length(meta, row) + meta.fixed_key_length + SIZE_OF_NEXT_PTR
            })
            .max()
            .unwrap_or(0);
        (max <= maximum, max)
    }
}

/// 按元数据将一行编码为 `EncodedRow`（null map + key + 列数据，8 字节对齐）。
fn encode_row(row: &[Value], meta: &JoinTableMeta, key: Vec<u8>) -> EncodedRow {
    let mut null_map = vec![0_u8; meta.null_map_length];
    for (saved_index, column_index) in meta.row_columns_order.iter().enumerate() {
        if matches!(row[*column_index], Value::Null) {
            null_map[saved_index / 8] |= 1 << (saved_index % 8);
        }
    }
    let mut bytes = Vec::new();
    if meta.need_used_flag {
        bytes.push(0);
    }
    bytes.extend_from_slice(&null_map);
    let key_offset = bytes.len();
    bytes.extend_from_slice(&key);
    let row_data_offset = bytes.len();
    for index in &meta.row_columns_order {
        encode_value(&row[*index], &mut bytes);
    }
    // 填充到 8 字节对齐，便于 tagged pointer 与裸指针访问。
    let padding = (8 - bytes.len() % 8) % 8;
    bytes.resize(bytes.len() + padding, 0);
    EncodedRow {
        bytes,
        null_map,
        key_offset,
        key_length: key.len(),
        row_data_offset,
        used: AtomicBool::new(false),
    }
}

/// 按 key 模式序列化一行的 join key 列。
fn serialize_key(row: &[Value], indices: &[usize], mode: KeyMode) -> Vec<u8> {
    let mut output = Vec::new();
    for index in indices {
        let start = output.len();
        encode_value(&row[*index], &mut output);
        // VariableSerialized 模式在每个字段前写入长度前缀。
        if mode == KeyMode::VariableSerialized {
            let length = (output.len() - start) as u32;
            output.splice(start..start, length.to_le_bytes());
        }
    }
    output
}

/// 将单个 `Value` 追加编码到字节缓冲。
fn encode_value(value: &Value, output: &mut Vec<u8>) {
    match value {
        Value::Null => output.push(0),
        Value::Bool(value) => output.push(u8::from(*value)),
        Value::Int(value) => output.extend_from_slice(&value.to_le_bytes()),
        Value::UInt(value) => output.extend_from_slice(&value.to_le_bytes()),
        Value::Float(value) => output.extend_from_slice(&value.to_bits().to_le_bytes()),
        Value::Bytes(value) => {
            output.extend_from_slice(&(value.len() as u32).to_le_bytes());
            output.extend_from_slice(value);
        }
        Value::Text(value) => {
            output.extend_from_slice(&(value.len() as u32).to_le_bytes());
            output.extend_from_slice(value.as_bytes());
        }
    }
}

/// FNV-1a 风格的 64 位哈希，用于分区与冲突链查找。
fn hash_bytes(bytes: &[u8]) -> u64 {
    bytes.iter().fold(1469598103934665603_u64, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(1099511628211)
    })
}

/// 设置 segment 中某行的 next 指针（哈希冲突链）。
pub fn fill_next_row_pointer(segment: &mut RowTableSegment, row_index: usize, next: Option<usize>) {
    segment.set_next_row_address(row_index, next);
}

/// 按元数据计算一行可变列数据占用的字节数。
pub fn calculate_row_data_length(meta: &JoinTableMeta, row: &[Value]) -> usize {
    meta.row_columns_order
        .iter()
        .map(|index| {
            match (
                &meta.column_sizes[meta
                    .row_columns_order
                    .iter()
                    .position(|column| column == index)
                    .unwrap()],
                &row[*index],
            ) {
                (Some(size), _) => *size,
                (None, Value::Bytes(value)) => value.len() + 4,
                (None, Value::Text(value)) => value.len() + 4,
                _ => 1,
            }
        })
        .sum()
}

/// 计算将行长度补齐到 8 字节对齐所需的假填充长度。
pub fn calculate_fake_length(row_length: usize) -> usize {
    (8 - row_length % 8) % 8
}
