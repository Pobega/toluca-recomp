use core::mem::offset_of;

use cranelift_codegen::ir::condcodes::FloatCC;
use cranelift_codegen::ir::types::{F32, F64, I8, I16, I32, I64};
use cranelift_codegen::ir::{InstBuilder, MemFlags, Type, Value};
use iced_x86::{Instruction, Mnemonic, OpKind, Register};
use toluca_core::cpu::{X87_DEFAULT_TAG_WORD, X87State, offsets as cpu_offsets};

use super::LifterContext;

const XMM_BYTES: i32 = 16;
const MM_BYTES: i32 = 8;

impl<'ctx, 'b> LifterContext<'ctx, 'b> {
    pub fn lift_simd(&mut self, instr: &Instruction) -> bool {
        match instr.mnemonic() {
            Mnemonic::Movaps | Mnemonic::Movups | Mnemonic::Movdqa | Mnemonic::Movdqu => {
                self.lift_xmm_move_128(instr);
                true
            }
            Mnemonic::Movss => {
                self.lift_movss(instr);
                true
            }
            Mnemonic::Movsd if has_xmm_operand(instr) => {
                self.lift_movsd_sse(instr);
                true
            }
            Mnemonic::Movlps => {
                self.lift_mov_half_ps(instr, false);
                true
            }
            Mnemonic::Movhps => {
                self.lift_mov_half_ps(instr, true);
                true
            }
            Mnemonic::Movlhps => {
                let dst = xmm_index(instr.op_register(0));
                let src = xmm_index(instr.op_register(1));
                let lo = self.xmm_load_lane(src, I64, 0);
                self.xmm_store_lane(dst, lo, 1);
                true
            }
            Mnemonic::Movhlps => {
                let dst = xmm_index(instr.op_register(0));
                let src = xmm_index(instr.op_register(1));
                let hi = self.xmm_load_lane(src, I64, 1);
                self.xmm_store_lane(dst, hi, 0);
                true
            }
            Mnemonic::Movd => {
                self.lift_movd(instr);
                true
            }
            Mnemonic::Movq => {
                self.lift_movq(instr);
                true
            }
            Mnemonic::Addss => self.lift_scalar_f32(instr, FloatBinOp::Add),
            Mnemonic::Subss => self.lift_scalar_f32(instr, FloatBinOp::Sub),
            Mnemonic::Mulss => self.lift_scalar_f32(instr, FloatBinOp::Mul),
            Mnemonic::Divss => self.lift_scalar_f32(instr, FloatBinOp::Div),
            Mnemonic::Minss => self.lift_scalar_f32(instr, FloatBinOp::Min),
            Mnemonic::Maxss => self.lift_scalar_f32(instr, FloatBinOp::Max),
            Mnemonic::Sqrtss => self.lift_scalar_f32_unary(instr, FloatUnaryOp::Sqrt),
            Mnemonic::Rcpss => self.lift_scalar_f32_unary(instr, FloatUnaryOp::Rcp),
            Mnemonic::Rsqrtss => self.lift_scalar_f32_unary(instr, FloatUnaryOp::Rsqrt),

            Mnemonic::Addsd => self.lift_scalar_f64(instr, FloatBinOp::Add),
            Mnemonic::Subsd => self.lift_scalar_f64(instr, FloatBinOp::Sub),
            Mnemonic::Mulsd => self.lift_scalar_f64(instr, FloatBinOp::Mul),
            Mnemonic::Divsd => self.lift_scalar_f64(instr, FloatBinOp::Div),
            Mnemonic::Minsd => self.lift_scalar_f64(instr, FloatBinOp::Min),
            Mnemonic::Maxsd => self.lift_scalar_f64(instr, FloatBinOp::Max),
            Mnemonic::Sqrtsd => self.lift_scalar_f64_unary(instr),

            Mnemonic::Addps => self.lift_packed_f32(instr, FloatBinOp::Add),
            Mnemonic::Subps => self.lift_packed_f32(instr, FloatBinOp::Sub),
            Mnemonic::Mulps => self.lift_packed_f32(instr, FloatBinOp::Mul),
            Mnemonic::Divps => self.lift_packed_f32(instr, FloatBinOp::Div),
            Mnemonic::Minps => self.lift_packed_f32(instr, FloatBinOp::Min),
            Mnemonic::Maxps => self.lift_packed_f32(instr, FloatBinOp::Max),
            Mnemonic::Sqrtps => self.lift_packed_f32_unary(instr, FloatUnaryOp::Sqrt),
            Mnemonic::Rcpps => self.lift_packed_f32_unary(instr, FloatUnaryOp::Rcp),
            Mnemonic::Rsqrtps => self.lift_packed_f32_unary(instr, FloatUnaryOp::Rsqrt),

            Mnemonic::Andps | Mnemonic::Andpd | Mnemonic::Pand => {
                self.lift_bitwise_simd(instr, SimdBitOp::And);
                true
            }
            Mnemonic::Andnps | Mnemonic::Andnpd | Mnemonic::Pandn => {
                self.lift_bitwise_simd(instr, SimdBitOp::AndNot);
                true
            }
            Mnemonic::Orps | Mnemonic::Orpd | Mnemonic::Por => {
                self.lift_bitwise_simd(instr, SimdBitOp::Or);
                true
            }
            Mnemonic::Xorps | Mnemonic::Xorpd | Mnemonic::Pxor => {
                self.lift_bitwise_simd(instr, SimdBitOp::Xor);
                true
            }
            Mnemonic::Shufps => {
                self.lift_shufps(instr);
                true
            }
            Mnemonic::Unpcklps => {
                self.lift_unpack_ps(instr, false);
                true
            }
            Mnemonic::Unpckhps => {
                self.lift_unpack_ps(instr, true);
                true
            }
            Mnemonic::Comiss | Mnemonic::Ucomiss => {
                let lhs = self.xmm_load_lane(xmm_index(instr.op_register(0)), F32, 0);
                let rhs = self.read_sse_f32_operand(instr, 1);
                self.set_eflags_float_compare(lhs, rhs);
                true
            }
            Mnemonic::Comisd | Mnemonic::Ucomisd => {
                let lhs = self.xmm_load_lane(xmm_index(instr.op_register(0)), F64, 0);
                let rhs = self.read_sse_f64_operand(instr, 1);
                self.set_eflags_float_compare(lhs, rhs);
                true
            }
            Mnemonic::Cvtsi2ss => {
                let dst = xmm_index(instr.op_register(0));
                let int_val = self.read_operand(instr, 1, I32);
                let f = self.builder.ins().fcvt_from_sint(F32, int_val);
                self.xmm_store_lane(dst, f, 0);
                true
            }
            Mnemonic::Cvtss2si => {
                let f = self.read_sse_f32_operand(instr, 1);
                let rounded = self.round_nearest_even_f32(f);
                let int_val = self.builder.ins().fcvt_to_sint_sat(I32, rounded);
                self.write_operand(instr, 0, int_val);
                true
            }
            Mnemonic::Cvttss2si => {
                let f = self.read_sse_f32_operand(instr, 1);
                let int_val = self.builder.ins().fcvt_to_sint_sat(I32, f);
                self.write_operand(instr, 0, int_val);
                true
            }
            Mnemonic::Cvtsi2sd => {
                let dst = xmm_index(instr.op_register(0));
                let int_val = self.read_operand(instr, 1, I32);
                let f = self.builder.ins().fcvt_from_sint(F64, int_val);
                self.xmm_store_lane(dst, f, 0);
                true
            }
            Mnemonic::Cvtsd2si => {
                let f = self.read_sse_f64_operand(instr, 1);
                let rounded = self.round_nearest_even_f64(f);
                let int_val = self.builder.ins().fcvt_to_sint_sat(I32, rounded);
                self.write_operand(instr, 0, int_val);
                true
            }
            Mnemonic::Cvttsd2si => {
                let f = self.read_sse_f64_operand(instr, 1);
                let int_val = self.builder.ins().fcvt_to_sint_sat(I32, f);
                self.write_operand(instr, 0, int_val);
                true
            }
            Mnemonic::Cvtss2sd => {
                let dst = xmm_index(instr.op_register(0));
                let f32_val = self.read_sse_f32_operand(instr, 1);
                let f64_val = self.builder.ins().fpromote(F64, f32_val);
                self.xmm_store_lane(dst, f64_val, 0);
                true
            }
            Mnemonic::Cvtsd2ss => {
                let dst = xmm_index(instr.op_register(0));
                let f64_val = self.read_sse_f64_operand(instr, 1);
                let f32_val = self.builder.ins().fdemote(F32, f64_val);
                self.xmm_store_lane(dst, f32_val, 0);
                true
            }
            Mnemonic::Stmxcsr => {
                let mxcsr = self.builder.ins().load(
                    I32,
                    MemFlags::trusted(),
                    self.cpu_ptr,
                    cpu_offsets::MXCSR as i32,
                );
                self.write_operand(instr, 0, mxcsr);
                true
            }
            Mnemonic::Ldmxcsr => {
                let mxcsr = self.read_operand(instr, 0, I32);
                self.builder.ins().store(
                    MemFlags::trusted(),
                    mxcsr,
                    self.cpu_ptr,
                    cpu_offsets::MXCSR as i32,
                );
                true
            }
            Mnemonic::Paddb => self.lift_packed_int_arith(instr, I8, false),
            Mnemonic::Paddw => self.lift_packed_int_arith(instr, I16, false),
            Mnemonic::Paddd => self.lift_packed_int_arith(instr, I32, false),
            Mnemonic::Psubb => self.lift_packed_int_arith(instr, I8, true),
            Mnemonic::Psubw => self.lift_packed_int_arith(instr, I16, true),
            Mnemonic::Psubd => self.lift_packed_int_arith(instr, I32, true),
            Mnemonic::Emms => {
                let tw = self
                    .builder
                    .ins()
                    .iconst(I16, X87_DEFAULT_TAG_WORD as i64);
                let off = (cpu_offsets::X87 + offset_of!(X87State, tag_word)) as i32;
                self.builder
                    .ins()
                    .store(MemFlags::trusted(), tw, self.cpu_ptr, off);
                true
            }
            _ => false,
        }
    }

    fn lift_xmm_move_128(&mut self, instr: &Instruction) {
        if instr.op_kind(0) == OpKind::Register && instr.op_kind(1) == OpKind::Register {
            let dst = xmm_index(instr.op_register(0));
            let src = xmm_index(instr.op_register(1));
            for half in 0..2 {
                let val = self.xmm_load_lane(src, I64, half);
                self.xmm_store_lane(dst, val, half);
            }
        } else if instr.op_kind(0) == OpKind::Register {
            let dst = xmm_index(instr.op_register(0));
            let addr = self.compute_memory_addr(instr);
            for half in 0..2 {
                let lane_addr = if half == 0 {
                    addr
                } else {
                    self.builder.ins().iadd_imm(addr, 8)
                };
                let val = self.load_guest(lane_addr, I64);
                self.xmm_store_lane(dst, val, half);
            }
        } else {
            let src = xmm_index(instr.op_register(1));
            let addr = self.compute_memory_addr(instr);
            for half in 0..2 {
                let lane_addr = if half == 0 {
                    addr
                } else {
                    self.builder.ins().iadd_imm(addr, 8)
                };
                let val = self.xmm_load_lane(src, I64, half);
                self.store_guest(lane_addr, val);
            }
        }
    }

    fn lift_movss(&mut self, instr: &Instruction) {
        if instr.op_kind(0) == OpKind::Register && instr.op_kind(1) == OpKind::Register {
            let dst = xmm_index(instr.op_register(0));
            let src = xmm_index(instr.op_register(1));
            let val = self.xmm_load_lane(src, F32, 0);
            self.xmm_store_lane(dst, val, 0);
        } else if instr.op_kind(0) == OpKind::Register {
            let dst = xmm_index(instr.op_register(0));
            let addr = self.compute_memory_addr(instr);
            let val = self.load_guest(addr, F32);
            self.xmm_zero(dst);
            self.xmm_store_lane(dst, val, 0);
        } else {
            let src = xmm_index(instr.op_register(1));
            let val = self.xmm_load_lane(src, F32, 0);
            let addr = self.compute_memory_addr(instr);
            self.store_guest(addr, val);
        }
    }

    fn lift_movsd_sse(&mut self, instr: &Instruction) {
        if instr.op_kind(0) == OpKind::Register && instr.op_kind(1) == OpKind::Register {
            let dst = xmm_index(instr.op_register(0));
            let src = xmm_index(instr.op_register(1));
            let val = self.xmm_load_lane(src, F64, 0);
            self.xmm_store_lane(dst, val, 0);
        } else if instr.op_kind(0) == OpKind::Register {
            let dst = xmm_index(instr.op_register(0));
            let addr = self.compute_memory_addr(instr);
            let val = self.load_guest(addr, F64);
            self.xmm_zero(dst);
            self.xmm_store_lane(dst, val, 0);
        } else {
            let src = xmm_index(instr.op_register(1));
            let val = self.xmm_load_lane(src, F64, 0);
            let addr = self.compute_memory_addr(instr);
            self.store_guest(addr, val);
        }
    }

    fn lift_mov_half_ps(&mut self, instr: &Instruction, high_half: bool) {
        let half_idx = usize::from(high_half);
        if instr.op_kind(0) == OpKind::Register {
            let dst = xmm_index(instr.op_register(0));
            let addr = self.compute_memory_addr(instr);
            let val = self.load_guest(addr, I64);
            self.xmm_store_lane(dst, val, half_idx);
        } else {
            let src = xmm_index(instr.op_register(1));
            let val = self.xmm_load_lane(src, I64, half_idx);
            let addr = self.compute_memory_addr(instr);
            self.store_guest(addr, val);
        }
    }

    fn lift_movd(&mut self, instr: &Instruction) {
        let op0 = instr.op_kind(0);
        let op1 = instr.op_kind(1);
        if op0 == OpKind::Register && is_xmm_reg(instr.op_register(0)) {
            let dst = xmm_index(instr.op_register(0));
            let val = self.read_operand(instr, 1, I32);
            self.xmm_zero(dst);
            self.xmm_store_lane(dst, val, 0);
        } else if op1 == OpKind::Register && is_xmm_reg(instr.op_register(1)) {
            let src = xmm_index(instr.op_register(1));
            let val = self.xmm_load_lane(src, I32, 0);
            self.write_operand(instr, 0, val);
        } else if op0 == OpKind::Register && is_mm_reg(instr.op_register(0)) {
            let dst = mm_index(instr.op_register(0));
            let val32 = self.read_operand(instr, 1, I32);
            let val64 = self.builder.ins().uextend(I64, val32);
            self.mm_store_u64(dst, val64);
        } else if op1 == OpKind::Register && is_mm_reg(instr.op_register(1)) {
            let src = mm_index(instr.op_register(1));
            let val64 = self.mm_load_u64(src);
            let val32 = self.builder.ins().ireduce(I32, val64);
            self.write_operand(instr, 0, val32);
        }
    }

    fn lift_movq(&mut self, instr: &Instruction) {
        let op0 = instr.op_kind(0);
        let op1 = instr.op_kind(1);
        if op0 == OpKind::Register && is_xmm_reg(instr.op_register(0)) {
            let dst = xmm_index(instr.op_register(0));
            let val = if op1 == OpKind::Register && is_xmm_reg(instr.op_register(1)) {
                self.xmm_load_lane(xmm_index(instr.op_register(1)), I64, 0)
            } else if op1 == OpKind::Register && is_mm_reg(instr.op_register(1)) {
                self.mm_load_u64(mm_index(instr.op_register(1)))
            } else {
                let addr = self.compute_memory_addr(instr);
                self.load_guest(addr, I64)
            };
            self.xmm_zero(dst);
            self.xmm_store_lane(dst, val, 0);
        } else if op1 == OpKind::Register && is_xmm_reg(instr.op_register(1)) {
            let val = self.xmm_load_lane(xmm_index(instr.op_register(1)), I64, 0);
            if op0 == OpKind::Register && is_mm_reg(instr.op_register(0)) {
                self.mm_store_u64(mm_index(instr.op_register(0)), val);
            } else {
                let addr = self.compute_memory_addr(instr);
                self.store_guest(addr, val);
            }
        } else if op0 == OpKind::Register && is_mm_reg(instr.op_register(0)) {
            let dst = mm_index(instr.op_register(0));
            let val = if op1 == OpKind::Register && is_mm_reg(instr.op_register(1)) {
                self.mm_load_u64(mm_index(instr.op_register(1)))
            } else {
                let addr = self.compute_memory_addr(instr);
                self.load_guest(addr, I64)
            };
            self.mm_store_u64(dst, val);
        } else if op1 == OpKind::Register && is_mm_reg(instr.op_register(1)) {
            let val = self.mm_load_u64(mm_index(instr.op_register(1)));
            let addr = self.compute_memory_addr(instr);
            self.store_guest(addr, val);
        }
    }

    fn lift_scalar_f32(&mut self, instr: &Instruction, op: FloatBinOp) -> bool {
        let dst = xmm_index(instr.op_register(0));
        let lhs = self.xmm_load_lane(dst, F32, 0);
        let rhs = self.read_sse_f32_operand(instr, 1);
        let res = self.emit_float_binop(op, lhs, rhs);
        self.xmm_store_lane(dst, res, 0);
        true
    }

    fn lift_scalar_f32_unary(&mut self, instr: &Instruction, op: FloatUnaryOp) -> bool {
        let dst = xmm_index(instr.op_register(0));
        let src = self.read_sse_f32_operand(instr, 1);
        let res = self.emit_float_unary_f32(op, src);
        self.xmm_store_lane(dst, res, 0);
        true
    }

    fn lift_scalar_f64(&mut self, instr: &Instruction, op: FloatBinOp) -> bool {
        let dst = xmm_index(instr.op_register(0));
        let lhs = self.xmm_load_lane(dst, F64, 0);
        let rhs = self.read_sse_f64_operand(instr, 1);
        let res = self.emit_float_binop(op, lhs, rhs);
        self.xmm_store_lane(dst, res, 0);
        true
    }

    fn lift_scalar_f64_unary(&mut self, instr: &Instruction) -> bool {
        let dst = xmm_index(instr.op_register(0));
        let src = self.read_sse_f64_operand(instr, 1);
        let res = self.builder.ins().sqrt(src);
        self.xmm_store_lane(dst, res, 0);
        true
    }

    fn lift_packed_f32(&mut self, instr: &Instruction, op: FloatBinOp) -> bool {
        let dst = xmm_index(instr.op_register(0));
        let rhs_vals = self.read_xmm_f32x4_operand(instr, 1);
        for (lane, rhs) in rhs_vals.into_iter().enumerate() {
            let lhs = self.xmm_load_lane(dst, F32, lane);
            let res = self.emit_float_binop(op, lhs, rhs);
            self.xmm_store_lane(dst, res, lane);
        }
        true
    }

    fn lift_packed_f32_unary(&mut self, instr: &Instruction, op: FloatUnaryOp) -> bool {
        let dst = xmm_index(instr.op_register(0));
        let src_vals = self.read_xmm_f32x4_operand(instr, 1);
        for (lane, src) in src_vals.into_iter().enumerate() {
            let res = self.emit_float_unary_f32(op, src);
            self.xmm_store_lane(dst, res, lane);
        }
        true
    }

    fn lift_bitwise_simd(&mut self, instr: &Instruction, op: SimdBitOp) {
        if is_mm_reg(instr.op_register(0)) {
            let dst = mm_index(instr.op_register(0));
            let lhs = self.mm_load_u64(dst);
            let rhs = if instr.op_kind(1) == OpKind::Register {
                self.mm_load_u64(mm_index(instr.op_register(1)))
            } else {
                let addr = self.compute_memory_addr(instr);
                self.load_guest(addr, I64)
            };
            let res = self.emit_bit_op(op, lhs, rhs);
            self.mm_store_u64(dst, res);
            return;
        }

        let dst = xmm_index(instr.op_register(0));
        let rhs_halves = if instr.op_kind(1) == OpKind::Register {
            let src = xmm_index(instr.op_register(1));
            [
                self.xmm_load_lane(src, I64, 0),
                self.xmm_load_lane(src, I64, 1),
            ]
        } else {
            let addr = self.compute_memory_addr(instr);
            let addr_hi = self.builder.ins().iadd_imm(addr, 8);
            [self.load_guest(addr, I64), self.load_guest(addr_hi, I64)]
        };

        for (half, rhs) in rhs_halves.into_iter().enumerate() {
            let lhs = self.xmm_load_lane(dst, I64, half);
            let res = self.emit_bit_op(op, lhs, rhs);
            self.xmm_store_lane(dst, res, half);
        }
    }

    fn lift_shufps(&mut self, instr: &Instruction) {
        let dst = xmm_index(instr.op_register(0));
        let imm = instr.immediate8();
        let dst_lanes = [
            self.xmm_load_lane(dst, I32, 0),
            self.xmm_load_lane(dst, I32, 1),
            self.xmm_load_lane(dst, I32, 2),
            self.xmm_load_lane(dst, I32, 3),
        ];
        let src_lanes = self.read_xmm_i32x4_operand(instr, 1);

        let out = [
            dst_lanes[(imm & 0x3) as usize],
            dst_lanes[((imm >> 2) & 0x3) as usize],
            src_lanes[((imm >> 4) & 0x3) as usize],
            src_lanes[((imm >> 6) & 0x3) as usize],
        ];
        for (lane, val) in out.into_iter().enumerate() {
            self.xmm_store_lane(dst, val, lane);
        }
    }

    fn lift_unpack_ps(&mut self, instr: &Instruction, high: bool) {
        let dst = xmm_index(instr.op_register(0));
        let dst_lanes = [
            self.xmm_load_lane(dst, I32, 0),
            self.xmm_load_lane(dst, I32, 1),
            self.xmm_load_lane(dst, I32, 2),
            self.xmm_load_lane(dst, I32, 3),
        ];
        let src_lanes = self.read_xmm_i32x4_operand(instr, 1);
        let base = if high { 2 } else { 0 };
        let out = [
            dst_lanes[base],
            src_lanes[base],
            dst_lanes[base + 1],
            src_lanes[base + 1],
        ];
        for (lane, val) in out.into_iter().enumerate() {
            self.xmm_store_lane(dst, val, lane);
        }
    }

    fn lift_packed_int_arith(&mut self, instr: &Instruction, lane_ty: Type, is_sub: bool) -> bool {
        let lane_count = if is_mm_reg(instr.op_register(0)) {
            MM_BYTES as usize / lane_ty.bytes() as usize
        } else {
            XMM_BYTES as usize / lane_ty.bytes() as usize
        };

        if is_mm_reg(instr.op_register(0)) {
            let dst = mm_index(instr.op_register(0));
            let mem_addr = if instr.op_kind(1) == OpKind::Memory {
                Some(self.compute_memory_addr(instr))
            } else {
                None
            };
            let src_mm = (instr.op_kind(1) == OpKind::Register).then(|| mm_index(instr.op_register(1)));
            for lane in 0..lane_count {
                let lhs = self.mm_load_lane(dst, lane_ty, lane);
                let rhs = if let Some(src) = src_mm {
                    self.mm_load_lane(src, lane_ty, lane)
                } else {
                    let base = mem_addr.unwrap();
                    let off = (lane * lane_ty.bytes() as usize) as i64;
                    let addr = if off == 0 {
                        base
                    } else {
                        self.builder.ins().iadd_imm(base, off)
                    };
                    self.load_guest(addr, lane_ty)
                };
                let res = if is_sub {
                    self.builder.ins().isub(lhs, rhs)
                } else {
                    self.builder.ins().iadd(lhs, rhs)
                };
                self.mm_store_lane(dst, res, lane);
            }
            return true;
        }

        let dst = xmm_index(instr.op_register(0));
        let mem_addr = if instr.op_kind(1) == OpKind::Memory {
            Some(self.compute_memory_addr(instr))
        } else {
            None
        };
        let src_xmm = (instr.op_kind(1) == OpKind::Register).then(|| xmm_index(instr.op_register(1)));
        for lane in 0..lane_count {
            let lhs = self.xmm_load_lane(dst, lane_ty, lane);
            let rhs = if let Some(src) = src_xmm {
                self.xmm_load_lane(src, lane_ty, lane)
            } else {
                let base = mem_addr.unwrap();
                let off = (lane * lane_ty.bytes() as usize) as i64;
                let addr = if off == 0 {
                    base
                } else {
                    self.builder.ins().iadd_imm(base, off)
                };
                self.load_guest(addr, lane_ty)
            };
            let res = if is_sub {
                self.builder.ins().isub(lhs, rhs)
            } else {
                self.builder.ins().iadd(lhs, rhs)
            };
            self.xmm_store_lane(dst, res, lane);
        }
        true
    }

    fn emit_float_binop(&mut self, op: FloatBinOp, lhs: Value, rhs: Value) -> Value {
        match op {
            FloatBinOp::Add => self.builder.ins().fadd(lhs, rhs),
            FloatBinOp::Sub => self.builder.ins().fsub(lhs, rhs),
            FloatBinOp::Mul => self.builder.ins().fmul(lhs, rhs),
            FloatBinOp::Div => self.builder.ins().fdiv(lhs, rhs),
            FloatBinOp::Min => {
                let lt = self.builder.ins().fcmp(FloatCC::LessThan, lhs, rhs);
                self.builder.ins().select(lt, lhs, rhs)
            }
            FloatBinOp::Max => {
                let gt = self.builder.ins().fcmp(FloatCC::GreaterThan, lhs, rhs);
                self.builder.ins().select(gt, lhs, rhs)
            }
        }
    }

    fn emit_float_unary_f32(&mut self, op: FloatUnaryOp, src: Value) -> Value {
        match op {
            FloatUnaryOp::Sqrt => self.builder.ins().sqrt(src),
            FloatUnaryOp::Rcp => {
                let one = self.builder.ins().f32const(1.0);
                self.builder.ins().fdiv(one, src)
            }
            FloatUnaryOp::Rsqrt => {
                let root = self.builder.ins().sqrt(src);
                let one = self.builder.ins().f32const(1.0);
                self.builder.ins().fdiv(one, root)
            }
        }
    }

    fn emit_bit_op(&mut self, op: SimdBitOp, lhs: Value, rhs: Value) -> Value {
        match op {
            SimdBitOp::And => self.builder.ins().band(lhs, rhs),
            SimdBitOp::AndNot => {
                let inv = self.builder.ins().bnot(lhs);
                self.builder.ins().band(inv, rhs)
            }
            SimdBitOp::Or => self.builder.ins().bor(lhs, rhs),
            SimdBitOp::Xor => self.builder.ins().bxor(lhs, rhs),
        }
    }

    fn read_sse_f32_operand(&mut self, instr: &Instruction, op_idx: u32) -> Value {
        if instr.op_kind(op_idx) == OpKind::Register {
            self.xmm_load_lane(xmm_index(instr.op_register(op_idx)), F32, 0)
        } else {
            let addr = self.compute_memory_addr(instr);
            self.load_guest(addr, F32)
        }
    }

    fn read_sse_f64_operand(&mut self, instr: &Instruction, op_idx: u32) -> Value {
        if instr.op_kind(op_idx) == OpKind::Register {
            self.xmm_load_lane(xmm_index(instr.op_register(op_idx)), F64, 0)
        } else {
            let addr = self.compute_memory_addr(instr);
            self.load_guest(addr, F64)
        }
    }

    fn read_xmm_f32x4_operand(&mut self, instr: &Instruction, op_idx: u32) -> [Value; 4] {
        if instr.op_kind(op_idx) == OpKind::Register {
            let reg = xmm_index(instr.op_register(op_idx));
            [
                self.xmm_load_lane(reg, F32, 0),
                self.xmm_load_lane(reg, F32, 1),
                self.xmm_load_lane(reg, F32, 2),
                self.xmm_load_lane(reg, F32, 3),
            ]
        } else {
            let base = self.compute_memory_addr(instr);
            let mut out = [Value::from_u32(0); 4];
            for (lane, slot) in out.iter_mut().enumerate() {
                let addr = if lane == 0 {
                    base
                } else {
                    self.builder.ins().iadd_imm(base, (lane * 4) as i64)
                };
                *slot = self.load_guest(addr, F32);
            }
            out
        }
    }

    fn read_xmm_i32x4_operand(&mut self, instr: &Instruction, op_idx: u32) -> [Value; 4] {
        if instr.op_kind(op_idx) == OpKind::Register {
            let reg = xmm_index(instr.op_register(op_idx));
            [
                self.xmm_load_lane(reg, I32, 0),
                self.xmm_load_lane(reg, I32, 1),
                self.xmm_load_lane(reg, I32, 2),
                self.xmm_load_lane(reg, I32, 3),
            ]
        } else {
            let base = self.compute_memory_addr(instr);
            let mut out = [Value::from_u32(0); 4];
            for (lane, slot) in out.iter_mut().enumerate() {
                let addr = if lane == 0 {
                    base
                } else {
                    self.builder.ins().iadd_imm(base, (lane * 4) as i64)
                };
                *slot = self.load_guest(addr, I32);
            }
            out
        }
    }

    fn xmm_offset(reg_idx: usize, lane_bytes: usize, lane: usize) -> i32 {
        cpu_offsets::XMM as i32 + (reg_idx as i32) * XMM_BYTES + (lane * lane_bytes) as i32
    }

    fn xmm_load_lane(&mut self, reg_idx: usize, ty: Type, lane: usize) -> Value {
        let off = Self::xmm_offset(reg_idx, ty.bytes() as usize, lane);
        self.builder
            .ins()
            .load(ty, MemFlags::trusted(), self.cpu_ptr, off)
    }

    fn xmm_store_lane(&mut self, reg_idx: usize, val: Value, lane: usize) {
        let ty = self.builder.func.dfg.value_type(val);
        let off = Self::xmm_offset(reg_idx, ty.bytes() as usize, lane);
        self.builder
            .ins()
            .store(MemFlags::trusted(), val, self.cpu_ptr, off);
    }

    fn xmm_zero(&mut self, reg_idx: usize) {
        let zero = self.builder.ins().iconst(I64, 0);
        self.xmm_store_lane(reg_idx, zero, 0);
        self.xmm_store_lane(reg_idx, zero, 1);
    }

    fn mm_offset(reg_idx: usize, lane_bytes: usize, lane: usize) -> i32 {
        cpu_offsets::MM as i32 + (reg_idx as i32) * MM_BYTES + (lane * lane_bytes) as i32
    }

    fn mm_load_u64(&mut self, reg_idx: usize) -> Value {
        let off = Self::mm_offset(reg_idx, 8, 0);
        self.builder
            .ins()
            .load(I64, MemFlags::trusted(), self.cpu_ptr, off)
    }

    fn mm_store_u64(&mut self, reg_idx: usize, val: Value) {
        let off = Self::mm_offset(reg_idx, 8, 0);
        self.builder
            .ins()
            .store(MemFlags::trusted(), val, self.cpu_ptr, off);
    }

    fn mm_load_lane(&mut self, reg_idx: usize, ty: Type, lane: usize) -> Value {
        let off = Self::mm_offset(reg_idx, ty.bytes() as usize, lane);
        self.builder
            .ins()
            .load(ty, MemFlags::trusted(), self.cpu_ptr, off)
    }

    fn mm_store_lane(&mut self, reg_idx: usize, val: Value, lane: usize) {
        let ty = self.builder.func.dfg.value_type(val);
        let off = Self::mm_offset(reg_idx, ty.bytes() as usize, lane);
        self.builder
            .ins()
            .store(MemFlags::trusted(), val, self.cpu_ptr, off);
    }
}

#[derive(Copy, Clone)]
enum FloatBinOp {
    Add,
    Sub,
    Mul,
    Div,
    Min,
    Max,
}

#[derive(Copy, Clone)]
enum FloatUnaryOp {
    Sqrt,
    Rcp,
    Rsqrt,
}

#[derive(Copy, Clone)]
enum SimdBitOp {
    And,
    AndNot,
    Or,
    Xor,
}

const fn is_xmm_reg(reg: Register) -> bool {
    matches!(
        reg,
        Register::XMM0
            | Register::XMM1
            | Register::XMM2
            | Register::XMM3
            | Register::XMM4
            | Register::XMM5
            | Register::XMM6
            | Register::XMM7
    )
}

const fn is_mm_reg(reg: Register) -> bool {
    matches!(
        reg,
        Register::MM0
            | Register::MM1
            | Register::MM2
            | Register::MM3
            | Register::MM4
            | Register::MM5
            | Register::MM6
            | Register::MM7
    )
}

fn has_xmm_operand(instr: &Instruction) -> bool {
    (instr.op_count() > 0
        && instr.op_kind(0) == OpKind::Register
        && is_xmm_reg(instr.op_register(0)))
        || (instr.op_count() > 1
            && instr.op_kind(1) == OpKind::Register
            && is_xmm_reg(instr.op_register(1)))
}

const fn xmm_index(reg: Register) -> usize {
    match reg {
        Register::XMM0 => 0,
        Register::XMM1 => 1,
        Register::XMM2 => 2,
        Register::XMM3 => 3,
        Register::XMM4 => 4,
        Register::XMM5 => 5,
        Register::XMM6 => 6,
        Register::XMM7 => 7,
        _ => 0,
    }
}

const fn mm_index(reg: Register) -> usize {
    match reg {
        Register::MM0 => 0,
        Register::MM1 => 1,
        Register::MM2 => 2,
        Register::MM3 => 3,
        Register::MM4 => 4,
        Register::MM5 => 5,
        Register::MM6 => 6,
        Register::MM7 => 7,
        _ => 0,
    }
}
