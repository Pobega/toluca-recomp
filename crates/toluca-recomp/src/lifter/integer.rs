use core::mem::offset_of;

use cranelift_codegen::ir::condcodes::IntCC;
use cranelift_codegen::ir::types::{I8, I16, I32, I64};
use cranelift_codegen::ir::{InstBuilder, Type, Value};
use iced_x86::{Instruction, Mnemonic, OpKind, Register};
use toluca_core::cpu::{
    EFLAGS_AF, EFLAGS_ARITH_MASK, EFLAGS_CF, EFLAGS_FIXED_ONE, EFLAGS_OF, EFLAGS_PF, EFLAGS_SF,
    EFLAGS_ZF, Gpr, LazyFlagOp, LazyFlags, offsets as cpu_offsets,
};

use super::LifterContext;
use super::flags::condition_from_mnemonic;

const SHIFT_COUNT_MASK: i64 = 0x1F;
const AF_BIT_SHIFT: i64 = 4;
const LOW_BYTE_MASK: i64 = 0xFF;

impl<'ctx, 'b> LifterContext<'ctx, 'b> {
    pub fn lift_integer_instruction(&mut self, instr: &Instruction) -> bool {
        match instr.mnemonic() {
            Mnemonic::Mov => {
                let ty = self.operand_type(instr, 0);
                let val = self.read_operand(instr, 1, ty);
                self.write_operand(instr, 0, val);
                true
            }
            Mnemonic::Movzx => {
                let dst_ty = self.operand_type(instr, 0);
                let src_ty = self.operand_type(instr, 1);
                let src = self.read_operand(instr, 1, src_ty);
                let ext = self.builder.ins().uextend(dst_ty, src);
                self.write_operand(instr, 0, ext);
                true
            }
            Mnemonic::Movsx => {
                let dst_ty = self.operand_type(instr, 0);
                let src_ty = self.operand_type(instr, 1);
                let src = self.read_operand(instr, 1, src_ty);
                let ext = self.builder.ins().sextend(dst_ty, src);
                self.write_operand(instr, 0, ext);
                true
            }
            Mnemonic::Lea => {
                let dst_ty = self.operand_type(instr, 0);
                let addr = self.compute_memory_addr(instr);
                let val = self.coerce_int(addr, dst_ty);
                self.write_operand(instr, 0, val);
                true
            }
            Mnemonic::Xchg => {
                let ty = self.operand_type(instr, 0);
                let a = self.read_operand(instr, 0, ty);
                let b = self.read_operand(instr, 1, ty);
                self.write_operand(instr, 0, b);
                self.write_operand(instr, 1, a);
                true
            }
            Mnemonic::Push => {
                let ty = self.operand_type(instr, 0);
                if ty == I16 {
                    let val = self.read_operand(instr, 0, I16);
                    self.push16(val);
                } else {
                    let val = self.read_operand(instr, 0, I32);
                    self.push32(val);
                }
                true
            }
            Mnemonic::Pop => {
                let ty = self.operand_type(instr, 0);
                if ty == I16 {
                    let val = self.pop16();
                    self.write_operand(instr, 0, val);
                } else {
                    let val = self.pop32();
                    self.write_operand(instr, 0, val);
                }
                true
            }
            Mnemonic::Pushad => {
                let orig_esp = self.read_gpr(Gpr::Esp);
                for gpr in [
                    Gpr::Eax,
                    Gpr::Ecx,
                    Gpr::Edx,
                    Gpr::Ebx,
                    Gpr::Esp,
                    Gpr::Ebp,
                    Gpr::Esi,
                    Gpr::Edi,
                ] {
                    let val = if gpr == Gpr::Esp {
                        orig_esp
                    } else {
                        self.read_gpr(gpr)
                    };
                    self.push32(val);
                }
                true
            }
            Mnemonic::Popad => {
                for gpr in [Gpr::Edi, Gpr::Esi, Gpr::Ebp] {
                    let val = self.pop32();
                    self.write_gpr(gpr, val);
                }
                let _discarded_esp = self.pop32();
                for gpr in [Gpr::Ebx, Gpr::Edx, Gpr::Ecx, Gpr::Eax] {
                    let val = self.pop32();
                    self.write_gpr(gpr, val);
                }
                true
            }
            Mnemonic::Bswap => {
                let val = self.read_operand(instr, 0, I32);
                let swapped = self.builder.ins().bswap(val);
                self.write_operand(instr, 0, swapped);
                true
            }
            Mnemonic::Cbw => {
                let al = self.read_reg(Register::AL);
                let ax = self.builder.ins().sextend(I16, al);
                self.write_reg(Register::AX, ax);
                true
            }
            Mnemonic::Cwde => {
                let ax = self.read_reg(Register::AX);
                let eax = self.builder.ins().sextend(I32, ax);
                self.write_reg(Register::EAX, eax);
                true
            }
            Mnemonic::Cwd => {
                let ax = self.read_reg(Register::AX);
                let ext = self.builder.ins().sextend(I32, ax);
                let high = self.builder.ins().ushr_imm(ext, 16);
                let dx = self.builder.ins().ireduce(I16, high);
                self.write_reg(Register::DX, dx);
                true
            }
            Mnemonic::Cdq => {
                let eax = self.read_reg(Register::EAX);
                let ext = self.builder.ins().sextend(I64, eax);
                let high = self.builder.ins().ushr_imm(ext, 32);
                let edx = self.builder.ins().ireduce(I32, high);
                self.write_reg(Register::EDX, edx);
                true
            }
            Mnemonic::Xadd => {
                let ty = self.operand_type(instr, 0);
                let a = self.read_operand(instr, 0, ty);
                let b = self.read_operand(instr, 1, ty);
                let sum = self.builder.ins().iadd(a, b);
                self.write_operand(instr, 1, a);
                self.write_operand(instr, 0, sum);
                self.set_lazy_flags(LazyFlagOp::Add, ty.bytes(), a, b, sum);
                true
            }
            Mnemonic::Cmpxchg => {
                let ty = self.operand_type(instr, 0);
                let acc_reg = accumulator_for_type(ty);
                let acc = self.read_reg(acc_reg);
                let dst = self.read_operand(instr, 0, ty);
                let src = self.read_operand(instr, 1, ty);
                let diff = self.builder.ins().isub(acc, dst);
                self.set_lazy_flags(LazyFlagOp::Sub, ty.bytes(), acc, dst, diff);

                let is_eq = self.builder.ins().icmp(IntCC::Equal, acc, dst);
                let new_dst = self.builder.ins().select(is_eq, src, dst);
                let new_acc = self.builder.ins().select(is_eq, acc, dst);
                self.write_operand(instr, 0, new_dst);
                self.write_reg(acc_reg, new_acc);
                true
            }
            Mnemonic::Leave => {
                let ebp = self.read_gpr(Gpr::Ebp);
                self.write_gpr(Gpr::Esp, ebp);
                let restored = self.pop32();
                self.write_gpr(Gpr::Ebp, restored);
                true
            }
            Mnemonic::Add => {
                let ty = self.operand_type(instr, 0);
                let lhs = self.read_operand(instr, 0, ty);
                let rhs = self.read_operand(instr, 1, ty);
                let res = self.builder.ins().iadd(lhs, rhs);
                self.write_operand(instr, 0, res);
                self.set_lazy_flags(LazyFlagOp::Add, ty.bytes(), lhs, rhs, res);
                true
            }
            Mnemonic::Adc => {
                self.lift_adc_sbb(instr, false);
                true
            }
            Mnemonic::Sub => {
                let ty = self.operand_type(instr, 0);
                let lhs = self.read_operand(instr, 0, ty);
                let rhs = self.read_operand(instr, 1, ty);
                let res = self.builder.ins().isub(lhs, rhs);
                self.write_operand(instr, 0, res);
                self.set_lazy_flags(LazyFlagOp::Sub, ty.bytes(), lhs, rhs, res);
                true
            }
            Mnemonic::Sbb => {
                self.lift_adc_sbb(instr, true);
                true
            }
            Mnemonic::Cmp => {
                let ty = self.operand_type(instr, 0);
                let lhs = self.read_operand(instr, 0, ty);
                let rhs = self.read_operand(instr, 1, ty);
                let res = self.builder.ins().isub(lhs, rhs);
                self.set_lazy_flags(LazyFlagOp::Sub, ty.bytes(), lhs, rhs, res);
                true
            }
            Mnemonic::Inc => {
                if self.current_cf_live_after {
                    self.materialize_flags();
                }
                let ty = self.operand_type(instr, 0);
                let lhs = self.read_operand(instr, 0, ty);
                let one = self.builder.ins().iconst(ty, 1);
                let res = self.builder.ins().iadd(lhs, one);
                self.write_operand(instr, 0, res);
                self.set_lazy_flags(LazyFlagOp::Inc, ty.bytes(), lhs, one, res);
                true
            }
            Mnemonic::Dec => {
                if self.current_cf_live_after {
                    self.materialize_flags();
                }
                let ty = self.operand_type(instr, 0);
                let lhs = self.read_operand(instr, 0, ty);
                let one = self.builder.ins().iconst(ty, 1);
                let res = self.builder.ins().isub(lhs, one);
                self.write_operand(instr, 0, res);
                self.set_lazy_flags(LazyFlagOp::Dec, ty.bytes(), lhs, one, res);
                true
            }
            Mnemonic::Neg => {
                let ty = self.operand_type(instr, 0);
                let lhs = self.read_operand(instr, 0, ty);
                let zero = self.builder.ins().iconst(ty, 0);
                let res = self.builder.ins().isub(zero, lhs);
                self.write_operand(instr, 0, res);
                self.set_lazy_flags(LazyFlagOp::Sub, ty.bytes(), zero, lhs, res);
                true
            }
            Mnemonic::Mul => {
                self.lift_mul(instr);
                true
            }
            Mnemonic::Imul => {
                self.lift_imul(instr);
                true
            }
            Mnemonic::Div => {
                self.lift_div(instr, false);
                true
            }
            Mnemonic::Idiv => {
                self.lift_div(instr, true);
                true
            }
            Mnemonic::And => {
                let ty = self.operand_type(instr, 0);
                let lhs = self.read_operand(instr, 0, ty);
                let rhs = self.read_operand(instr, 1, ty);
                let res = self.builder.ins().band(lhs, rhs);
                self.write_operand(instr, 0, res);
                self.set_lazy_flags(LazyFlagOp::Logic, ty.bytes(), lhs, rhs, res);
                true
            }
            Mnemonic::Or => {
                let ty = self.operand_type(instr, 0);
                let lhs = self.read_operand(instr, 0, ty);
                let rhs = self.read_operand(instr, 1, ty);
                let res = self.builder.ins().bor(lhs, rhs);
                self.write_operand(instr, 0, res);
                self.set_lazy_flags(LazyFlagOp::Logic, ty.bytes(), lhs, rhs, res);
                true
            }
            Mnemonic::Xor => {
                let ty = self.operand_type(instr, 0);
                let lhs = self.read_operand(instr, 0, ty);
                let rhs = self.read_operand(instr, 1, ty);
                let res = self.builder.ins().bxor(lhs, rhs);
                self.write_operand(instr, 0, res);
                self.set_lazy_flags(LazyFlagOp::Logic, ty.bytes(), lhs, rhs, res);
                true
            }
            Mnemonic::Test => {
                let ty = self.operand_type(instr, 0);
                let lhs = self.read_operand(instr, 0, ty);
                let rhs = self.read_operand(instr, 1, ty);
                let res = self.builder.ins().band(lhs, rhs);
                self.set_lazy_flags(LazyFlagOp::Logic, ty.bytes(), lhs, rhs, res);
                true
            }
            Mnemonic::Not => {
                let ty = self.operand_type(instr, 0);
                let val = self.read_operand(instr, 0, ty);
                let res = self.builder.ins().bnot(val);
                self.write_operand(instr, 0, res);
                true
            }
            Mnemonic::Shl | Mnemonic::Sal => {
                self.lift_shift(instr, LazyFlagOp::Shl);
                true
            }
            Mnemonic::Shr => {
                self.lift_shift(instr, LazyFlagOp::Shr);
                true
            }
            Mnemonic::Sar => {
                self.lift_shift(instr, LazyFlagOp::Sar);
                true
            }
            Mnemonic::Rol => {
                self.lift_rotate(instr, true);
                true
            }
            Mnemonic::Ror => {
                self.lift_rotate(instr, false);
                true
            }
            Mnemonic::Rcl => {
                self.lift_rotate_carry(instr, true);
                true
            }
            Mnemonic::Rcr => {
                self.lift_rotate_carry(instr, false);
                true
            }
            Mnemonic::Shld => {
                self.lift_double_shift(instr, true);
                true
            }
            Mnemonic::Shrd => {
                self.lift_double_shift(instr, false);
                true
            }
            Mnemonic::Bt => {
                self.lift_bit_test(instr, BitTestAction::TestOnly);
                true
            }
            Mnemonic::Bts => {
                self.lift_bit_test(instr, BitTestAction::Set);
                true
            }
            Mnemonic::Btr => {
                self.lift_bit_test(instr, BitTestAction::Reset);
                true
            }
            Mnemonic::Btc => {
                self.lift_bit_test(instr, BitTestAction::Complement);
                true
            }
            Mnemonic::Bsf => {
                self.lift_bit_scan(instr, true);
                true
            }
            Mnemonic::Bsr => {
                self.lift_bit_scan(instr, false);
                true
            }
            mnemonic => {
                let Some(cond) = condition_from_mnemonic(mnemonic) else {
                    return false;
                };
                if is_setcc_mnemonic(mnemonic) {
                    let bit = self.eval_condition(cond);
                    self.write_operand(instr, 0, bit);
                    return true;
                }
                if is_cmovcc_mnemonic(mnemonic) {
                    let ty = self.operand_type(instr, 0);
                    let bit = self.eval_condition(cond);
                    let src = self.read_operand(instr, 1, ty);
                    let dst = self.read_operand(instr, 0, ty);
                    let res = self.builder.ins().select(bit, src, dst);
                    self.write_operand(instr, 0, res);
                    return true;
                }
                false
            }
        }
    }

    fn lift_adc_sbb(&mut self, instr: &Instruction, is_sub: bool) {
        self.materialize_flags();
        let eflags = self.read_eflags();
        let cf_in = self.builder.ins().band_imm(eflags, EFLAGS_CF as i64);

        let ty = self.operand_type(instr, 0);
        let bits = ty.bits() as i64;
        let lhs = self.read_operand(instr, 0, ty);
        let rhs = self.read_operand(instr, 1, ty);

        let lhs64 = self.builder.ins().uextend(I64, lhs);
        let rhs64 = self.builder.ins().uextend(I64, rhs);
        let cf64 = self.builder.ins().uextend(I64, cf_in);

        let full = if is_sub {
            let tmp = self.builder.ins().isub(lhs64, rhs64);
            self.builder.ins().isub(tmp, cf64)
        } else {
            let tmp = self.builder.ins().iadd(lhs64, rhs64);
            self.builder.ins().iadd(tmp, cf64)
        };
        let res = self.coerce_int(full, ty);
        self.write_operand(instr, 0, res);

        let lhs32 = self.ensure_u32(lhs);
        let rhs32 = self.ensure_u32(rhs);
        let res32 = self.ensure_u32(res);

        let cf_shifted = self.builder.ins().ushr_imm(full, bits);
        let cf_bit = self.builder.ins().band_imm(cf_shifted, 1);
        let cf_out = self.builder.ins().ireduce(I32, cf_bit);

        let xor_lr = self.builder.ins().bxor(lhs32, rhs32);
        let xor_res = self.builder.ins().bxor(lhs32, res32);
        let of_raw = if is_sub {
            self.builder.ins().band(xor_lr, xor_res)
        } else {
            let same_sign = self.builder.ins().bnot(xor_lr);
            self.builder.ins().band(same_sign, xor_res)
        };
        let of_shifted = self.builder.ins().ushr_imm(of_raw, bits - 1);
        let of_out = self.builder.ins().band_imm(of_shifted, 1);

        let af_xor = self.builder.ins().bxor(xor_lr, res32);
        let af_shifted = self.builder.ins().ushr_imm(af_xor, AF_BIT_SHIFT);
        let af_out = self.builder.ins().band_imm(af_shifted, 1);

        self.write_materialized_arith_flags(ty, res32, cf_out, af_out, of_out);
        self.clear_lazy_flags();
    }

    fn write_materialized_arith_flags(
        &mut self,
        ty: Type,
        res32: Value,
        cf_bit: Value,
        af_bit: Value,
        of_bit: Value,
    ) {
        let bits = ty.bits() as i64;
        let low_byte = self.builder.ins().band_imm(res32, LOW_BYTE_MASK);
        let ones = self.builder.ins().popcnt(low_byte);
        let odd_parity = self.builder.ins().band_imm(ones, 1);
        let pf_bit = self.builder.ins().bxor_imm(odd_parity, 1);

        let zf_cmp = self.builder.ins().icmp_imm(IntCC::Equal, res32, 0);
        let zf_bit = self.builder.ins().uextend(I32, zf_cmp);

        let sf_shifted = self.builder.ins().ushr_imm(res32, bits - 1);
        let sf_bit = self.builder.ins().band_imm(sf_shifted, 1);

        let mut flags = self.read_eflags();
        flags = self
            .builder
            .ins()
            .band_imm(flags, (!EFLAGS_ARITH_MASK) as i64);
        flags = self.builder.ins().bor_imm(flags, EFLAGS_FIXED_ONE as i64);

        flags = self.set_flag_bit(flags, cf_bit, EFLAGS_CF);
        flags = self.set_flag_bit(flags, pf_bit, EFLAGS_PF);
        flags = self.set_flag_bit(flags, af_bit, EFLAGS_AF);
        flags = self.set_flag_bit(flags, zf_bit, EFLAGS_ZF);
        flags = self.set_flag_bit(flags, sf_bit, EFLAGS_SF);
        flags = self.set_flag_bit(flags, of_bit, EFLAGS_OF);
        self.write_eflags(flags);
    }

    fn set_flag_bit(&mut self, flags: Value, bit_01: Value, mask: u32) -> Value {
        let shift = mask.trailing_zeros() as i64;
        let shifted = if shift == 0 {
            bit_01
        } else {
            self.builder.ins().ishl_imm(bit_01, shift)
        };
        self.builder.ins().bor(flags, shifted)
    }

    fn update_cf_of_flags(&mut self, cf_bool: Value, of_bool: Value) {
        self.materialize_flags();
        let mut flags = self.read_eflags();
        flags = self
            .builder
            .ins()
            .band_imm(flags, (!(EFLAGS_CF | EFLAGS_OF)) as i64);
        let cf32 = self.builder.ins().uextend(I32, cf_bool);
        let of32 = self.builder.ins().uextend(I32, of_bool);
        flags = self.set_flag_bit(flags, cf32, EFLAGS_CF);
        flags = self.set_flag_bit(flags, of32, EFLAGS_OF);
        self.write_eflags(flags);
    }

    fn lift_mul(&mut self, instr: &Instruction) {
        let ty = self.operand_type(instr, 0);
        let src = self.read_operand(instr, 0, ty);
        let overflow = match ty {
            I8 => {
                let al = self.read_reg(Register::AL);
                let al16 = self.builder.ins().uextend(I16, al);
                let src16 = self.builder.ins().uextend(I16, src);
                let prod = self.builder.ins().imul(al16, src16);
                self.write_reg(Register::AX, prod);
                if self.current_arith_flags_live_after {
                    let high = self.builder.ins().ushr_imm(prod, 8);
                    Some(self.builder.ins().icmp_imm(IntCC::NotEqual, high, 0))
                } else {
                    None
                }
            }
            I16 => {
                let ax = self.read_reg(Register::AX);
                let ax32 = self.builder.ins().uextend(I32, ax);
                let src32 = self.builder.ins().uextend(I32, src);
                let prod = self.builder.ins().imul(ax32, src32);
                let low = self.builder.ins().ireduce(I16, prod);
                let high = self.builder.ins().ushr_imm(prod, 16);
                let dx = self.builder.ins().ireduce(I16, high);
                self.write_reg(Register::AX, low);
                self.write_reg(Register::DX, dx);
                if self.current_arith_flags_live_after {
                    Some(self.builder.ins().icmp_imm(IntCC::NotEqual, high, 0))
                } else {
                    None
                }
            }
            _ => {
                let eax = self.read_reg(Register::EAX);
                let eax64 = self.builder.ins().uextend(I64, eax);
                let src64 = self.builder.ins().uextend(I64, src);
                let prod = self.builder.ins().imul(eax64, src64);
                let low = self.builder.ins().ireduce(I32, prod);
                let high = self.builder.ins().ushr_imm(prod, 32);
                let edx = self.builder.ins().ireduce(I32, high);
                self.write_reg(Register::EAX, low);
                self.write_reg(Register::EDX, edx);
                if self.current_arith_flags_live_after {
                    Some(self.builder.ins().icmp_imm(IntCC::NotEqual, high, 0))
                } else {
                    None
                }
            }
        };
        if let Some(ov) = overflow {
            self.update_cf_of_flags(ov, ov);
        } else {
            self.pending_flags = None;
        }
    }

    fn lift_imul(&mut self, instr: &Instruction) {
        let op_count = instr.op_count();
        if op_count == 1 {
            let ty = self.operand_type(instr, 0);
            let src = self.read_operand(instr, 0, ty);
            let overflow = match ty {
                I8 => {
                    let al = self.read_reg(Register::AL);
                    let al16 = self.builder.ins().sextend(I16, al);
                    let src16 = self.builder.ins().sextend(I16, src);
                    let prod = self.builder.ins().imul(al16, src16);
                    self.write_reg(Register::AX, prod);
                    if self.current_arith_flags_live_after {
                        let low = self.builder.ins().ireduce(I8, prod);
                        let re_ext = self.builder.ins().sextend(I16, low);
                        Some(self.builder.ins().icmp(IntCC::NotEqual, prod, re_ext))
                    } else {
                        None
                    }
                }
                I16 => {
                    let ax = self.read_reg(Register::AX);
                    let ax32 = self.builder.ins().sextend(I32, ax);
                    let src32 = self.builder.ins().sextend(I32, src);
                    let prod = self.builder.ins().imul(ax32, src32);
                    let low = self.builder.ins().ireduce(I16, prod);
                    let high = self.builder.ins().ushr_imm(prod, 16);
                    let dx = self.builder.ins().ireduce(I16, high);
                    self.write_reg(Register::AX, low);
                    self.write_reg(Register::DX, dx);
                    if self.current_arith_flags_live_after {
                        let re_ext = self.builder.ins().sextend(I32, low);
                        Some(self.builder.ins().icmp(IntCC::NotEqual, prod, re_ext))
                    } else {
                        None
                    }
                }
                _ => {
                    let eax = self.read_reg(Register::EAX);
                    let eax64 = self.builder.ins().sextend(I64, eax);
                    let src64 = self.builder.ins().sextend(I64, src);
                    let prod = self.builder.ins().imul(eax64, src64);
                    let low = self.builder.ins().ireduce(I32, prod);
                    let high = self.builder.ins().ushr_imm(prod, 32);
                    let edx = self.builder.ins().ireduce(I32, high);
                    self.write_reg(Register::EAX, low);
                    self.write_reg(Register::EDX, edx);
                    if self.current_arith_flags_live_after {
                        let re_ext = self.builder.ins().sextend(I64, low);
                        Some(self.builder.ins().icmp(IntCC::NotEqual, prod, re_ext))
                    } else {
                        None
                    }
                }
            };
            if let Some(ov) = overflow {
                self.update_cf_of_flags(ov, ov);
            } else {
                self.pending_flags = None;
            }
            return;
        }

        let ty = self.operand_type(instr, 0);
        let (lhs, rhs) = if op_count == 2 {
            (
                self.read_operand(instr, 0, ty),
                self.read_operand(instr, 1, ty),
            )
        } else {
            (
                self.read_operand(instr, 1, ty),
                self.read_operand(instr, 2, ty),
            )
        };

        if !self.current_arith_flags_live_after {
            let res = self.builder.ins().imul(lhs, rhs);
            self.write_operand(instr, 0, res);
            self.pending_flags = None;
            return;
        }

        let wide_ty = if ty == I16 { I32 } else { I64 };
        let lhs_w = self.builder.ins().sextend(wide_ty, lhs);
        let rhs_w = self.builder.ins().sextend(wide_ty, rhs);
        let prod = self.builder.ins().imul(lhs_w, rhs_w);
        let res = self.builder.ins().ireduce(ty, prod);
        self.write_operand(instr, 0, res);

        let re_ext = self.builder.ins().sextend(wide_ty, res);
        let overflow = self.builder.ins().icmp(IntCC::NotEqual, prod, re_ext);
        self.update_cf_of_flags(overflow, overflow);
    }

    fn lift_div(&mut self, instr: &Instruction, signed: bool) {
        let ty = self.operand_type(instr, 0);
        let divisor = self.read_operand(instr, 0, ty);
        match ty {
            I8 => {
                let dividend = self.read_reg(Register::AX);
                let (quot, rem) = if signed {
                    let div16 = self.builder.ins().sextend(I16, divisor);
                    (
                        self.builder.ins().sdiv(dividend, div16),
                        self.builder.ins().srem(dividend, div16),
                    )
                } else {
                    let div16 = self.builder.ins().uextend(I16, divisor);
                    (
                        self.builder.ins().udiv(dividend, div16),
                        self.builder.ins().urem(dividend, div16),
                    )
                };
                let al = self.builder.ins().ireduce(I8, quot);
                let ah = self.builder.ins().ireduce(I8, rem);
                self.write_reg(Register::AL, al);
                self.write_reg(Register::AH, ah);
            }
            I16 => {
                let ax = self.read_reg(Register::AX);
                let dx = self.read_reg(Register::DX);
                let dx_u32 = self.ensure_u32(dx);
                let ax_u32 = self.ensure_u32(ax);
                let dx32 = self.builder.ins().ishl_imm(dx_u32, 16);
                let dividend = self.builder.ins().bor(dx32, ax_u32);
                let (quot, rem) = if signed {
                    let div32 = self.builder.ins().sextend(I32, divisor);
                    (
                        self.builder.ins().sdiv(dividend, div32),
                        self.builder.ins().srem(dividend, div32),
                    )
                } else {
                    let div32 = self.builder.ins().uextend(I32, divisor);
                    (
                        self.builder.ins().udiv(dividend, div32),
                        self.builder.ins().urem(dividend, div32),
                    )
                };
                let q16 = self.builder.ins().ireduce(I16, quot);
                let r16 = self.builder.ins().ireduce(I16, rem);
                self.write_reg(Register::AX, q16);
                self.write_reg(Register::DX, r16);
            }
            _ => {
                let eax = self.read_reg(Register::EAX);
                let edx = self.read_reg(Register::EDX);
                let edx64 = self.builder.ins().uextend(I64, edx);
                let high = self.builder.ins().ishl_imm(edx64, 32);
                let eax64 = self.builder.ins().uextend(I64, eax);
                let dividend = self.builder.ins().bor(high, eax64);
                let (quot, rem) = if signed {
                    let div64 = self.builder.ins().sextend(I64, divisor);
                    (
                        self.builder.ins().sdiv(dividend, div64),
                        self.builder.ins().srem(dividend, div64),
                    )
                } else {
                    let div64 = self.builder.ins().uextend(I64, divisor);
                    (
                        self.builder.ins().udiv(dividend, div64),
                        self.builder.ins().urem(dividend, div64),
                    )
                };
                let q32 = self.builder.ins().ireduce(I32, quot);
                let r32 = self.builder.ins().ireduce(I32, rem);
                self.write_reg(Register::EAX, q32);
                self.write_reg(Register::EDX, r32);
            }
        }
    }

    fn lift_shift(&mut self, instr: &Instruction, op: LazyFlagOp) {
        let ty = self.operand_type(instr, 0);
        let is_imm_count = matches!(
            instr.op_kind(1),
            OpKind::Immediate8 | OpKind::Immediate8_2nd
        );

        if is_imm_count {
            let imm_count = (self.immediate_shift_value(instr) as i64) & SHIFT_COUNT_MASK;
            if imm_count == 0 {
                return;
            }
            let lhs = self.read_operand(instr, 0, ty);
            let count_val = self.builder.ins().iconst(I32, imm_count);
            let res = self.perform_shift_in_i32(op, ty, lhs, count_val);
            self.write_operand(instr, 0, res);
            self.set_lazy_flags(op, ty.bytes(), lhs, count_val, res);
            return;
        }

        if self.current_arith_flags_live_after {
            // Materialize flags first so shift count == 0 preserves prior flags.
            self.materialize_flags();
        }
        let lhs = self.read_operand(instr, 0, ty);
        let raw_count = self.read_operand(instr, 1, I8);
        let raw_count32 = self.ensure_u32(raw_count);
        let count32 = self.builder.ins().band_imm(raw_count32, SHIFT_COUNT_MASK);
        let res = self.perform_shift_in_i32(op, ty, lhs, count32);
        self.write_operand(instr, 0, res);

        if !self.current_arith_flags_live_after {
            self.pending_flags = None;
            return;
        }

        let is_nz = self.builder.ins().icmp_imm(IntCC::NotEqual, count32, 0);
        let op_active = self.builder.ins().iconst(I32, op as i64);
        let op_none = self.builder.ins().iconst(I32, LazyFlagOp::None as i64);
        let selected_op = self.builder.ins().select(is_nz, op_active, op_none);

        let base = cpu_offsets::LAZY_FLAGS;
        let width_val = self.builder.ins().iconst(I32, ty.bytes() as i64);
        let lhs32 = self.ensure_u32(lhs);
        let res32 = self.ensure_u32(res);
        self.store_lazy_field(base + offset_of!(LazyFlags, op), selected_op);
        self.store_lazy_field(base + offset_of!(LazyFlags, width), width_val);
        self.store_lazy_field(base + offset_of!(LazyFlags, lhs), lhs32);
        self.store_lazy_field(base + offset_of!(LazyFlags, rhs), count32);
        self.store_lazy_field(base + offset_of!(LazyFlags, res), res32);
    }

    fn perform_shift_in_i32(
        &mut self,
        op: LazyFlagOp,
        ty: Type,
        lhs: Value,
        count32: Value,
    ) -> Value {
        let wide_lhs = match op {
            LazyFlagOp::Sar => {
                if ty == I32 {
                    lhs
                } else {
                    self.builder.ins().sextend(I32, lhs)
                }
            }
            _ => self.ensure_u32(lhs),
        };
        let shifted = match op {
            LazyFlagOp::Shl => self.builder.ins().ishl(wide_lhs, count32),
            LazyFlagOp::Shr => self.builder.ins().ushr(wide_lhs, count32),
            LazyFlagOp::Sar => self.builder.ins().sshr(wide_lhs, count32),
            _ => wide_lhs,
        };
        self.coerce_int(shifted, ty)
    }

    fn immediate_shift_value(&self, instr: &Instruction) -> u8 {
        match instr.op_kind(1) {
            OpKind::Immediate8 => instr.immediate8(),
            OpKind::Immediate8_2nd => instr.immediate8_2nd(),
            _ => 0,
        }
    }

    fn lift_rotate(&mut self, instr: &Instruction, left: bool) {
        self.materialize_flags();
        let ty = self.operand_type(instr, 0);
        let bits = ty.bits() as i64;
        let lhs = self.read_operand(instr, 0, ty);
        let raw_count = self.read_operand(instr, 1, I8);
        let raw_count32 = self.ensure_u32(raw_count);
        let count32 = self.builder.ins().band_imm(raw_count32, SHIFT_COUNT_MASK);

        let res = if left {
            self.builder.ins().rotl(lhs, count32)
        } else {
            self.builder.ins().rotr(lhs, count32)
        };
        self.write_operand(instr, 0, res);

        let res32 = self.ensure_u32(res);
        let msb_raw = self.builder.ins().ushr_imm(res32, bits - 1);
        let msb = self.builder.ins().band_imm(msb_raw, 1);
        let cf_bit = if left {
            self.builder.ins().band_imm(res32, 1)
        } else {
            msb
        };
        let of_bit = if left {
            self.builder.ins().bxor(msb, cf_bit)
        } else {
            let next_raw = self.builder.ins().ushr_imm(res32, bits - 2);
            let next_msb = self.builder.ins().band_imm(next_raw, 1);
            self.builder.ins().bxor(msb, next_msb)
        };

        let is_nz = self.builder.ins().icmp_imm(IntCC::NotEqual, count32, 0);
        let is_one = self.builder.ins().icmp_imm(IntCC::Equal, count32, 1);

        let eflags = self.read_eflags();
        let old_cf = self.builder.ins().band_imm(eflags, EFLAGS_CF as i64);
        let old_of_shifted = self.builder.ins().ushr_imm(eflags, 11);
        let old_of = self.builder.ins().band_imm(old_of_shifted, 1);

        let new_cf = self.builder.ins().select(is_nz, cf_bit, old_cf);
        let new_of = self.builder.ins().select(is_one, of_bit, old_of);
        let cleared = self
            .builder
            .ins()
            .band_imm(eflags, (!(EFLAGS_CF | EFLAGS_OF)) as i64);
        let with_cf = self.set_flag_bit(cleared, new_cf, EFLAGS_CF);
        let updated = self.set_flag_bit(with_cf, new_of, EFLAGS_OF);
        self.write_eflags(updated);
    }

    fn lift_rotate_carry(&mut self, instr: &Instruction, left: bool) {
        self.materialize_flags();
        let ty = self.operand_type(instr, 0);
        let bits = ty.bits() as i64;
        let lhs = self.read_operand(instr, 0, ty);
        let raw_count = self.read_operand(instr, 1, I8);
        let raw_count32 = self.ensure_u32(raw_count);
        let count32 = self.builder.ins().band_imm(raw_count32, SHIFT_COUNT_MASK);

        let eflags = self.read_eflags();
        let cf_in = self.builder.ins().band_imm(eflags, EFLAGS_CF as i64);

        let mod_base = self.builder.ins().iconst(I32, bits + 1);
        let eff_count = self.builder.ins().urem(count32, mod_base);
        let eff_count64 = self.builder.ins().uextend(I64, eff_count);

        let lhs64 = self.builder.ins().uextend(I64, lhs);
        let cf64 = self.builder.ins().uextend(I64, cf_in);
        let cf_shifted_in = self.builder.ins().ishl_imm(cf64, bits);
        let packed = self.builder.ins().bor(lhs64, cf_shifted_in);

        let total_bits = self.builder.ins().iconst(I64, bits + 1);
        let rev_count = self.builder.ins().isub(total_bits, eff_count64);
        let rotated = if left {
            let hi = self.builder.ins().ishl(packed, eff_count64);
            let lo = self.builder.ins().ushr(packed, rev_count);
            self.builder.ins().bor(hi, lo)
        } else {
            let lo = self.builder.ins().ushr(packed, eff_count64);
            let hi = self.builder.ins().ishl(packed, rev_count);
            self.builder.ins().bor(lo, hi)
        };

        let is_eff_zero = self.builder.ins().icmp_imm(IntCC::Equal, eff_count, 0);
        let final_packed = self.builder.ins().select(is_eff_zero, packed, rotated);

        let res = self.coerce_int(final_packed, ty);
        self.write_operand(instr, 0, res);

        let cf_shifted = self.builder.ins().ushr_imm(final_packed, bits);
        let cf_masked = self.builder.ins().band_imm(cf_shifted, 1);
        let cf_bit = self.builder.ins().ireduce(I32, cf_masked);
        let res32 = self.ensure_u32(res);
        let msb_raw = self.builder.ins().ushr_imm(res32, bits - 1);
        let msb = self.builder.ins().band_imm(msb_raw, 1);
        let of_bit = if left {
            self.builder.ins().bxor(msb, cf_bit)
        } else {
            let next_raw = self.builder.ins().ushr_imm(res32, bits - 2);
            let next_msb = self.builder.ins().band_imm(next_raw, 1);
            self.builder.ins().bxor(msb, next_msb)
        };

        let is_nz = self.builder.ins().icmp_imm(IntCC::NotEqual, count32, 0);
        let is_one = self.builder.ins().icmp_imm(IntCC::Equal, count32, 1);
        let old_of_shifted = self.builder.ins().ushr_imm(eflags, 11);
        let old_of = self.builder.ins().band_imm(old_of_shifted, 1);
        let new_cf = self.builder.ins().select(is_nz, cf_bit, cf_in);
        let new_of = self.builder.ins().select(is_one, of_bit, old_of);
        let cleared = self
            .builder
            .ins()
            .band_imm(eflags, (!(EFLAGS_CF | EFLAGS_OF)) as i64);
        let with_cf = self.set_flag_bit(cleared, new_cf, EFLAGS_CF);
        let updated = self.set_flag_bit(with_cf, new_of, EFLAGS_OF);
        self.write_eflags(updated);
    }

    fn lift_double_shift(&mut self, instr: &Instruction, left: bool) {
        self.materialize_flags();
        let ty = self.operand_type(instr, 0);
        let bits = ty.bits() as i64;
        let dst = self.read_operand(instr, 0, ty);
        let src = self.read_operand(instr, 1, ty);
        let raw_count = self.read_operand(instr, 2, I8);
        let raw_count32 = self.ensure_u32(raw_count);
        let count32 = self.builder.ins().band_imm(raw_count32, SHIFT_COUNT_MASK);

        let dst64 = self.builder.ins().uextend(I64, dst);
        let src64 = self.builder.ins().uextend(I64, src);
        let count64 = self.builder.ins().uextend(I64, count32);

        let (res, cf_out) = if left {
            let hi = self.builder.ins().ishl_imm(dst64, bits);
            let combined = self.builder.ins().bor(hi, src64);
            let shifted = self.builder.ins().ishl(combined, count64);
            let out_shifted = self.builder.ins().ushr_imm(shifted, bits);
            let out_val = self.coerce_int(out_shifted, ty);
            let cf_shift = self.builder.ins().ushr_imm(shifted, bits * 2);
            let cf_masked = self.builder.ins().band_imm(cf_shift, 1);
            let cf = self.builder.ins().ireduce(I32, cf_masked);
            (out_val, cf)
        } else {
            let hi = self.builder.ins().ishl_imm(src64, bits);
            let combined = self.builder.ins().bor(hi, dst64);
            let shifted = self.builder.ins().ushr(combined, count64);
            let out_val = self.coerce_int(shifted, ty);
            let count_minus_1 = self.builder.ins().iadd_imm(count64, -1);
            let cf_shift = self.builder.ins().ushr(combined, count_minus_1);
            let cf_masked = self.builder.ins().band_imm(cf_shift, 1);
            let cf = self.builder.ins().ireduce(I32, cf_masked);
            (out_val, cf)
        };

        let is_nz = self.builder.ins().icmp_imm(IntCC::NotEqual, count32, 0);
        let final_res = self.builder.ins().select(is_nz, res, dst);
        self.write_operand(instr, 0, final_res);

        let old_eflags = self.read_eflags();
        let zero32 = self.builder.ins().iconst(I32, 0);
        let final_res32 = self.ensure_u32(final_res);
        self.write_materialized_arith_flags(ty, final_res32, cf_out, zero32, zero32);
        let new_eflags = self.read_eflags();
        let selected_flags = self.builder.ins().select(is_nz, new_eflags, old_eflags);
        self.write_eflags(selected_flags);
    }

    fn lift_bit_test(&mut self, instr: &Instruction, action: BitTestAction) {
        self.materialize_flags();
        let ty = self.operand_type(instr, 0);
        let bits = ty.bits() as i64;
        let bit_pos = self.read_operand(instr, 1, ty);
        let pos32 = self.ensure_u32(bit_pos);

        let bit_val = if instr.op_kind(0) == OpKind::Memory && instr.op_kind(1) == OpKind::Register
        {
            let base_addr = self.compute_memory_addr(instr);
            let pos_signed = self.builder.ins().sextend(I32, bit_pos);
            let byte_off = self.builder.ins().sshr_imm(pos_signed, 3);
            let byte_addr = self.builder.ins().iadd(base_addr, byte_off);
            let byte_val = self.load_guest(byte_addr, I8);
            let bit_in_byte = self.builder.ins().band_imm(pos32, 7);
            let bit_in_byte_8 = self.builder.ins().ireduce(I8, bit_in_byte);
            let shifted = self.builder.ins().ushr(byte_val, bit_in_byte_8);
            let extracted = self.builder.ins().band_imm(shifted, 1);

            let one8 = self.builder.ins().iconst(I8, 1);
            let mask8 = self.builder.ins().ishl(one8, bit_in_byte_8);
            match action {
                BitTestAction::TestOnly => {}
                BitTestAction::Set => {
                    let updated = self.builder.ins().bor(byte_val, mask8);
                    self.store_guest(byte_addr, updated);
                }
                BitTestAction::Reset => {
                    let inv = self.builder.ins().bnot(mask8);
                    let updated = self.builder.ins().band(byte_val, inv);
                    self.store_guest(byte_addr, updated);
                }
                BitTestAction::Complement => {
                    let updated = self.builder.ins().bxor(byte_val, mask8);
                    self.store_guest(byte_addr, updated);
                }
            }
            self.ensure_u32(extracted)
        } else {
            let val = self.read_operand(instr, 0, ty);
            let bit_idx = self.builder.ins().band_imm(pos32, bits - 1);
            let idx_ty = self.coerce_int(bit_idx, ty);
            let shifted = self.builder.ins().ushr(val, idx_ty);
            let extracted = self.builder.ins().band_imm(shifted, 1);

            let one = self.builder.ins().iconst(ty, 1);
            let mask = self.builder.ins().ishl(one, idx_ty);
            match action {
                BitTestAction::TestOnly => {}
                BitTestAction::Set => {
                    let updated = self.builder.ins().bor(val, mask);
                    self.write_operand(instr, 0, updated);
                }
                BitTestAction::Reset => {
                    let inv = self.builder.ins().bnot(mask);
                    let updated = self.builder.ins().band(val, inv);
                    self.write_operand(instr, 0, updated);
                }
                BitTestAction::Complement => {
                    let updated = self.builder.ins().bxor(val, mask);
                    self.write_operand(instr, 0, updated);
                }
            }
            self.ensure_u32(extracted)
        };

        let eflags = self.read_eflags();
        let cleared = self.builder.ins().band_imm(eflags, (!EFLAGS_CF) as i64);
        let updated = self.set_flag_bit(cleared, bit_val, EFLAGS_CF);
        self.write_eflags(updated);
    }

    fn lift_bit_scan(&mut self, instr: &Instruction, forward: bool) {
        self.materialize_flags();
        let ty = self.operand_type(instr, 0);
        let bits = ty.bits() as i64;
        let src = self.read_operand(instr, 1, ty);
        let dst = self.read_operand(instr, 0, ty);

        let is_zero = self.builder.ins().icmp_imm(IntCC::Equal, src, 0);
        let idx = if forward {
            self.builder.ins().ctz(src)
        } else {
            let lz = self.builder.ins().clz(src);
            let max_idx = self.builder.ins().iconst(ty, bits - 1);
            self.builder.ins().isub(max_idx, lz)
        };

        let new_dst = self.builder.ins().select(is_zero, dst, idx);
        self.write_operand(instr, 0, new_dst);

        let zf_bit = self.builder.ins().uextend(I32, is_zero);
        let eflags = self.read_eflags();
        let cleared = self.builder.ins().band_imm(eflags, (!EFLAGS_ZF) as i64);
        let updated = self.set_flag_bit(cleared, zf_bit, EFLAGS_ZF);
        self.write_eflags(updated);
    }
}

#[derive(Copy, Clone)]
enum BitTestAction {
    TestOnly,
    Set,
    Reset,
    Complement,
}

const fn accumulator_for_type(ty: Type) -> Register {
    match ty {
        I8 => Register::AL,
        I16 => Register::AX,
        _ => Register::EAX,
    }
}

const fn is_setcc_mnemonic(m: Mnemonic) -> bool {
    matches!(
        m,
        Mnemonic::Seto
            | Mnemonic::Setno
            | Mnemonic::Setb
            | Mnemonic::Setae
            | Mnemonic::Sete
            | Mnemonic::Setne
            | Mnemonic::Setbe
            | Mnemonic::Seta
            | Mnemonic::Sets
            | Mnemonic::Setns
            | Mnemonic::Setp
            | Mnemonic::Setnp
            | Mnemonic::Setl
            | Mnemonic::Setge
            | Mnemonic::Setle
            | Mnemonic::Setg
    )
}

const fn is_cmovcc_mnemonic(m: Mnemonic) -> bool {
    matches!(
        m,
        Mnemonic::Cmovo
            | Mnemonic::Cmovno
            | Mnemonic::Cmovb
            | Mnemonic::Cmovae
            | Mnemonic::Cmove
            | Mnemonic::Cmovne
            | Mnemonic::Cmovbe
            | Mnemonic::Cmova
            | Mnemonic::Cmovs
            | Mnemonic::Cmovns
            | Mnemonic::Cmovp
            | Mnemonic::Cmovnp
            | Mnemonic::Cmovl
            | Mnemonic::Cmovge
            | Mnemonic::Cmovle
            | Mnemonic::Cmovg
    )
}
