use std::collections::BTreeSet;

use cranelift_codegen::ir::condcodes::IntCC;
use cranelift_codegen::ir::types::I32;
use cranelift_codegen::ir::{Block, InstBuilder, MemFlags, Value};
use cranelift_frontend::Switch;
use iced_x86::{Code, FlowControl, Instruction, Mnemonic, OpKind, Register};
use toluca_core::GuestAddr;
use toluca_core::bridge;
use toluca_core::cpu::{ExitReason, Gpr, offsets as cpu_offsets};

use super::LifterContext;
use super::flags::{Condition, condition_from_mnemonic};

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ControlFlowOutcome {
    Continue,
    Terminated,
    NotHandled,
}

impl<'ctx, 'b> LifterContext<'ctx, 'b> {
    pub fn lift_control_flow(
        &mut self,
        instr: &Instruction,
        next_va: GuestAddr,
        known_functions: &BTreeSet<GuestAddr>,
    ) -> ControlFlowOutcome {
        let instr_va = instr.ip() as GuestAddr;

        match instr.mnemonic() {
            Mnemonic::Nop
            | Mnemonic::Fnop
            | Mnemonic::Prefetchnta
            | Mnemonic::Prefetcht0
            | Mnemonic::Prefetcht1
            | Mnemonic::Prefetcht2
            | Mnemonic::Prefetchw
            | Mnemonic::Prefetchwt1
            | Mnemonic::Sfence
            | Mnemonic::Lfence
            | Mnemonic::Mfence
            | Mnemonic::Pause
            | Mnemonic::Clflush => return ControlFlowOutcome::Continue,
            Mnemonic::Cpuid => {
                self.call_bridge_void(bridge::offsets::CPUID);
                return ControlFlowOutcome::Continue;
            }
            Mnemonic::Rdtsc => {
                self.call_bridge_void(bridge::offsets::RDTSC);
                return ControlFlowOutcome::Continue;
            }
            Mnemonic::Hlt => {
                let reason = self.builder.ins().iconst(I32, ExitReason::Halt as i64);
                self.builder.ins().store(
                    MemFlags::trusted(),
                    reason,
                    self.cpu_ptr,
                    cpu_offsets::EXIT_REASON as i32,
                );
                self.builder.ins().return_(&[]);
                return ControlFlowOutcome::Terminated;
            }
            Mnemonic::Int3 | Mnemonic::Ud2 => {
                self.emit_unhandled_and_return(instr_va);
                return ControlFlowOutcome::Terminated;
            }
            Mnemonic::Jecxz | Mnemonic::Jcxz => {
                let reg = if instr.code() == Code::Jcxz_rel8_16 {
                    Register::CX
                } else {
                    Register::ECX
                };
                let val = self.read_reg(reg);
                let is_zero = self.builder.ins().icmp_imm(IntCC::Equal, val, 0);
                let target_va = instr.near_branch32();
                self.emit_cond_branch(is_zero, target_va, next_va, known_functions);
                return ControlFlowOutcome::Terminated;
            }
            Mnemonic::Loop | Mnemonic::Loope | Mnemonic::Loopne => {
                let reg = if matches!(
                    instr.code(),
                    Code::Loop_rel8_16_CX | Code::Loope_rel8_16_CX | Code::Loopne_rel8_16_CX
                ) {
                    Register::CX
                } else {
                    Register::ECX
                };
                let count = self.read_reg(reg);
                let decremented = self.builder.ins().iadd_imm(count, -1);
                self.write_reg(reg, decremented);
                let nz = self.builder.ins().icmp_imm(IntCC::NotEqual, decremented, 0);
                let cond = match instr.mnemonic() {
                    Mnemonic::Loope => {
                        let zf = self.eval_condition(Condition::E);
                        self.builder.ins().band(nz, zf)
                    }
                    Mnemonic::Loopne => {
                        let nzf = self.eval_condition(Condition::NE);
                        self.builder.ins().band(nz, nzf)
                    }
                    _ => nz,
                };
                let target_va = instr.near_branch32();
                self.emit_cond_branch(cond, target_va, next_va, known_functions);
                return ControlFlowOutcome::Terminated;
            }
            _ => {}
        }

        match instr.flow_control() {
            FlowControl::UnconditionalBranch => {
                let target_va = instr.near_branch32();
                if let Some(&block) = self.func_blocks.get(&target_va) {
                    self.builder.ins().jump(block, &[]);
                } else {
                    self.emit_tail_call(target_va, known_functions);
                }
                ControlFlowOutcome::Terminated
            }
            FlowControl::ConditionalBranch => {
                let Some(cond_kind) = condition_from_mnemonic(instr.mnemonic()) else {
                    return ControlFlowOutcome::NotHandled;
                };
                let cond = self.eval_condition(cond_kind);
                let target_va = instr.near_branch32();
                self.emit_cond_branch(cond, target_va, next_va, known_functions);
                ControlFlowOutcome::Terminated
            }
            FlowControl::Call => {
                let target_va = instr.near_branch32();
                let ret_va = self.builder.ins().iconst(I32, next_va as i64);
                self.push32(ret_va);
                if known_functions.contains(&target_va) {
                    let func_ref = self.get_or_declare_direct_call(target_va);
                    self.builder.ins().call(func_ref, &[self.cpu_ptr]);
                } else {
                    let target_val = self.builder.ins().iconst(I32, target_va as i64);
                    self.call_bridge_u32(bridge::offsets::CALL_INDIRECT, target_val);
                }
                self.emit_post_call_exit_check(next_va)
            }
            FlowControl::IndirectCall => {
                if let Some(iat_va) =
                    absolute_mem_disp(instr).filter(|&va| self.image.iat_map.contains_key(&va))
                {
                    let import_idx = self.image.iat_map[&iat_va];
                    let ret_va = self.builder.ins().iconst(I32, next_va as i64);
                    self.push32(ret_va);
                    let idx_val = self.builder.ins().iconst(I32, import_idx as i64);
                    self.call_bridge_u32(bridge::offsets::CALL_IMPORT, idx_val);
                } else {
                    // Evaluate operand before pushing the return address in case it uses ESP.
                    let target_va = self.read_operand(instr, 0, I32);
                    let ret_va = self.builder.ins().iconst(I32, next_va as i64);
                    self.push32(ret_va);
                    self.call_bridge_u32(bridge::offsets::CALL_INDIRECT, target_va);
                }
                self.emit_post_call_exit_check(next_va)
            }
            FlowControl::IndirectBranch => {
                if let Some(iat_va) =
                    absolute_mem_disp(instr).filter(|&va| self.image.iat_map.contains_key(&va))
                {
                    let import_idx = self.image.iat_map[&iat_va];
                    let idx_val = self.builder.ins().iconst(I32, import_idx as i64);
                    self.call_bridge_u32(bridge::offsets::CALL_IMPORT, idx_val);
                    self.builder.ins().return_(&[]);
                    return ControlFlowOutcome::Terminated;
                }

                if let Some(jt) = self
                    .jump_tables
                    .get(&instr_va)
                    .copied()
                    .filter(|_| instr.memory_index() != Register::None)
                {
                    let index_val = self.read_reg(instr.memory_index());
                    let index32 = self.ensure_u32(index_val);

                    let default_block = self.builder.create_block();
                    let mut switch = Switch::new();
                    for (case_idx, &target_va) in jt.targets.iter().enumerate() {
                        if let Some(&target_block) = self.func_blocks.get(&target_va) {
                            switch.set_entry(case_idx as u128, target_block);
                        }
                    }
                    switch.emit(self.builder, index32, default_block);

                    self.builder.switch_to_block(default_block);
                    let target_va = self.read_operand(instr, 0, I32);
                    self.call_bridge_u32(bridge::offsets::CALL_INDIRECT, target_va);
                    self.builder.ins().return_(&[]);
                    return ControlFlowOutcome::Terminated;
                }

                let target_va = self.read_operand(instr, 0, I32);
                self.call_bridge_u32(bridge::offsets::CALL_INDIRECT, target_va);
                self.builder.ins().return_(&[]);
                ControlFlowOutcome::Terminated
            }
            FlowControl::Return => {
                let ret_va = self.pop32();
                self.builder.ins().store(
                    MemFlags::trusted(),
                    ret_va,
                    self.cpu_ptr,
                    cpu_offsets::EIP as i32,
                );
                if instr.op_count() > 0 {
                    let imm = instr.immediate16() as i64;
                    let esp = self.read_gpr(Gpr::Esp);
                    let adjusted = self.builder.ins().iadd_imm(esp, imm);
                    self.write_gpr(Gpr::Esp, adjusted);
                }
                self.builder.ins().return_(&[]);
                ControlFlowOutcome::Terminated
            }
            _ => ControlFlowOutcome::NotHandled,
        }
    }

    fn emit_post_call_exit_check(&mut self, next_va: GuestAddr) -> ControlFlowOutcome {
        let exit_reason = self.builder.ins().load(
            I32,
            MemFlags::trusted(),
            self.cpu_ptr,
            cpu_offsets::EXIT_REASON as i32,
        );
        let running = self.builder.ins().iconst(I32, ExitReason::Running as i64);
        let is_exiting = self.builder.ins().icmp(IntCC::NotEqual, exit_reason, running);
        if let Some(&next_block) = self.func_blocks.get(&next_va) {
            self.builder
                .ins()
                .brif(is_exiting, self.unwind_block, &[], next_block, &[]);
            ControlFlowOutcome::Terminated
        } else {
            let continue_block = self.builder.create_block();
            self.builder
                .ins()
                .brif(is_exiting, self.unwind_block, &[], continue_block, &[]);
            self.builder.switch_to_block(continue_block);
            ControlFlowOutcome::Continue
        }
    }

    pub fn emit_unsupported(&mut self, guest_va: GuestAddr) {
        self.unsupported_count += 1;
        let va_val = self.builder.ins().iconst(I32, guest_va as i64);
        let zero = self.builder.ins().iconst(I32, 0);
        self.call_bridge_u32_u32(bridge::offsets::UNHANDLED_INSTRUCTION, va_val, zero);
    }

    fn emit_unhandled_and_return(&mut self, guest_va: GuestAddr) {
        let va_val = self.builder.ins().iconst(I32, guest_va as i64);
        let zero = self.builder.ins().iconst(I32, 0);
        self.call_bridge_u32_u32(bridge::offsets::UNHANDLED_INSTRUCTION, va_val, zero);
        self.builder.ins().return_(&[]);
    }

    fn emit_tail_call(&mut self, target_va: GuestAddr, known_functions: &BTreeSet<GuestAddr>) {
        if known_functions.contains(&target_va) {
            let func_ref = self.get_or_declare_direct_call(target_va);
            self.builder.ins().call(func_ref, &[self.cpu_ptr]);
        } else {
            let target_val = self.builder.ins().iconst(I32, target_va as i64);
            self.call_bridge_u32(bridge::offsets::CALL_INDIRECT, target_val);
        }
        self.builder.ins().return_(&[]);
    }

    fn emit_cond_branch(
        &mut self,
        cond: Value,
        taken_va: GuestAddr,
        fallthrough_va: GuestAddr,
        known_functions: &BTreeSet<GuestAddr>,
    ) {
        let taken_block = self.resolve_branch_target_block(taken_va, known_functions);
        let fallthrough_block = self.resolve_branch_target_block(fallthrough_va, known_functions);
        self.builder
            .ins()
            .brif(cond, taken_block, &[], fallthrough_block, &[]);
    }

    fn resolve_branch_target_block(
        &mut self,
        target_va: GuestAddr,
        known_functions: &BTreeSet<GuestAddr>,
    ) -> Block {
        if let Some(&block) = self.func_blocks.get(&target_va) {
            return block;
        }
        let trampoline = self.builder.create_block();
        let saved = self.builder.current_block();
        self.builder.switch_to_block(trampoline);
        self.emit_tail_call(target_va, known_functions);
        if let Some(prev) = saved {
            self.builder.switch_to_block(prev);
        }
        trampoline
    }
}

fn absolute_mem_disp(instr: &Instruction) -> Option<GuestAddr> {
    if instr.op0_kind() == OpKind::Memory
        && instr.memory_base() == Register::None
        && instr.memory_index() == Register::None
    {
        Some(instr.memory_displacement32())
    } else {
        None
    }
}
