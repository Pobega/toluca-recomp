use core::mem::offset_of;

use cranelift_codegen::ir::condcodes::IntCC;
use cranelift_codegen::ir::types::{I8, I16, I32};
use cranelift_codegen::ir::{InstBuilder, MemFlags, Value};
use iced_x86::{Instruction, Mnemonic, Register};
use toluca_core::bridge;
use toluca_core::cpu::{
    EFLAGS_AF, EFLAGS_CF, EFLAGS_DF, EFLAGS_FIXED_ONE, EFLAGS_OF, EFLAGS_PF, EFLAGS_SF, EFLAGS_ZF,
    LazyFlagOp, LazyFlags, offsets as cpu_offsets,
};

use super::LifterContext;

const LOW_BYTE_MASK: u32 = 0xFF;
const SAHF_FLAG_MASK: u32 = EFLAGS_CF | EFLAGS_PF | EFLAGS_AF | EFLAGS_ZF | EFLAGS_SF;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Condition {
    O,
    NO,
    B,
    AE,
    E,
    NE,
    BE,
    A,
    S,
    NS,
    P,
    NP,
    L,
    GE,
    LE,
    G,
}

pub const fn condition_from_mnemonic(mnemonic: Mnemonic) -> Option<Condition> {
    match mnemonic {
        Mnemonic::Jo | Mnemonic::Seto | Mnemonic::Cmovo => Some(Condition::O),
        Mnemonic::Jno | Mnemonic::Setno | Mnemonic::Cmovno => Some(Condition::NO),
        Mnemonic::Jb | Mnemonic::Setb | Mnemonic::Cmovb => Some(Condition::B),
        Mnemonic::Jae | Mnemonic::Setae | Mnemonic::Cmovae => Some(Condition::AE),
        Mnemonic::Je | Mnemonic::Sete | Mnemonic::Cmove => Some(Condition::E),
        Mnemonic::Jne | Mnemonic::Setne | Mnemonic::Cmovne => Some(Condition::NE),
        Mnemonic::Jbe | Mnemonic::Setbe | Mnemonic::Cmovbe => Some(Condition::BE),
        Mnemonic::Ja | Mnemonic::Seta | Mnemonic::Cmova => Some(Condition::A),
        Mnemonic::Js | Mnemonic::Sets | Mnemonic::Cmovs => Some(Condition::S),
        Mnemonic::Jns | Mnemonic::Setns | Mnemonic::Cmovns => Some(Condition::NS),
        Mnemonic::Jp | Mnemonic::Setp | Mnemonic::Cmovp => Some(Condition::P),
        Mnemonic::Jnp | Mnemonic::Setnp | Mnemonic::Cmovnp => Some(Condition::NP),
        Mnemonic::Jl | Mnemonic::Setl | Mnemonic::Cmovl => Some(Condition::L),
        Mnemonic::Jge | Mnemonic::Setge | Mnemonic::Cmovge => Some(Condition::GE),
        Mnemonic::Jle | Mnemonic::Setle | Mnemonic::Cmovle => Some(Condition::LE),
        Mnemonic::Jg | Mnemonic::Setg | Mnemonic::Cmovg => Some(Condition::G),
        _ => None,
    }
}

impl<'ctx, 'b> LifterContext<'ctx, 'b> {
    pub fn set_lazy_flags(
        &mut self,
        op: LazyFlagOp,
        byte_width: u32,
        lhs: Value,
        rhs: Value,
        res: Value,
    ) {
        let base = cpu_offsets::LAZY_FLAGS;
        let op_val = self.builder.ins().iconst(I32, op as i64);
        let width_val = self.builder.ins().iconst(I32, byte_width as i64);
        let lhs_32 = self.ensure_u32(lhs);
        let rhs_32 = self.ensure_u32(rhs);
        let res_32 = self.ensure_u32(res);

        self.store_lazy_field(base + offset_of!(LazyFlags, op), op_val);
        self.store_lazy_field(base + offset_of!(LazyFlags, width), width_val);
        self.store_lazy_field(base + offset_of!(LazyFlags, lhs), lhs_32);
        self.store_lazy_field(base + offset_of!(LazyFlags, rhs), rhs_32);
        self.store_lazy_field(base + offset_of!(LazyFlags, res), res_32);
    }

    pub fn clear_lazy_flags(&mut self) {
        let off = cpu_offsets::LAZY_FLAGS + offset_of!(LazyFlags, op);
        let zero = self.builder.ins().iconst(I32, LazyFlagOp::None as i64);
        self.store_lazy_field(off, zero);
    }

    pub fn materialize_flags(&mut self) {
        self.call_bridge_void(bridge::offsets::MATERIALIZE_FLAGS);
    }

    pub fn eval_condition(&mut self, cond: Condition) -> Value {
        self.materialize_flags();
        let eflags = self.read_eflags();
        match cond {
            Condition::O => self.flag_bit(eflags, EFLAGS_OF),
            Condition::NO => {
                let bit = self.flag_bit(eflags, EFLAGS_OF);
                self.builder.ins().bxor_imm(bit, 1)
            }
            Condition::B => self.flag_bit(eflags, EFLAGS_CF),
            Condition::AE => {
                let bit = self.flag_bit(eflags, EFLAGS_CF);
                self.builder.ins().bxor_imm(bit, 1)
            }
            Condition::E => self.flag_bit(eflags, EFLAGS_ZF),
            Condition::NE => {
                let bit = self.flag_bit(eflags, EFLAGS_ZF);
                self.builder.ins().bxor_imm(bit, 1)
            }
            Condition::BE => {
                let cf = self.flag_bit(eflags, EFLAGS_CF);
                let zf = self.flag_bit(eflags, EFLAGS_ZF);
                self.builder.ins().bor(cf, zf)
            }
            Condition::A => {
                let cf = self.flag_bit(eflags, EFLAGS_CF);
                let zf = self.flag_bit(eflags, EFLAGS_ZF);
                let cz = self.builder.ins().bor(cf, zf);
                self.builder.ins().bxor_imm(cz, 1)
            }
            Condition::S => self.flag_bit(eflags, EFLAGS_SF),
            Condition::NS => {
                let bit = self.flag_bit(eflags, EFLAGS_SF);
                self.builder.ins().bxor_imm(bit, 1)
            }
            Condition::P => self.flag_bit(eflags, EFLAGS_PF),
            Condition::NP => {
                let bit = self.flag_bit(eflags, EFLAGS_PF);
                self.builder.ins().bxor_imm(bit, 1)
            }
            Condition::L => {
                let sf = self.flag_bit(eflags, EFLAGS_SF);
                let of = self.flag_bit(eflags, EFLAGS_OF);
                self.builder.ins().bxor(sf, of)
            }
            Condition::GE => {
                let sf = self.flag_bit(eflags, EFLAGS_SF);
                let of = self.flag_bit(eflags, EFLAGS_OF);
                let diff = self.builder.ins().bxor(sf, of);
                self.builder.ins().bxor_imm(diff, 1)
            }
            Condition::LE => {
                let zf = self.flag_bit(eflags, EFLAGS_ZF);
                let sf = self.flag_bit(eflags, EFLAGS_SF);
                let of = self.flag_bit(eflags, EFLAGS_OF);
                let diff = self.builder.ins().bxor(sf, of);
                self.builder.ins().bor(zf, diff)
            }
            Condition::G => {
                let zf = self.flag_bit(eflags, EFLAGS_ZF);
                let sf = self.flag_bit(eflags, EFLAGS_SF);
                let of = self.flag_bit(eflags, EFLAGS_OF);
                let diff = self.builder.ins().bxor(sf, of);
                let le = self.builder.ins().bor(zf, diff);
                self.builder.ins().bxor_imm(le, 1)
            }
        }
    }

    pub fn lift_flag_instruction(&mut self, instr: &Instruction) -> bool {
        match instr.mnemonic() {
            Mnemonic::Clc => {
                self.materialize_flags();
                let eflags = self.read_eflags();
                let updated = self
                    .builder
                    .ins()
                    .band_imm(eflags, (!EFLAGS_CF) as i64);
                self.write_eflags(updated);
                true
            }
            Mnemonic::Stc => {
                self.materialize_flags();
                let eflags = self.read_eflags();
                let updated = self.builder.ins().bor_imm(eflags, EFLAGS_CF as i64);
                self.write_eflags(updated);
                true
            }
            Mnemonic::Cmc => {
                self.materialize_flags();
                let eflags = self.read_eflags();
                let updated = self.builder.ins().bxor_imm(eflags, EFLAGS_CF as i64);
                self.write_eflags(updated);
                true
            }
            Mnemonic::Cld => {
                let eflags = self.read_eflags();
                let updated = self
                    .builder
                    .ins()
                    .band_imm(eflags, (!EFLAGS_DF) as i64);
                self.write_eflags(updated);
                true
            }
            Mnemonic::Std => {
                let eflags = self.read_eflags();
                let updated = self.builder.ins().bor_imm(eflags, EFLAGS_DF as i64);
                self.write_eflags(updated);
                true
            }
            Mnemonic::Sahf => {
                self.materialize_flags();
                let ah = self.read_reg(Register::AH);
                let ah32 = self.ensure_u32(ah);
                let low_flags = self.builder.ins().band_imm(ah32, SAHF_FLAG_MASK as i64);
                let eflags = self.read_eflags();
                let preserved = self
                    .builder
                    .ins()
                    .band_imm(eflags, (!LOW_BYTE_MASK) as i64);
                let fixed = self
                    .builder
                    .ins()
                    .bor_imm(preserved, EFLAGS_FIXED_ONE as i64);
                let updated = self.builder.ins().bor(fixed, low_flags);
                self.write_eflags(updated);
                true
            }
            Mnemonic::Lahf => {
                self.materialize_flags();
                let eflags = self.read_eflags();
                let low = self.builder.ins().ireduce(I8, eflags);
                self.write_reg(Register::AH, low);
                true
            }
            Mnemonic::Pushfd => {
                self.materialize_flags();
                let eflags = self.read_eflags();
                self.push32(eflags);
                true
            }
            Mnemonic::Pushf => {
                self.materialize_flags();
                let eflags = self.read_eflags();
                let low16 = self.builder.ins().ireduce(I16, eflags);
                self.push16(low16);
                true
            }
            Mnemonic::Popfd => {
                self.clear_lazy_flags();
                let popped = self.pop32();
                let updated = self.builder.ins().bor_imm(popped, EFLAGS_FIXED_ONE as i64);
                self.write_eflags(updated);
                true
            }
            Mnemonic::Popf => {
                self.clear_lazy_flags();
                let popped16 = self.pop16();
                let popped32 = self.ensure_u32(popped16);
                let cur = self.read_eflags();
                let upper = self.builder.ins().band_imm(cur, 0xFFFF_0000u32 as i64);
                let combined = self.builder.ins().bor(upper, popped32);
                let updated = self
                    .builder
                    .ins()
                    .bor_imm(combined, EFLAGS_FIXED_ONE as i64);
                self.write_eflags(updated);
                true
            }
            _ => false,
        }
    }

    pub fn store_lazy_field(&mut self, offset: usize, val: Value) {
        self.builder
            .ins()
            .store(MemFlags::trusted(), val, self.cpu_ptr, offset as i32);
    }

    fn flag_bit(&mut self, eflags: Value, mask: u32) -> Value {
        let masked = self.builder.ins().band_imm(eflags, mask as i64);
        self.builder.ins().icmp_imm(IntCC::NotEqual, masked, 0)
    }
}
