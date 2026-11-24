use core::mem::offset_of;

use cranelift_codegen::ir::condcodes::FloatCC;
use cranelift_codegen::ir::types::{F32, F64, I16, I32, I64};
use cranelift_codegen::ir::{InstBuilder, MemFlags, Value};
use iced_x86::{Instruction, MemorySize, Mnemonic, OpKind, Register};
use toluca_core::bridge::{self, X87HelperOp};
use toluca_core::cpu::{
    EFLAGS_AF, EFLAGS_CF, EFLAGS_OF, EFLAGS_PF, EFLAGS_SF, EFLAGS_ZF, X87_DEFAULT_CONTROL_WORD,
    X87_DEFAULT_TAG_WORD, X87_SW_C0, X87_SW_C2, X87_SW_C3, X87_SW_TOP_MASK, X87_SW_TOP_SHIFT,
    X87State, offsets as cpu_offsets,
};

use super::LifterContext;
use super::flags::Condition;

const X87_STACK_MASK: i64 = 7;
const X87_REG_SHIFT: i64 = 3;
const X87_CC_MASK: u16 = X87_SW_C0 | X87_SW_C2 | X87_SW_C3;
const EFLAGS_FCOMI_MASK: u32 =
    EFLAGS_CF | EFLAGS_PF | EFLAGS_AF | EFLAGS_ZF | EFLAGS_SF | EFLAGS_OF;

impl<'ctx, 'b> LifterContext<'ctx, 'b> {
    pub fn lift_fpu(&mut self, instr: &Instruction) -> bool {
        match instr.mnemonic() {
            Mnemonic::Wait => true,
            Mnemonic::Fninit | Mnemonic::Finit => {
                let zero16 = self.builder.ins().iconst(I16, 0);
                let cw = self
                    .builder
                    .ins()
                    .iconst(I16, X87_DEFAULT_CONTROL_WORD as i64);
                let tw = self
                    .builder
                    .ins()
                    .iconst(I16, X87_DEFAULT_TAG_WORD as i64);
                let zero32 = self.builder.ins().iconst(I32, 0);
                self.x87_write_top(zero32);
                self.x87_store_u16(offset_of!(X87State, control_word), cw);
                self.x87_store_u16(offset_of!(X87State, status_word), zero16);
                self.x87_store_u16(offset_of!(X87State, tag_word), tw);
                true
            }
            Mnemonic::Fnclex | Mnemonic::Fclex => {
                let sw = self.x87_load_u16(offset_of!(X87State, status_word));
                // Clear sticky exception flags (bits 0..7) and busy bit (bit 15).
                let cleared = self.builder.ins().band_imm(sw, 0x7F00);
                self.x87_store_u16(offset_of!(X87State, status_word), cleared);
                true
            }
            Mnemonic::Fld => {
                let val = if instr.op_kind(0) == OpKind::Register {
                    self.x87_load_st(x87_reg_index(instr.op_register(0)))
                } else {
                    self.x87_load_mem_float(instr)
                };
                self.x87_push(val);
                true
            }
            Mnemonic::Fild => {
                let val = self.x87_load_mem_int(instr);
                self.x87_push(val);
                true
            }
            Mnemonic::Fldz => {
                let val = self.builder.ins().f64const(0.0);
                self.x87_push(val);
                true
            }
            Mnemonic::Fld1 => {
                let val = self.builder.ins().f64const(1.0);
                self.x87_push(val);
                true
            }
            Mnemonic::Fldpi => {
                let val = self.builder.ins().f64const(core::f64::consts::PI);
                self.x87_push(val);
                true
            }
            Mnemonic::Fldl2e => {
                let val = self.builder.ins().f64const(core::f64::consts::LOG2_E);
                self.x87_push(val);
                true
            }
            Mnemonic::Fldl2t => {
                let val = self.builder.ins().f64const(core::f64::consts::LOG2_10);
                self.x87_push(val);
                true
            }
            Mnemonic::Fldlg2 => {
                let val = self.builder.ins().f64const(core::f64::consts::LOG10_2);
                self.x87_push(val);
                true
            }
            Mnemonic::Fldln2 => {
                let val = self.builder.ins().f64const(core::f64::consts::LN_2);
                self.x87_push(val);
                true
            }
            Mnemonic::Fst => {
                let val = self.x87_load_st(0);
                if instr.op_kind(0) == OpKind::Register {
                    self.x87_store_st(x87_reg_index(instr.op_register(0)), val);
                } else {
                    self.x87_store_mem_float(instr, val);
                }
                true
            }
            Mnemonic::Fstp => {
                let val = self.x87_load_st(0);
                if instr.op_kind(0) == OpKind::Register {
                    self.x87_store_st(x87_reg_index(instr.op_register(0)), val);
                } else {
                    self.x87_store_mem_float(instr, val);
                }
                let _ = self.x87_pop();
                true
            }
            Mnemonic::Fist => {
                let val = self.x87_load_st(0);
                self.x87_store_mem_int(instr, val, false);
                true
            }
            Mnemonic::Fistp => {
                let val = self.x87_pop();
                self.x87_store_mem_int(instr, val, false);
                true
            }
            Mnemonic::Fisttp => {
                let val = self.x87_pop();
                self.x87_store_mem_int(instr, val, true);
                true
            }
            Mnemonic::Fadd => {
                self.lift_x87_binop(instr, X87BinOp::Add, false, false);
                true
            }
            Mnemonic::Faddp => {
                self.lift_x87_binop(instr, X87BinOp::Add, true, false);
                true
            }
            Mnemonic::Fiadd => {
                self.lift_x87_binop(instr, X87BinOp::Add, false, true);
                true
            }
            Mnemonic::Fsub => {
                self.lift_x87_binop(instr, X87BinOp::Sub, false, false);
                true
            }
            Mnemonic::Fsubp => {
                self.lift_x87_binop(instr, X87BinOp::Sub, true, false);
                true
            }
            Mnemonic::Fisub => {
                self.lift_x87_binop(instr, X87BinOp::Sub, false, true);
                true
            }
            Mnemonic::Fsubr => {
                self.lift_x87_binop(instr, X87BinOp::SubRev, false, false);
                true
            }
            Mnemonic::Fsubrp => {
                self.lift_x87_binop(instr, X87BinOp::SubRev, true, false);
                true
            }
            Mnemonic::Fisubr => {
                self.lift_x87_binop(instr, X87BinOp::SubRev, false, true);
                true
            }
            Mnemonic::Fmul => {
                self.lift_x87_binop(instr, X87BinOp::Mul, false, false);
                true
            }
            Mnemonic::Fmulp => {
                self.lift_x87_binop(instr, X87BinOp::Mul, true, false);
                true
            }
            Mnemonic::Fimul => {
                self.lift_x87_binop(instr, X87BinOp::Mul, false, true);
                true
            }
            Mnemonic::Fdiv => {
                self.lift_x87_binop(instr, X87BinOp::Div, false, false);
                true
            }
            Mnemonic::Fdivp => {
                self.lift_x87_binop(instr, X87BinOp::Div, true, false);
                true
            }
            Mnemonic::Fidiv => {
                self.lift_x87_binop(instr, X87BinOp::Div, false, true);
                true
            }
            Mnemonic::Fdivr => {
                self.lift_x87_binop(instr, X87BinOp::DivRev, false, false);
                true
            }
            Mnemonic::Fdivrp => {
                self.lift_x87_binop(instr, X87BinOp::DivRev, true, false);
                true
            }
            Mnemonic::Fidivr => {
                self.lift_x87_binop(instr, X87BinOp::DivRev, false, true);
                true
            }
            Mnemonic::Fchs => {
                let st0 = self.x87_load_st(0);
                let neg = self.builder.ins().fneg(st0);
                self.x87_store_st(0, neg);
                true
            }
            Mnemonic::Fabs => {
                let st0 = self.x87_load_st(0);
                let abs_val = self.builder.ins().fabs(st0);
                self.x87_store_st(0, abs_val);
                true
            }
            Mnemonic::Fsqrt => {
                let st0 = self.x87_load_st(0);
                let root = self.builder.ins().sqrt(st0);
                self.x87_store_st(0, root);
                true
            }
            Mnemonic::Fxch => {
                let other_idx = x87_second_reg_index(instr);
                let a = self.x87_load_st(0);
                let b = self.x87_load_st(other_idx);
                self.x87_store_st(0, b);
                self.x87_store_st(other_idx, a);
                true
            }
            Mnemonic::Fdecstp => {
                let top = self.x87_read_top();
                let dec = self.builder.ins().iadd_imm(top, -1);
                let wrapped = self.builder.ins().band_imm(dec, X87_STACK_MASK);
                self.x87_write_top(wrapped);
                true
            }
            Mnemonic::Fincstp => {
                let top = self.x87_read_top();
                let inc = self.builder.ins().iadd_imm(top, 1);
                let wrapped = self.builder.ins().band_imm(inc, X87_STACK_MASK);
                self.x87_write_top(wrapped);
                true
            }
            Mnemonic::Ffree => true,
            Mnemonic::Fcom | Mnemonic::Fucom => {
                let rhs = self.x87_compare_rhs(instr, false);
                let lhs = self.x87_load_st(0);
                self.x87_set_sw_compare(lhs, rhs);
                true
            }
            Mnemonic::Fcomp | Mnemonic::Fucomp => {
                let rhs = self.x87_compare_rhs(instr, false);
                let lhs = self.x87_load_st(0);
                self.x87_set_sw_compare(lhs, rhs);
                let _ = self.x87_pop();
                true
            }
            Mnemonic::Fcompp | Mnemonic::Fucompp => {
                let rhs = self.x87_load_st(1);
                let lhs = self.x87_load_st(0);
                self.x87_set_sw_compare(lhs, rhs);
                let _ = self.x87_pop();
                let _ = self.x87_pop();
                true
            }
            Mnemonic::Ficom => {
                let rhs = self.x87_load_mem_int(instr);
                let lhs = self.x87_load_st(0);
                self.x87_set_sw_compare(lhs, rhs);
                true
            }
            Mnemonic::Ficomp => {
                let rhs = self.x87_load_mem_int(instr);
                let lhs = self.x87_load_st(0);
                self.x87_set_sw_compare(lhs, rhs);
                let _ = self.x87_pop();
                true
            }
            Mnemonic::Ftst => {
                let lhs = self.x87_load_st(0);
                let zero = self.builder.ins().f64const(0.0);
                self.x87_set_sw_compare(lhs, zero);
                true
            }
            Mnemonic::Fcomi | Mnemonic::Fucomi => {
                let rhs_idx = if instr.op_count() > 1 && instr.op_kind(1) == OpKind::Register {
                    x87_reg_index(instr.op_register(1))
                } else {
                    1
                };
                let lhs = self.x87_load_st(0);
                let rhs = self.x87_load_st(rhs_idx);
                self.set_eflags_float_compare(lhs, rhs);
                true
            }
            Mnemonic::Fcomip | Mnemonic::Fucomip => {
                let rhs_idx = if instr.op_count() > 1 && instr.op_kind(1) == OpKind::Register {
                    x87_reg_index(instr.op_register(1))
                } else {
                    1
                };
                let lhs = self.x87_load_st(0);
                let rhs = self.x87_load_st(rhs_idx);
                self.set_eflags_float_compare(lhs, rhs);
                let _ = self.x87_pop();
                true
            }
            Mnemonic::Fcmovb => self.lift_fcmov(instr, Condition::B),
            Mnemonic::Fcmove => self.lift_fcmov(instr, Condition::E),
            Mnemonic::Fcmovbe => self.lift_fcmov(instr, Condition::BE),
            Mnemonic::Fcmovu => self.lift_fcmov(instr, Condition::P),
            Mnemonic::Fcmovnb => self.lift_fcmov(instr, Condition::AE),
            Mnemonic::Fcmovne => self.lift_fcmov(instr, Condition::NE),
            Mnemonic::Fcmovnbe => self.lift_fcmov(instr, Condition::A),
            Mnemonic::Fcmovnu => self.lift_fcmov(instr, Condition::NP),
            Mnemonic::Fnstsw | Mnemonic::Fstsw => {
                let sw = self.x87_load_u16(offset_of!(X87State, status_word));
                let sw_cleared = self.builder.ins().band_imm(sw, (!X87_SW_TOP_MASK) as i64);
                let top = self.x87_read_top();
                let top16 = self.builder.ins().ireduce(I16, top);
                let top_masked = self.builder.ins().band_imm(top16, X87_STACK_MASK);
                let top_shifted = self
                    .builder
                    .ins()
                    .ishl_imm(top_masked, X87_SW_TOP_SHIFT as i64);
                let eff_sw = self.builder.ins().bor(sw_cleared, top_shifted);
                self.write_operand(instr, 0, eff_sw);
                true
            }
            Mnemonic::Fnstcw | Mnemonic::Fstcw => {
                let cw = self.x87_load_u16(offset_of!(X87State, control_word));
                self.write_operand(instr, 0, cw);
                true
            }
            Mnemonic::Fldcw => {
                let cw = self.read_operand(instr, 0, I16);
                self.x87_store_u16(offset_of!(X87State, control_word), cw);
                true
            }
            Mnemonic::Fsin => self.call_x87_helper(X87HelperOp::Fsin),
            Mnemonic::Fcos => self.call_x87_helper(X87HelperOp::Fcos),
            Mnemonic::Fsincos => self.call_x87_helper(X87HelperOp::Fsincos),
            Mnemonic::Fptan => self.call_x87_helper(X87HelperOp::Fptan),
            Mnemonic::Fpatan => self.call_x87_helper(X87HelperOp::Fpatan),
            Mnemonic::F2xm1 => self.call_x87_helper(X87HelperOp::F2xm1),
            Mnemonic::Fyl2x => self.call_x87_helper(X87HelperOp::Fyl2x),
            Mnemonic::Fyl2xp1 => self.call_x87_helper(X87HelperOp::Fyl2xp1),
            Mnemonic::Fscale => self.call_x87_helper(X87HelperOp::Fscale),
            Mnemonic::Fprem => self.call_x87_helper(X87HelperOp::Fprem),
            Mnemonic::Fprem1 => self.call_x87_helper(X87HelperOp::Fprem1),
            Mnemonic::Frndint => self.call_x87_helper(X87HelperOp::Frndint),
            _ => false,
        }
    }

    pub fn set_eflags_float_compare(&mut self, lhs: Value, rhs: Value) {
        let is_lt = self.builder.ins().fcmp(FloatCC::LessThan, lhs, rhs);
        let is_eq = self.builder.ins().fcmp(FloatCC::Equal, lhs, rhs);
        let is_unord = self.builder.ins().fcmp(FloatCC::Unordered, lhs, rhs);

        let cf_b = self.builder.ins().bor(is_lt, is_unord);
        let zf_b = self.builder.ins().bor(is_eq, is_unord);
        let pf_b = is_unord;

        let cf = self.builder.ins().uextend(I32, cf_b);
        let zf = self.builder.ins().uextend(I32, zf_b);
        let pf = self.builder.ins().uextend(I32, pf_b);

        let mut flags = self.read_eflags();
        flags = self
            .builder
            .ins()
            .band_imm(flags, (!EFLAGS_FCOMI_MASK) as i64);

        let pf_shifted = self.builder.ins().ishl_imm(pf, EFLAGS_PF.trailing_zeros() as i64);
        let zf_shifted = self.builder.ins().ishl_imm(zf, EFLAGS_ZF.trailing_zeros() as i64);
        flags = self.builder.ins().bor(flags, cf);
        flags = self.builder.ins().bor(flags, pf_shifted);
        flags = self.builder.ins().bor(flags, zf_shifted);

        self.write_eflags(flags);
        self.clear_lazy_flags();
    }

    fn lift_fcmov(&mut self, instr: &Instruction, cond: Condition) -> bool {
        let src_idx = if instr.op_count() > 1 && instr.op_kind(1) == OpKind::Register {
            x87_reg_index(instr.op_register(1))
        } else {
            1
        };
        let cond_bit = self.eval_condition(cond);
        let st0 = self.x87_load_st(0);
        let src = self.x87_load_st(src_idx);
        let res = self.builder.ins().select(cond_bit, src, st0);
        self.x87_store_st(0, res);
        true
    }

    fn call_x87_helper(&mut self, op: X87HelperOp) -> bool {
        let op_val = self.builder.ins().iconst(I32, op as i64);
        self.call_bridge_u32(bridge::offsets::X87_HELPER, op_val);
        true
    }

    fn x87_compare_rhs(&mut self, instr: &Instruction, is_int: bool) -> Value {
        if instr.op_count() == 0 {
            return self.x87_load_st(1);
        }
        if instr.op_count() >= 2 && instr.op_kind(1) == OpKind::Register {
            return self.x87_load_st(x87_reg_index(instr.op_register(1)));
        }
        if instr.op_kind(0) == OpKind::Register {
            self.x87_load_st(x87_reg_index(instr.op_register(0)))
        } else if is_int {
            self.x87_load_mem_int(instr)
        } else {
            self.x87_load_mem_float(instr)
        }
    }

    fn x87_set_sw_compare(&mut self, lhs: Value, rhs: Value) {
        let is_lt = self.builder.ins().fcmp(FloatCC::LessThan, lhs, rhs);
        let is_eq = self.builder.ins().fcmp(FloatCC::Equal, lhs, rhs);
        let is_unord = self.builder.ins().fcmp(FloatCC::Unordered, lhs, rhs);

        let c0_b = self.builder.ins().bor(is_lt, is_unord);
        let c2_b = is_unord;
        let c3_b = self.builder.ins().bor(is_eq, is_unord);

        let c0_16 = self.builder.ins().uextend(I16, c0_b);
        let c2_16 = self.builder.ins().uextend(I16, c2_b);
        let c3_16 = self.builder.ins().uextend(I16, c3_b);

        let c0_s = self
            .builder
            .ins()
            .ishl_imm(c0_16, X87_SW_C0.trailing_zeros() as i64);
        let c2_s = self
            .builder
            .ins()
            .ishl_imm(c2_16, X87_SW_C2.trailing_zeros() as i64);
        let c3_s = self
            .builder
            .ins()
            .ishl_imm(c3_16, X87_SW_C3.trailing_zeros() as i64);

        let sw = self.x87_load_u16(offset_of!(X87State, status_word));
        let cleared = self.builder.ins().band_imm(sw, (!X87_CC_MASK) as i64);
        let w0 = self.builder.ins().bor(cleared, c0_s);
        let w2 = self.builder.ins().bor(w0, c2_s);
        let updated = self.builder.ins().bor(w2, c3_s);
        self.x87_store_u16(offset_of!(X87State, status_word), updated);
    }

    fn lift_x87_binop(&mut self, instr: &Instruction, op: X87BinOp, pop: bool, is_int: bool) {
        let (dst_idx, lhs, rhs) = if instr.op_count() == 0 {
            (1, self.x87_load_st(1), self.x87_load_st(0))
        } else if instr.op_count() == 1 {
            let lhs = self.x87_load_st(0);
            let rhs = if instr.op_kind(0) == OpKind::Register {
                self.x87_load_st(x87_reg_index(instr.op_register(0)))
            } else if is_int {
                self.x87_load_mem_int(instr)
            } else {
                self.x87_load_mem_float(instr)
            };
            let dst = if pop && instr.op_kind(0) == OpKind::Register {
                x87_reg_index(instr.op_register(0))
            } else {
                0
            };
            (dst, lhs, rhs)
        } else {
            let dst = x87_reg_index(instr.op_register(0));
            let src = x87_reg_index(instr.op_register(1));
            (dst, self.x87_load_st(dst), self.x87_load_st(src))
        };

        let res = match op {
            X87BinOp::Add => self.builder.ins().fadd(lhs, rhs),
            X87BinOp::Sub => self.builder.ins().fsub(lhs, rhs),
            X87BinOp::SubRev => self.builder.ins().fsub(rhs, lhs),
            X87BinOp::Mul => self.builder.ins().fmul(lhs, rhs),
            X87BinOp::Div => self.builder.ins().fdiv(lhs, rhs),
            X87BinOp::DivRev => self.builder.ins().fdiv(rhs, lhs),
        };
        self.x87_store_st(dst_idx, res);
        if pop {
            let _ = self.x87_pop();
        }
    }

    fn x87_read_top(&mut self) -> Value {
        let off = (cpu_offsets::X87 + offset_of!(X87State, top)) as i32;
        self.builder
            .ins()
            .load(I32, MemFlags::trusted(), self.cpu_ptr, off)
    }

    fn x87_write_top(&mut self, top: Value) {
        let masked = self.builder.ins().band_imm(top, X87_STACK_MASK);
        let off = (cpu_offsets::X87 + offset_of!(X87State, top)) as i32;
        self.builder
            .ins()
            .store(MemFlags::trusted(), masked, self.cpu_ptr, off);
    }

    fn x87_load_u16(&mut self, field_offset: usize) -> Value {
        let off = (cpu_offsets::X87 + field_offset) as i32;
        self.builder
            .ins()
            .load(I16, MemFlags::trusted(), self.cpu_ptr, off)
    }

    fn x87_store_u16(&mut self, field_offset: usize, val: Value) {
        let off = (cpu_offsets::X87 + field_offset) as i32;
        self.builder
            .ins()
            .store(MemFlags::trusted(), val, self.cpu_ptr, off);
    }

    fn x87_slot_addr(&mut self, st_index: u32) -> Value {
        let top = self.x87_read_top();
        let idx = if st_index == 0 {
            top
        } else {
            self.builder.ins().iadd_imm(top, st_index as i64)
        };
        let wrapped = self.builder.ins().band_imm(idx, X87_STACK_MASK);
        let byte_off = self.builder.ins().ishl_imm(wrapped, X87_REG_SHIFT);
        let off_ptr = if self.ptr_ty == I64 {
            self.builder.ins().uextend(I64, byte_off)
        } else {
            byte_off
        };
        let regs_offset = (cpu_offsets::X87 + offset_of!(X87State, regs)) as i64;
        let regs_base = self.builder.ins().iadd_imm(self.cpu_ptr, regs_offset);
        self.builder.ins().iadd(regs_base, off_ptr)
    }

    fn x87_load_st(&mut self, st_index: u32) -> Value {
        let addr = self.x87_slot_addr(st_index);
        self.builder.ins().load(F64, MemFlags::trusted(), addr, 0)
    }

    fn x87_store_st(&mut self, st_index: u32, val: Value) {
        let addr = self.x87_slot_addr(st_index);
        self.builder.ins().store(MemFlags::trusted(), val, addr, 0);
    }

    fn x87_push(&mut self, val: Value) {
        let top = self.x87_read_top();
        let dec = self.builder.ins().iadd_imm(top, -1);
        let wrapped = self.builder.ins().band_imm(dec, X87_STACK_MASK);
        self.x87_write_top(wrapped);
        self.x87_store_st(0, val);
    }

    fn x87_pop(&mut self) -> Value {
        let val = self.x87_load_st(0);
        let top = self.x87_read_top();
        let inc = self.builder.ins().iadd_imm(top, 1);
        let wrapped = self.builder.ins().band_imm(inc, X87_STACK_MASK);
        self.x87_write_top(wrapped);
        val
    }

    fn x87_load_mem_float(&mut self, instr: &Instruction) -> Value {
        let addr = self.compute_memory_addr(instr);
        match instr.memory_size() {
            MemorySize::Float32 => {
                let f32_val = self.load_guest(addr, F32);
                self.builder.ins().fpromote(F64, f32_val)
            }
            _ => self.load_guest(addr, F64),
        }
    }

    fn x87_store_mem_float(&mut self, instr: &Instruction, val: Value) {
        let addr = self.compute_memory_addr(instr);
        match instr.memory_size() {
            MemorySize::Float32 => {
                let f32_val = self.builder.ins().fdemote(F32, val);
                self.store_guest(addr, f32_val);
            }
            _ => self.store_guest(addr, val),
        }
    }

    fn x87_load_mem_int(&mut self, instr: &Instruction) -> Value {
        let addr = self.compute_memory_addr(instr);
        match instr.memory_size() {
            MemorySize::Int16 => {
                let raw16 = self.load_guest(addr, I16);
                let ext32 = self.builder.ins().sextend(I32, raw16);
                self.builder.ins().fcvt_from_sint(F64, ext32)
            }
            MemorySize::Int64 => {
                let raw64 = self.load_guest(addr, I64);
                self.builder.ins().fcvt_from_sint(F64, raw64)
            }
            _ => {
                let raw32 = self.load_guest(addr, I32);
                self.builder.ins().fcvt_from_sint(F64, raw32)
            }
        }
    }

    pub fn round_nearest_even_f64(&mut self, val: Value) -> Value {
        const TWO_POW_52: f64 = 4_503_599_627_370_496.0;
        let abs_val = self.builder.ins().fabs(val);
        let limit = self.builder.ins().f64const(TWO_POW_52);
        let in_range = self.builder.ins().fcmp(FloatCC::LessThan, abs_val, limit);
        let signed_limit = self.builder.ins().fcopysign(limit, val);
        let added = self.builder.ins().fadd(val, signed_limit);
        let rounded = self.builder.ins().fsub(added, signed_limit);
        self.builder.ins().select(in_range, rounded, val)
    }

    pub fn round_nearest_even_f32(&mut self, val: Value) -> Value {
        const TWO_POW_23: f32 = 8_388_608.0;
        let abs_val = self.builder.ins().fabs(val);
        let limit = self.builder.ins().f32const(TWO_POW_23);
        let in_range = self.builder.ins().fcmp(FloatCC::LessThan, abs_val, limit);
        let signed_limit = self.builder.ins().fcopysign(limit, val);
        let added = self.builder.ins().fadd(val, signed_limit);
        let rounded = self.builder.ins().fsub(added, signed_limit);
        self.builder.ins().select(in_range, rounded, val)
    }

    fn x87_round_for_int_store(&mut self, val: Value) -> Value {
        const X87_CW_RC_MASK: i64 = 0x0C00;
        const X87_CW_RC_FLOOR: i64 = 0x0400;
        const X87_CW_RC_CEIL: i64 = 0x0800;
        const X87_CW_RC_TRUNC: i64 = 0x0C00;
        const TWO_POW_52: f64 = 4_503_599_627_370_496.0;

        let cw = self.x87_load_u16(offset_of!(X87State, control_word));
        let rc = self.builder.ins().band_imm(cw, X87_CW_RC_MASK);

        let nearest = self.round_nearest_even_f64(val);

        // Compute trunc(val) for |val| < 2^52 without host libcalls.
        let abs_val = self.builder.ins().fabs(val);
        let limit = self.builder.ins().f64const(TWO_POW_52);
        let in_range = self.builder.ins().fcmp(FloatCC::LessThan, abs_val, limit);
        let trunc_i64 = self.builder.ins().fcvt_to_sint_sat(I64, val);
        let trunc_f64 = self.builder.ins().fcvt_from_sint(F64, trunc_i64);
        let trunc_val = self.builder.ins().select(in_range, trunc_f64, val);

        let one = self.builder.ins().f64const(1.0);
        let need_floor = self.builder.ins().fcmp(FloatCC::LessThan, val, trunc_val);
        let floor_sub = self.builder.ins().fsub(trunc_val, one);
        let floor_val = self.builder.ins().select(need_floor, floor_sub, trunc_val);

        let need_ceil = self.builder.ins().fcmp(FloatCC::GreaterThan, val, trunc_val);
        let ceil_add = self.builder.ins().fadd(trunc_val, one);
        let ceil_val = self.builder.ins().select(need_ceil, ceil_add, trunc_val);

        let is_floor = self.builder.ins().icmp_imm(
            cranelift_codegen::ir::condcodes::IntCC::Equal,
            rc,
            X87_CW_RC_FLOOR,
        );
        let is_ceil = self.builder.ins().icmp_imm(
            cranelift_codegen::ir::condcodes::IntCC::Equal,
            rc,
            X87_CW_RC_CEIL,
        );
        let is_trunc = self.builder.ins().icmp_imm(
            cranelift_codegen::ir::condcodes::IntCC::Equal,
            rc,
            X87_CW_RC_TRUNC,
        );

        let r1 = self.builder.ins().select(is_floor, floor_val, nearest);
        let r2 = self.builder.ins().select(is_ceil, ceil_val, r1);
        self.builder.ins().select(is_trunc, val, r2)
    }

    fn x87_store_mem_int(&mut self, instr: &Instruction, val: Value, truncate: bool) {
        let addr = self.compute_memory_addr(instr);
        let rounded = if truncate {
            val
        } else {
            self.x87_round_for_int_store(val)
        };
        match instr.memory_size() {
            MemorySize::Int16 => {
                let i32_val = self.builder.ins().fcvt_to_sint_sat(I32, rounded);
                let i16_val = self.builder.ins().ireduce(I16, i32_val);
                self.store_guest(addr, i16_val);
            }
            MemorySize::Int64 => {
                let i64_val = self.builder.ins().fcvt_to_sint_sat(I64, rounded);
                self.store_guest(addr, i64_val);
            }
            _ => {
                let i32_val = self.builder.ins().fcvt_to_sint_sat(I32, rounded);
                self.store_guest(addr, i32_val);
            }
        }
    }
}

#[derive(Copy, Clone)]
enum X87BinOp {
    Add,
    Sub,
    SubRev,
    Mul,
    Div,
    DivRev,
}

fn x87_second_reg_index(instr: &Instruction) -> u32 {
    if instr.op_count() >= 2 && instr.op_kind(1) == OpKind::Register {
        x87_reg_index(instr.op_register(1))
    } else if instr.op_count() == 1 && instr.op_kind(0) == OpKind::Register {
        let idx = x87_reg_index(instr.op_register(0));
        if idx == 0 { 1 } else { idx }
    } else {
        1
    }
}

const fn x87_reg_index(reg: Register) -> u32 {
    match reg {
        Register::ST0 => 0,
        Register::ST1 => 1,
        Register::ST2 => 2,
        Register::ST3 => 3,
        Register::ST4 => 4,
        Register::ST5 => 5,
        Register::ST6 => 6,
        Register::ST7 => 7,
        _ => 0,
    }
}
