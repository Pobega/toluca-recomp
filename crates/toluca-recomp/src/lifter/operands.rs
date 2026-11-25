use cranelift_codegen::ir::types::{I8, I16, I32, I64};
use cranelift_codegen::ir::{InstBuilder, MemFlags, Type, Value};
use iced_x86::{Instruction, OpKind, Register};
use toluca_core::bridge;
use toluca_core::cpu::{Gpr, offsets as cpu_offsets};

use super::LifterContext;

pub const ALL_GPRS: [Gpr; 8] = [
    Gpr::Eax,
    Gpr::Ecx,
    Gpr::Edx,
    Gpr::Ebx,
    Gpr::Esp,
    Gpr::Ebp,
    Gpr::Esi,
    Gpr::Edi,
];

impl<'ctx, 'b> LifterContext<'ctx, 'b> {
    pub fn gpr_index(reg: Register) -> Option<Gpr> {
        match reg {
            Register::AL | Register::AH | Register::AX | Register::EAX => Some(Gpr::Eax),
            Register::CL | Register::CH | Register::CX | Register::ECX => Some(Gpr::Ecx),
            Register::DL | Register::DH | Register::DX | Register::EDX => Some(Gpr::Edx),
            Register::BL | Register::BH | Register::BX | Register::EBX => Some(Gpr::Ebx),
            Register::SP | Register::ESP => Some(Gpr::Esp),
            Register::BP | Register::EBP => Some(Gpr::Ebp),
            Register::SI | Register::ESI => Some(Gpr::Esi),
            Register::DI | Register::EDI => Some(Gpr::Edi),
            _ => None,
        }
    }

    pub const fn is_high_8bit(reg: Register) -> bool {
        matches!(
            reg,
            Register::AH | Register::CH | Register::DH | Register::BH
        )
    }

    pub const fn reg_byte_width(reg: Register) -> usize {
        match reg {
            Register::AL
            | Register::AH
            | Register::CL
            | Register::CH
            | Register::DL
            | Register::DH
            | Register::BL
            | Register::BH => 1,
            Register::AX
            | Register::CX
            | Register::DX
            | Register::BX
            | Register::SP
            | Register::BP
            | Register::SI
            | Register::DI => 2,
            _ => 4,
        }
    }

    pub const fn reg_type(reg: Register) -> Type {
        match Self::reg_byte_width(reg) {
            1 => I8,
            2 => I16,
            _ => I32,
        }
    }

    pub const fn type_for_bytes(byte_width: usize) -> Type {
        match byte_width {
            1 => I8,
            2 => I16,
            _ => I32,
        }
    }

    pub fn operand_type(&self, instr: &Instruction, op_idx: u32) -> Type {
        match instr.op_kind(op_idx) {
            OpKind::Register => Self::reg_type(instr.op_register(op_idx)),
            OpKind::Memory => Self::type_for_bytes(instr.memory_size().size()),
            OpKind::Immediate8 | OpKind::Immediate8_2nd => I8,
            OpKind::Immediate16 | OpKind::Immediate8to16 => I16,
            _ => I32,
        }
    }

    pub fn read_gpr(&mut self, gpr: Gpr) -> Value {
        self.builder.use_var(self.gpr_vars[gpr as usize])
    }

    pub fn write_gpr(&mut self, gpr: Gpr, val: Value) {
        self.builder.def_var(self.gpr_vars[gpr as usize], val);
    }

    pub fn flush_gprs(&mut self) {
        for gpr in ALL_GPRS {
            let val = self.builder.use_var(self.gpr_vars[gpr as usize]);
            let off = toluca_core::GuestCpuState::gpr_offset(gpr) as i32;
            self.builder
                .ins()
                .store(MemFlags::trusted(), val, self.cpu_ptr, off);
        }
    }

    pub fn reload_gprs(&mut self) {
        for gpr in ALL_GPRS {
            let off = toluca_core::GuestCpuState::gpr_offset(gpr) as i32;
            let val = self
                .builder
                .ins()
                .load(I32, MemFlags::trusted(), self.cpu_ptr, off);
            self.builder.def_var(self.gpr_vars[gpr as usize], val);
        }
    }

    pub fn read_reg(&mut self, reg: Register) -> Value {
        let Some(gpr) = Self::gpr_index(reg) else {
            return self.builder.ins().iconst(Self::reg_type(reg), 0);
        };
        let full = self.read_gpr(gpr);
        if Self::is_high_8bit(reg) {
            let shifted = self.builder.ins().ushr_imm(full, 8);
            return self.builder.ins().ireduce(I8, shifted);
        }
        match Self::reg_byte_width(reg) {
            1 => self.builder.ins().ireduce(I8, full),
            2 => self.builder.ins().ireduce(I16, full),
            _ => full,
        }
    }

    pub fn write_reg(&mut self, reg: Register, val: Value) {
        let Some(gpr) = Self::gpr_index(reg) else {
            return;
        };
        if Self::is_high_8bit(reg) {
            let old = self.read_gpr(gpr);
            let cleared = self.builder.ins().band_imm(old, 0xFFFF_00FFu32 as i64);
            let ext = self.ensure_u32(val);
            let shifted = self.builder.ins().ishl_imm(ext, 8);
            let combined = self.builder.ins().bor(cleared, shifted);
            self.write_gpr(gpr, combined);
            return;
        }
        match Self::reg_byte_width(reg) {
            1 => {
                let old = self.read_gpr(gpr);
                let cleared = self.builder.ins().band_imm(old, 0xFFFF_FF00u32 as i64);
                let ext = self.ensure_u32(val);
                let combined = self.builder.ins().bor(cleared, ext);
                self.write_gpr(gpr, combined);
            }
            2 => {
                let old = self.read_gpr(gpr);
                let cleared = self.builder.ins().band_imm(old, 0xFFFF_0000u32 as i64);
                let ext = self.ensure_u32(val);
                let combined = self.builder.ins().bor(cleared, ext);
                self.write_gpr(gpr, combined);
            }
            _ => {
                let ext = self.ensure_u32(val);
                self.write_gpr(gpr, ext);
            }
        }
    }

    pub fn compute_memory_addr(&mut self, instr: &Instruction) -> Value {
        let mut addr = self
            .builder
            .ins()
            .iconst(I32, instr.memory_displacement32() as i64);

        let base = instr.memory_base();
        if base != Register::None {
            let base_val = self.read_reg(base);
            let base_32 = self.ensure_u32(base_val);
            addr = self.builder.ins().iadd(addr, base_32);
        }

        let index = instr.memory_index();
        if index != Register::None {
            let idx_val = self.read_reg(index);
            let mut idx_32 = self.ensure_u32(idx_val);
            let scale = instr.memory_index_scale();
            if scale > 1 {
                idx_32 = self
                    .builder
                    .ins()
                    .ishl_imm(idx_32, scale.trailing_zeros() as i64);
            }
            addr = self.builder.ins().iadd(addr, idx_32);
        }

        match instr.segment_prefix() {
            Register::FS => {
                let fs_base = self.builder.ins().load(
                    I32,
                    MemFlags::trusted(),
                    self.cpu_ptr,
                    cpu_offsets::FS_BASE as i32,
                );
                addr = self.builder.ins().iadd(addr, fs_base);
            }
            Register::GS => {
                let gs_base = self.builder.ins().load(
                    I32,
                    MemFlags::trusted(),
                    self.cpu_ptr,
                    cpu_offsets::GS_BASE as i32,
                );
                addr = self.builder.ins().iadd(addr, gs_base);
            }
            _ => {}
        }

        addr
    }

    pub fn guest_to_host_ptr(&mut self, guest_va: Value) -> Value {
        let offset = if self.ptr_ty == I64 {
            self.builder.ins().uextend(I64, guest_va)
        } else {
            guest_va
        };
        self.builder.ins().iadd(self.mem_base, offset)
    }

    pub fn load_guest(&mut self, guest_va: Value, ty: Type) -> Value {
        let host_ptr = self.guest_to_host_ptr(guest_va);
        self.builder.ins().load(ty, MemFlags::new(), host_ptr, 0)
    }

    pub fn store_guest(&mut self, guest_va: Value, val: Value) {
        let host_ptr = self.guest_to_host_ptr(guest_va);
        self.builder.ins().store(MemFlags::new(), val, host_ptr, 0);
    }

    pub fn read_operand(&mut self, instr: &Instruction, op_idx: u32, width_ty: Type) -> Value {
        match instr.op_kind(op_idx) {
            OpKind::Register => {
                let val = self.read_reg(instr.op_register(op_idx));
                self.coerce_int(val, width_ty)
            }
            OpKind::Memory => {
                let addr = self.compute_memory_addr(instr);
                self.load_guest(addr, width_ty)
            }
            OpKind::Immediate8 => self
                .builder
                .ins()
                .iconst(width_ty, instr.immediate8() as i64),
            OpKind::Immediate8_2nd => self
                .builder
                .ins()
                .iconst(width_ty, instr.immediate8_2nd() as i64),
            OpKind::Immediate8to16 => self
                .builder
                .ins()
                .iconst(width_ty, instr.immediate8to16() as i64),
            OpKind::Immediate8to32 => self
                .builder
                .ins()
                .iconst(width_ty, instr.immediate8to32() as i64),
            OpKind::Immediate16 => self
                .builder
                .ins()
                .iconst(width_ty, instr.immediate16() as i64),
            OpKind::Immediate32 => self
                .builder
                .ins()
                .iconst(width_ty, instr.immediate32() as i64),
            _ => self.builder.ins().iconst(width_ty, 0),
        }
    }

    pub fn write_operand(&mut self, instr: &Instruction, op_idx: u32, val: Value) {
        match instr.op_kind(op_idx) {
            OpKind::Register => self.write_reg(instr.op_register(op_idx), val),
            OpKind::Memory => {
                let addr = self.compute_memory_addr(instr);
                self.store_guest(addr, val);
            }
            _ => {}
        }
    }

    pub fn push32(&mut self, val: Value) {
        let val32 = self.ensure_u32(val);
        let esp = self.read_gpr(Gpr::Esp);
        let new_esp = self.builder.ins().iadd_imm(esp, -4);
        self.write_gpr(Gpr::Esp, new_esp);
        self.store_guest(new_esp, val32);
    }

    pub fn pop32(&mut self) -> Value {
        let esp = self.read_gpr(Gpr::Esp);
        let val = self.load_guest(esp, I32);
        let new_esp = self.builder.ins().iadd_imm(esp, 4);
        self.write_gpr(Gpr::Esp, new_esp);
        val
    }

    pub fn push16(&mut self, val: Value) {
        let val16 = self.coerce_int(val, I16);
        let esp = self.read_gpr(Gpr::Esp);
        let new_esp = self.builder.ins().iadd_imm(esp, -2);
        self.write_gpr(Gpr::Esp, new_esp);
        self.store_guest(new_esp, val16);
    }

    pub fn pop16(&mut self) -> Value {
        let esp = self.read_gpr(Gpr::Esp);
        let val = self.load_guest(esp, I16);
        let new_esp = self.builder.ins().iadd_imm(esp, 2);
        self.write_gpr(Gpr::Esp, new_esp);
        val
    }

    pub fn read_eflags(&mut self) -> Value {
        self.builder.ins().load(
            I32,
            MemFlags::trusted(),
            self.cpu_ptr,
            cpu_offsets::EFLAGS as i32,
        )
    }

    pub fn write_eflags(&mut self, val: Value) {
        self.builder.ins().store(
            MemFlags::trusted(),
            val,
            self.cpu_ptr,
            cpu_offsets::EFLAGS as i32,
        );
    }

    pub fn call_bridge_void(&mut self, offset: usize) {
        self.flush_pending_flags();
        let sync_gprs = offset != bridge::offsets::MATERIALIZE_FLAGS;
        if sync_gprs {
            self.flush_gprs();
        }
        let fn_ptr = self.builder.ins().load(
            self.ptr_ty,
            MemFlags::trusted(),
            self.bridge_ptr,
            offset as i32,
        );
        self.builder
            .ins()
            .call_indirect(self.sig_void_cpu, fn_ptr, &[self.cpu_ptr]);
        if sync_gprs {
            self.reload_gprs();
        }
        self.flags_known_clean = offset == bridge::offsets::MATERIALIZE_FLAGS;
    }

    pub fn call_bridge_u32(&mut self, offset: usize, arg: Value) {
        let arg32 = self.ensure_u32(arg);
        let touches_cpu_state = offset != bridge::offsets::X87_HELPER;
        if touches_cpu_state {
            self.flush_pending_flags();
            self.flush_gprs();
        }
        let fn_ptr = self.builder.ins().load(
            self.ptr_ty,
            MemFlags::trusted(),
            self.bridge_ptr,
            offset as i32,
        );
        self.builder
            .ins()
            .call_indirect(self.sig_cpu_u32, fn_ptr, &[self.cpu_ptr, arg32]);
        if touches_cpu_state {
            self.reload_gprs();
            self.flags_known_clean = false;
        }
    }

    pub fn call_bridge_u32_u32(&mut self, offset: usize, arg0: Value, arg1: Value) {
        let a0 = self.ensure_u32(arg0);
        let a1 = self.ensure_u32(arg1);
        self.flush_pending_flags();
        self.flush_gprs();
        let fn_ptr = self.builder.ins().load(
            self.ptr_ty,
            MemFlags::trusted(),
            self.bridge_ptr,
            offset as i32,
        );
        self.builder
            .ins()
            .call_indirect(self.sig_cpu_u32_u32, fn_ptr, &[self.cpu_ptr, a0, a1]);
        self.reload_gprs();
        self.flags_known_clean = false;
    }

    pub fn ensure_u32(&mut self, val: Value) -> Value {
        let ty = self.builder.func.dfg.value_type(val);
        if ty == I32 {
            val
        } else if ty.bits() < 32 {
            self.builder.ins().uextend(I32, val)
        } else {
            self.builder.ins().ireduce(I32, val)
        }
    }

    pub fn coerce_int(&mut self, val: Value, target_ty: Type) -> Value {
        let ty = self.builder.func.dfg.value_type(val);
        if ty == target_ty {
            val
        } else if ty.bits() < target_ty.bits() {
            self.builder.ins().uextend(target_ty, val)
        } else {
            self.builder.ins().ireduce(target_ty, val)
        }
    }
}
