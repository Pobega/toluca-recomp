use cranelift_codegen::ir::condcodes::IntCC;
use cranelift_codegen::ir::types::{I8, I16, I32};
use cranelift_codegen::ir::{InstBuilder, Type, Value};
use iced_x86::{Instruction, Mnemonic, OpKind, Register};
use toluca_core::bridge;
use toluca_core::cpu::{EFLAGS_DF, Gpr, LazyFlagOp};

use super::LifterContext;
use super::flags::Condition;

#[derive(Copy, Clone)]
enum StringOpKind {
    Movs,
    Stos,
    Lods,
    Cmps,
    Scas,
}

impl<'ctx, 'b> LifterContext<'ctx, 'b> {
    pub fn lift_string_op(&mut self, instr: &Instruction) -> bool {
        let has_xmm = (0..instr.op_count())
            .any(|i| instr.op_kind(i) == OpKind::Register && instr.op_register(i).is_xmm());
        if has_xmm {
            return false;
        }
        let Some((kind, ty)) = classify_string_op(instr.mnemonic()) else {
            return false;
        };

        let has_rep = instr.has_rep_prefix() || instr.has_repe_prefix();
        let has_repne = instr.has_repne_prefix();

        if has_rep && matches!(kind, StringOpKind::Movs | StringOpKind::Stos) {
            let offset = match kind {
                StringOpKind::Movs => bridge::offsets::REP_MOVS,
                StringOpKind::Stos => bridge::offsets::REP_STOS,
                _ => unreachable!(),
            };
            let elem_val = self.builder.ins().iconst(I32, ty.bytes() as i64);
            self.call_bridge_u32(offset, elem_val);
            return true;
        }

        let eflags = self.read_eflags();
        let df_bit = self.builder.ins().band_imm(eflags, EFLAGS_DF as i64);
        let df_set = self.builder.ins().icmp_imm(IntCC::NotEqual, df_bit, 0);
        let elem_bytes = ty.bytes() as i64;
        let pos_step = self.builder.ins().iconst(I32, elem_bytes);
        let neg_step = self.builder.ins().iconst(I32, -elem_bytes);
        let step = self.builder.ins().select(df_set, neg_step, pos_step);

        if !has_rep && !has_repne {
            self.emit_string_step(kind, ty, step);
            return true;
        }

        let head_block = self.builder.create_block();
        let body_block = self.builder.create_block();
        let exit_block = self.builder.create_block();

        self.builder.ins().jump(head_block, &[]);

        self.builder.switch_to_block(head_block);
        let ecx = self.read_gpr(Gpr::Ecx);
        let ecx_nz = self.builder.ins().icmp_imm(IntCC::NotEqual, ecx, 0);
        self.builder
            .ins()
            .brif(ecx_nz, body_block, &[], exit_block, &[]);

        self.builder.switch_to_block(body_block);
        self.emit_string_step(kind, ty, step);
        let cur_ecx = self.read_gpr(Gpr::Ecx);
        let next_ecx = self.builder.ins().iadd_imm(cur_ecx, -1);
        self.write_gpr(Gpr::Ecx, next_ecx);

        if matches!(kind, StringOpKind::Cmps | StringOpKind::Scas) {
            let cond = if has_repne {
                Condition::NE
            } else {
                Condition::E
            };
            let zf_match = self.eval_condition(cond);
            self.builder
                .ins()
                .brif(zf_match, head_block, &[], exit_block, &[]);
        } else {
            self.builder.ins().jump(head_block, &[]);
        }

        self.builder.switch_to_block(exit_block);
        true
    }

    fn emit_string_step(&mut self, kind: StringOpKind, ty: Type, step: Value) {
        let acc_reg = match ty {
            I8 => Register::AL,
            I16 => Register::AX,
            _ => Register::EAX,
        };

        match kind {
            StringOpKind::Movs => {
                let esi = self.read_gpr(Gpr::Esi);
                let edi = self.read_gpr(Gpr::Edi);
                let val = self.load_guest(esi, ty);
                self.store_guest(edi, val);
                let next_esi = self.builder.ins().iadd(esi, step);
                let next_edi = self.builder.ins().iadd(edi, step);
                self.write_gpr(Gpr::Esi, next_esi);
                self.write_gpr(Gpr::Edi, next_edi);
            }
            StringOpKind::Stos => {
                let edi = self.read_gpr(Gpr::Edi);
                let val = self.read_reg(acc_reg);
                self.store_guest(edi, val);
                let next_edi = self.builder.ins().iadd(edi, step);
                self.write_gpr(Gpr::Edi, next_edi);
            }
            StringOpKind::Lods => {
                let esi = self.read_gpr(Gpr::Esi);
                let val = self.load_guest(esi, ty);
                self.write_reg(acc_reg, val);
                let next_esi = self.builder.ins().iadd(esi, step);
                self.write_gpr(Gpr::Esi, next_esi);
            }
            StringOpKind::Cmps => {
                let esi = self.read_gpr(Gpr::Esi);
                let edi = self.read_gpr(Gpr::Edi);
                let lhs = self.load_guest(esi, ty);
                let rhs = self.load_guest(edi, ty);
                let diff = self.builder.ins().isub(lhs, rhs);
                self.set_lazy_flags(LazyFlagOp::Sub, ty.bytes(), lhs, rhs, diff);
                let next_esi = self.builder.ins().iadd(esi, step);
                let next_edi = self.builder.ins().iadd(edi, step);
                self.write_gpr(Gpr::Esi, next_esi);
                self.write_gpr(Gpr::Edi, next_edi);
            }
            StringOpKind::Scas => {
                let edi = self.read_gpr(Gpr::Edi);
                let lhs = self.read_reg(acc_reg);
                let rhs = self.load_guest(edi, ty);
                let diff = self.builder.ins().isub(lhs, rhs);
                self.set_lazy_flags(LazyFlagOp::Sub, ty.bytes(), lhs, rhs, diff);
                let next_edi = self.builder.ins().iadd(edi, step);
                self.write_gpr(Gpr::Edi, next_edi);
            }
        }
    }
}

const fn classify_string_op(mnemonic: Mnemonic) -> Option<(StringOpKind, Type)> {
    match mnemonic {
        Mnemonic::Movsb => Some((StringOpKind::Movs, I8)),
        Mnemonic::Movsw => Some((StringOpKind::Movs, I16)),
        Mnemonic::Movsd => Some((StringOpKind::Movs, I32)),
        Mnemonic::Stosb => Some((StringOpKind::Stos, I8)),
        Mnemonic::Stosw => Some((StringOpKind::Stos, I16)),
        Mnemonic::Stosd => Some((StringOpKind::Stos, I32)),
        Mnemonic::Lodsb => Some((StringOpKind::Lods, I8)),
        Mnemonic::Lodsw => Some((StringOpKind::Lods, I16)),
        Mnemonic::Lodsd => Some((StringOpKind::Lods, I32)),
        Mnemonic::Cmpsb => Some((StringOpKind::Cmps, I8)),
        Mnemonic::Cmpsw => Some((StringOpKind::Cmps, I16)),
        Mnemonic::Cmpsd => Some((StringOpKind::Cmps, I32)),
        Mnemonic::Scasb => Some((StringOpKind::Scas, I8)),
        Mnemonic::Scasw => Some((StringOpKind::Scas, I16)),
        Mnemonic::Scasd => Some((StringOpKind::Scas, I32)),
        _ => None,
    }
}
