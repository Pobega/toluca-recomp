pub mod control_flow;
pub mod flags;
pub mod fpu;
pub mod integer;
pub mod operands;
pub mod simd;
pub mod string_ops;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt;
use std::str::FromStr;
use std::sync::Arc;

use cranelift_codegen::Context;
use cranelift_codegen::FinalizedRelocTarget;
use cranelift_codegen::binemit::Reloc;
use cranelift_codegen::control::ControlPlane;
use cranelift_codegen::ir::condcodes::IntCC;
use cranelift_codegen::ir::types::I32;
use cranelift_codegen::ir::{
    AbiParam, Block, ExternalName, FuncRef, InstBuilder, MemFlags, SigRef, Signature, Type,
    UserExternalName, Value,
};
use cranelift_codegen::isa::{self, CallConv, TargetIsa};
use cranelift_codegen::settings::{self, Configurable};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext, Switch};
use iced_x86::{Decoder, DecoderOptions, FlowControl, Instruction, Mnemonic};
use toluca_core::GuestAddr;
use toluca_core::aot::{RelocKind, TargetArch};
use toluca_core::cpu::{ExitReason, offsets as cpu_offsets};
use target_lexicon::Triple;

use crate::cfg::{DiscoveredFunction, JumpTable, discover_functions};
use crate::pe::{
    DIR_IMPORT, DataDirectory, IMAGE_SCN_CNT_CODE, IMAGE_SCN_MEM_EXECUTE, IMAGE_SCN_MEM_READ,
    PeImage,
};

use self::control_flow::ControlFlowOutcome;

const SNIPPET_IMAGE_BASE: u32 = 0x0040_0000;
const SNIPPET_TEXT_RVA: u32 = 0x1000;
const PE_OFFSET: usize = 0x80;
const PE_OPT_SIZE: usize = 96 + 16 * 8;
const PE_SECTION_HEADER_SIZE: usize = 40;
const PE_FILE_ALIGNMENT: usize = 0x200;
const PE_SECTION_ALIGNMENT: u32 = 0x1000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LifterError {
    UnsupportedArch,
    CraneliftCompile(String),
    NoInstructions(GuestAddr),
    InvalidIsa(String),
}

impl fmt::Display for LifterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedArch => write!(f, "unsupported target architecture for Cranelift ISA"),
            Self::CraneliftCompile(msg) => write!(f, "Cranelift compilation failed: {msg}"),
            Self::NoInstructions(va) => write!(f, "no decodable instructions at 0x{va:08x}"),
            Self::InvalidIsa(msg) => write!(f, "failed to configure TargetIsa: {msg}"),
        }
    }
}

impl std::error::Error for LifterError {}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct InternalReloc {
    pub code_offset: u32,
    pub target_va: GuestAddr,
    pub kind: RelocKind,
    pub addend: i32,
}

#[derive(Clone, Debug)]
pub struct CompiledFunction {
    pub entry_va: GuestAddr,
    pub code: Vec<u8>,
    pub relocs: Vec<InternalReloc>,
    pub unsupported_count: usize,
}

pub fn build_isa(arch: TargetArch) -> Result<Arc<dyn TargetIsa>, LifterError> {
    let triple_str = match arch {
        TargetArch::X86_64 => "x86_64-unknown-linux-gnu",
        TargetArch::AArch64 => "aarch64-unknown-linux-gnu",
    };
    let triple =
        Triple::from_str(triple_str).map_err(|err| LifterError::InvalidIsa(err.to_string()))?;

    let mut flag_builder = settings::builder();
    flag_builder
        .set("opt_level", "speed")
        .map_err(|err| LifterError::InvalidIsa(err.to_string()))?;
    flag_builder
        .set("preserve_frame_pointers", "false")
        .map_err(|err| LifterError::InvalidIsa(err.to_string()))?;
    flag_builder
        .set("is_pic", "true")
        .map_err(|err| LifterError::InvalidIsa(err.to_string()))?;

    let flags = settings::Flags::new(flag_builder);
    let isa_builder =
        isa::lookup(triple).map_err(|err| LifterError::InvalidIsa(err.to_string()))?;
    isa_builder
        .finish(flags)
        .map_err(|err| LifterError::InvalidIsa(err.to_string()))
}

pub struct LifterContext<'ctx, 'b> {
    pub builder: &'b mut FunctionBuilder<'ctx>,
    pub ptr_ty: Type,
    pub cpu_ptr: Value,
    pub mem_base: Value,
    pub bridge_ptr: Value,
    pub call_conv: CallConv,
    pub image: &'b PeImage,
    pub func_blocks: &'b BTreeMap<GuestAddr, Block>,
    pub unwind_block: Block,
    pub jump_tables: &'b BTreeMap<GuestAddr, &'b JumpTable>,
    pub direct_call_refs: HashMap<GuestAddr, FuncRef>,
    pub sig_void_cpu: SigRef,
    pub sig_cpu_u32: SigRef,
    pub sig_cpu_u32_u32: SigRef,
    pub unsupported_count: usize,
}

impl<'ctx, 'b> LifterContext<'ctx, 'b> {
    pub fn get_or_declare_direct_call(&mut self, target_va: GuestAddr) -> FuncRef {
        if let Some(&func_ref) = self.direct_call_refs.get(&target_va) {
            return func_ref;
        }
        let user_ref = self
            .builder
            .func
            .declare_imported_user_function(UserExternalName {
                namespace: 0,
                index: target_va,
            });
        let func_ref =
            self.builder
                .import_function(cranelift_codegen::ir::ExtFuncData {
                    name: ExternalName::user(user_ref),
                    signature: self.sig_void_cpu,
                    colocated: true,
                    patchable: false,
                });
        self.direct_call_refs.insert(target_va, func_ref);
        func_ref
    }
}

pub fn lift_function(
    isa: &dyn TargetIsa,
    ctx: &mut Context,
    func_ctx: &mut FunctionBuilderContext,
    image: &PeImage,
    func: &DiscoveredFunction,
    jump_tables: &BTreeMap<GuestAddr, &JumpTable>,
    known_functions: &BTreeSet<GuestAddr>,
) -> Result<CompiledFunction, LifterError> {
    if func.blocks.is_empty() {
        return Err(LifterError::NoInstructions(func.entry_va));
    }

    ctx.clear();
    let ptr_ty = isa.pointer_type();
    let call_conv = isa.default_call_conv();

    ctx.func.signature = Signature {
        params: vec![AbiParam::new(ptr_ty)],
        returns: Vec::new(),
        call_conv,
    };

    let (split_vas, call_return_vas) = collect_split_vas(image, func, jump_tables);
    if !split_vas.contains(&func.entry_va) {
        return Err(LifterError::NoInstructions(func.entry_va));
    }

    let unsupported_count = {
        let mut builder = FunctionBuilder::new(&mut ctx.func, func_ctx);

        let sig_void_cpu = builder.import_signature(Signature {
            params: vec![AbiParam::new(ptr_ty)],
            returns: Vec::new(),
            call_conv,
        });
        let sig_cpu_u32 = builder.import_signature(Signature {
            params: vec![AbiParam::new(ptr_ty), AbiParam::new(I32)],
            returns: Vec::new(),
            call_conv,
        });
        let sig_cpu_u32_u32 = builder.import_signature(Signature {
            params: vec![
                AbiParam::new(ptr_ty),
                AbiParam::new(I32),
                AbiParam::new(I32),
            ],
            returns: Vec::new(),
            call_conv,
        });

        let mut func_blocks = BTreeMap::new();
        for &va in &split_vas {
            func_blocks.insert(va, builder.create_block());
        }
        let unwind_block = builder.create_block();

        let entry_block = builder.create_block();
        builder.append_block_params_for_function_params(entry_block);
        builder.switch_to_block(entry_block);
        let cpu_ptr = builder.block_params(entry_block)[0];
        let mem_base = builder.ins().load(
            ptr_ty,
            MemFlags::trusted(),
            cpu_ptr,
            cpu_offsets::MEM_BASE as i32,
        );
        let bridge_ptr = builder.ins().load(
            ptr_ty,
            MemFlags::trusted(),
            cpu_ptr,
            cpu_offsets::BRIDGE as i32,
        );
        let first_block = func_blocks[&func.entry_va];
        builder.ins().jump(first_block, &[]);

        let mut lifter = LifterContext {
            builder: &mut builder,
            ptr_ty,
            cpu_ptr,
            mem_base,
            bridge_ptr,
            call_conv,
            image,
            func_blocks: &func_blocks,
            unwind_block,
            jump_tables,
            direct_call_refs: HashMap::new(),
            sig_void_cpu,
            sig_cpu_u32,
            sig_cpu_u32_u32,
            unsupported_count: 0,
        };

        let split_list: Vec<GuestAddr> = split_vas.iter().copied().collect();
        for (idx, &start_va) in split_list.iter().enumerate() {
            let block = func_blocks[&start_va];
            lifter.builder.switch_to_block(block);

            let Some(containing_end) = containing_block_end(func, start_va) else {
                lifter.builder.ins().return_(&[]);
                continue;
            };
            let next_split = split_list.get(idx + 1).copied();
            let limit_va = next_split.map_or(containing_end, |s| s.min(containing_end));

            lift_split_block(
                &mut lifter,
                start_va,
                limit_va,
                known_functions,
            );
        }

        let count = lifter.unsupported_count;
        drop(lifter);
        emit_unwind_block(
            &mut builder,
            cpu_ptr,
            unwind_block,
            &func_blocks,
            &call_return_vas,
        );
        builder.seal_all_blocks();
        builder.finalize();
        count
    };

    let (code, pending_relocs) = {
        let mut ctrl_plane = ControlPlane::default();
        let compiled = ctx
            .compile(isa, &mut ctrl_plane)
            .map_err(|err| LifterError::CraneliftCompile(format!("{err:?}")))?;

        let code = compiled.code_buffer().to_vec();
        let mut pending = Vec::new();
        for mach_reloc in compiled.buffer.relocs() {
            if let FinalizedRelocTarget::ExternalName(ExternalName::LibCall(call)) =
                &mach_reloc.target
            {
                return Err(LifterError::CraneliftCompile(format!(
                    "unexpected Cranelift libcall relocation: {call:?}"
                )));
            }
            let Some(kind) = map_reloc_kind(mach_reloc.kind) else {
                continue;
            };
            if let FinalizedRelocTarget::ExternalName(ExternalName::User(user_ref)) =
                &mach_reloc.target
            {
                pending.push((mach_reloc.offset, *user_ref, kind, mach_reloc.addend as i32));
            }
        }
        (code, pending)
    };

    let user_funcs = ctx.func.params.user_named_funcs();
    let relocs = pending_relocs
        .into_iter()
        .map(|(code_offset, user_ref, kind, addend)| InternalReloc {
            code_offset,
            target_va: user_funcs[user_ref].index,
            kind,
            addend,
        })
        .collect();

    Ok(CompiledFunction {
        entry_va: func.entry_va,
        code,
        relocs,
        unsupported_count,
    })
}

pub fn lift_raw_snippet(
    isa: &dyn TargetIsa,
    entry_va: GuestAddr,
    code_bytes: &[u8],
) -> Result<CompiledFunction, LifterError> {
    let image_base = entry_va.saturating_sub(SNIPPET_TEXT_RVA);
    let text_rva = entry_va - image_base;
    let pe_bytes = build_snippet_pe32(image_base, text_rva, code_bytes);
    let image = PeImage::parse(pe_bytes)
        .map_err(|err| LifterError::CraneliftCompile(format!("snippet PE build failed: {err}")))?;
    let report = discover_functions(&image);
    let func = report
        .functions
        .get(&entry_va)
        .ok_or(LifterError::NoInstructions(entry_va))?;

    let mut jt_map = BTreeMap::new();
    for jt in &report.jump_tables {
        jt_map.insert(jt.site_va, jt);
    }
    let known: BTreeSet<GuestAddr> = report.functions.keys().copied().collect();

    let mut ctx = Context::new();
    let mut func_ctx = FunctionBuilderContext::new();
    lift_function(isa, &mut ctx, &mut func_ctx, &image, func, &jt_map, &known)
}

fn lift_split_block(
    lifter: &mut LifterContext<'_, '_>,
    start_va: GuestAddr,
    limit_va: GuestAddr,
    known_functions: &BTreeSet<GuestAddr>,
) {
    let byte_len = limit_va.saturating_sub(start_va) as usize;
    let Some(bytes) = lifter.image.read_bytes_at_va(start_va, byte_len) else {
        lifter.builder.ins().return_(&[]);
        return;
    };

    let mut decoder = Decoder::with_ip(32, bytes, start_va as u64, DecoderOptions::NONE);
    let mut instr = Instruction::default();
    let mut terminated = false;
    let mut current_end = start_va;

    while decoder.can_decode() {
        decoder.decode_out(&mut instr);
        if instr.is_invalid() {
            lifter.emit_unsupported(instr.ip() as GuestAddr);
            lifter.builder.ins().return_(&[]);
            terminated = true;
            break;
        }

        let next_va = (instr.ip() as GuestAddr).saturating_add(instr.len() as u32);
        current_end = next_va;

        match lifter.lift_control_flow(&instr, next_va, known_functions) {
            ControlFlowOutcome::Terminated => {
                terminated = true;
                break;
            }
            ControlFlowOutcome::Continue => {}
            ControlFlowOutcome::NotHandled => {
                if !lifter.lift_flag_instruction(&instr)
                    && !lifter.lift_integer_instruction(&instr)
                    && !lifter.lift_fpu(&instr)
                    && !lifter.lift_simd(&instr)
                    && !lifter.lift_string_op(&instr)
                {
                    lifter.emit_unsupported(instr.ip() as GuestAddr);
                }
            }
        }
    }

    if !terminated {
        if let Some(&next_block) = lifter.func_blocks.get(&current_end) {
            lifter.builder.ins().jump(next_block, &[]);
        } else {
            lifter.builder.ins().return_(&[]);
        }
    }
}

fn collect_split_vas(
    image: &PeImage,
    func: &DiscoveredFunction,
    jump_tables: &BTreeMap<GuestAddr, &JumpTable>,
) -> (BTreeSet<GuestAddr>, BTreeSet<GuestAddr>) {
    let mut splits = BTreeSet::new();
    let mut call_return_vas = BTreeSet::new();
    for &va in func.blocks.keys() {
        splits.insert(va);
    }

    for block in func.blocks.values() {
        let len = block.end_va.saturating_sub(block.start_va) as usize;
        let Some(bytes) = image.read_bytes_at_va(block.start_va, len) else {
            continue;
        };
        let mut decoder = Decoder::with_ip(32, bytes, block.start_va as u64, DecoderOptions::NONE);
        let mut instr = Instruction::default();

        while decoder.can_decode() {
            decoder.decode_out(&mut instr);
            if instr.is_invalid() {
                break;
            }
            let instr_va = instr.ip() as GuestAddr;
            let next_va = instr_va.saturating_add(instr.len() as u32);

            match instr.flow_control() {
                FlowControl::Call | FlowControl::IndirectCall => {
                    if contains_va_in_func(func, next_va) {
                        splits.insert(next_va);
                        call_return_vas.insert(next_va);
                    }
                }
                FlowControl::ConditionalBranch => {
                    let target = instr.near_branch32();
                    if contains_va_in_func(func, target) {
                        splits.insert(target);
                    }
                    if contains_va_in_func(func, next_va) {
                        splits.insert(next_va);
                    }
                }
                FlowControl::UnconditionalBranch => {
                    let target = instr.near_branch32();
                    if contains_va_in_func(func, target) {
                        splits.insert(target);
                    }
                }
                FlowControl::IndirectBranch => {
                    if let Some(jt) = jump_tables.get(&instr_va) {
                        for &target in &jt.targets {
                            if contains_va_in_func(func, target) {
                                splits.insert(target);
                            }
                        }
                    }
                }
                _ if matches!(
                    instr.mnemonic(),
                    Mnemonic::Jecxz | Mnemonic::Jcxz | Mnemonic::Loop | Mnemonic::Loope | Mnemonic::Loopne
                ) =>
                {
                    let target = instr.near_branch32();
                    if contains_va_in_func(func, target) {
                        splits.insert(target);
                    }
                    if contains_va_in_func(func, next_va) {
                        splits.insert(next_va);
                    }
                }
                _ => {}
            }
        }
    }

    (splits, call_return_vas)
}

fn emit_unwind_block(
    builder: &mut FunctionBuilder<'_>,
    cpu_ptr: Value,
    unwind_block: Block,
    func_blocks: &BTreeMap<GuestAddr, Block>,
    call_return_vas: &BTreeSet<GuestAddr>,
) {
    builder.switch_to_block(unwind_block);
    let resume_targets: Vec<(GuestAddr, Block)> = call_return_vas
        .iter()
        .filter_map(|va| func_blocks.get(va).map(|&b| (*va, b)))
        .collect();
    if resume_targets.is_empty() {
        builder.ins().return_(&[]);
        return;
    }

    let exit_reason = builder.ins().load(
        I32,
        MemFlags::trusted(),
        cpu_ptr,
        cpu_offsets::EXIT_REASON as i32,
    );
    let is_longjmp = builder.ins().icmp_imm(
        IntCC::Equal,
        exit_reason,
        ExitReason::Longjmp as i64,
    );
    let return_block = builder.create_block();
    let dispatch_block = builder.create_block();
    builder
        .ins()
        .brif(is_longjmp, dispatch_block, &[], return_block, &[]);

    builder.switch_to_block(return_block);
    builder.ins().return_(&[]);

    builder.switch_to_block(dispatch_block);
    let eip = builder
        .ins()
        .load(I32, MemFlags::trusted(), cpu_ptr, cpu_offsets::EIP as i32);
    let running = builder.ins().iconst(I32, ExitReason::Running as i64);
    let mut switch = Switch::new();
    let mut resume_blocks = Vec::with_capacity(resume_targets.len());
    for (ret_va, target_block) in resume_targets {
        let resume_block = builder.create_block();
        switch.set_entry(ret_va as u128, resume_block);
        resume_blocks.push((resume_block, target_block));
    }
    switch.emit(builder, eip, return_block);

    for (resume_block, target_block) in resume_blocks {
        builder.switch_to_block(resume_block);
        builder.ins().store(
            MemFlags::trusted(),
            running,
            cpu_ptr,
            cpu_offsets::EXIT_REASON as i32,
        );
        builder.ins().jump(target_block, &[]);
    }
}

fn contains_va_in_func(func: &DiscoveredFunction, va: GuestAddr) -> bool {
    func.blocks
        .values()
        .any(|block| va >= block.start_va && va < block.end_va)
}

fn containing_block_end(func: &DiscoveredFunction, va: GuestAddr) -> Option<GuestAddr> {
    func.blocks
        .values()
        .find(|block| va >= block.start_va && va < block.end_va)
        .map(|block| block.end_va)
}

const fn map_reloc_kind(reloc: Reloc) -> Option<RelocKind> {
    match reloc {
        Reloc::X86CallPCRel4 => Some(RelocKind::X86_64Rel32),
        Reloc::Arm64Call => Some(RelocKind::AArch64Call26),
        _ => None,
    }
}

fn build_snippet_pe32(image_base: u32, text_rva: u32, code: &[u8]) -> Vec<u8> {
    let sec_table_offset = PE_OFFSET + 4 + 20 + PE_OPT_SIZE;
    let headers_end = sec_table_offset + PE_SECTION_HEADER_SIZE;
    let size_of_headers = headers_end.div_ceil(PE_FILE_ALIGNMENT) * PE_FILE_ALIGNMENT;
    let raw_code_size = code.len().max(1).div_ceil(PE_FILE_ALIGNMENT) * PE_FILE_ALIGNMENT;

    let mut buf = vec![0u8; size_of_headers + raw_code_size];
    buf[0..2].copy_from_slice(&crate::pe::DOS_MAGIC.to_le_bytes());
    buf[0x3C..0x40].copy_from_slice(&(PE_OFFSET as u32).to_le_bytes());

    buf[PE_OFFSET..PE_OFFSET + 4].copy_from_slice(&crate::pe::PE_SIGNATURE);
    let coff = PE_OFFSET + 4;
    buf[coff..coff + 2].copy_from_slice(&crate::pe::IMAGE_FILE_MACHINE_I386.to_le_bytes());
    buf[coff + 2..coff + 4].copy_from_slice(&1u16.to_le_bytes());
    buf[coff + 16..coff + 18].copy_from_slice(&(PE_OPT_SIZE as u16).to_le_bytes());
    buf[coff + 18..coff + 20].copy_from_slice(&0x0102u16.to_le_bytes());

    let opt = coff + 20;
    buf[opt..opt + 2].copy_from_slice(&crate::pe::PE32_OPTIONAL_MAGIC.to_le_bytes());
    buf[opt + 16..opt + 20].copy_from_slice(&text_rva.to_le_bytes());
    buf[opt + 28..opt + 32].copy_from_slice(&image_base.to_le_bytes());
    buf[opt + 32..opt + 36].copy_from_slice(&PE_SECTION_ALIGNMENT.to_le_bytes());
    buf[opt + 36..opt + 40].copy_from_slice(&(PE_FILE_ALIGNMENT as u32).to_le_bytes());
    let size_of_image = (text_rva + (code.len() as u32).max(PE_SECTION_ALIGNMENT))
        .div_ceil(PE_SECTION_ALIGNMENT)
        * PE_SECTION_ALIGNMENT;
    buf[opt + 56..opt + 60].copy_from_slice(&size_of_image.to_le_bytes());
    buf[opt + 60..opt + 64].copy_from_slice(&(size_of_headers as u32).to_le_bytes());
    buf[opt + 92..opt + 96].copy_from_slice(&16u32.to_le_bytes());

    let sec = sec_table_offset;
    buf[sec..sec + 5].copy_from_slice(b".text");
    buf[sec + 8..sec + 12].copy_from_slice(&(code.len() as u32).to_le_bytes());
    buf[sec + 12..sec + 16].copy_from_slice(&text_rva.to_le_bytes());
    buf[sec + 16..sec + 20].copy_from_slice(&(raw_code_size as u32).to_le_bytes());
    buf[sec + 20..sec + 24].copy_from_slice(&(size_of_headers as u32).to_le_bytes());
    let chars = IMAGE_SCN_CNT_CODE | IMAGE_SCN_MEM_EXECUTE | IMAGE_SCN_MEM_READ;
    buf[sec + 36..sec + 40].copy_from_slice(&chars.to_le_bytes());

    buf[size_of_headers..size_of_headers + code.len()].copy_from_slice(code);

    let _ = (SNIPPET_IMAGE_BASE, DIR_IMPORT, DataDirectory::default());
    buf
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pe::test_helpers::{TestSectionSpec, build_test_pe32};
    use toluca_core::bridge::{HostBridge, RecompiledFn};
    use toluca_core::cpu::{EFLAGS_CF, EFLAGS_ZF, Gpr, GuestCpuState};

    struct ExecBuffer {
        ptr: *mut u8,
        len: usize,
    }

    impl ExecBuffer {
        fn new(code: &[u8]) -> Self {
            let len = code.len().max(4096);
            let ptr = unsafe {
                libc::mmap(
                    core::ptr::null_mut(),
                    len,
                    libc::PROT_READ | libc::PROT_WRITE,
                    libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
                    -1,
                    0,
                )
            };
            assert_ne!(ptr, libc::MAP_FAILED, "mmap failed");
            unsafe {
                core::ptr::copy_nonoverlapping(code.as_ptr(), ptr.cast::<u8>(), code.len());
                let rc = libc::mprotect(ptr, len, libc::PROT_READ | libc::PROT_EXEC);
                assert_eq!(rc, 0, "mprotect RX failed");
                toluca_core::aot::flush_icache(ptr.cast::<u8>(), code.len());
            }
            Self {
                ptr: ptr.cast::<u8>(),
                len,
            }
        }

        fn as_fn(&self) -> RecompiledFn {
            unsafe { core::mem::transmute::<*const u8, RecompiledFn>(self.ptr) }
        }
    }

    impl Drop for ExecBuffer {
        fn drop(&mut self) {
            unsafe {
                libc::munmap(self.ptr.cast::<libc::c_void>(), self.len);
            }
        }
    }

    fn execute_snippet(code: &[u8], init: impl FnOnce(&mut GuestCpuState, &mut [u8])) -> GuestCpuState {
        let arch = TargetArch::current().expect("host arch supported");
        let isa = build_isa(arch).unwrap();
        let compiled = lift_raw_snippet(isa.as_ref(), 0x0040_1000, code).unwrap();
        assert_eq!(compiled.unsupported_count, 0);

        let mut arena = vec![0u8; 64 * 1024];
        let mut bridge = HostBridge::new_stub();
        bridge.x87_helper = toluca_core::bridge::default_x87_helper;
        let mut cpu = GuestCpuState::new(arena.as_mut_ptr(), &bridge);
        cpu.set_gpr(Gpr::Esp, 0x8000);
        cpu.fs_base = 0x2000;
        init(&mut cpu, &mut arena);

        let exec = ExecBuffer::new(&compiled.code);
        unsafe {
            (exec.as_fn())(&mut cpu);
        }
        cpu.materialize_flags();
        cpu
    }

    #[test]
    fn cross_compile_aarch64_smoke_test() {
        let isa = build_isa(TargetArch::AArch64).unwrap();
        // mov eax, 42; add eax, 8; ret
        let code = [0xB8, 0x2A, 0x00, 0x00, 0x00, 0x83, 0xC0, 0x08, 0xC3];
        let compiled = lift_raw_snippet(isa.as_ref(), 0x0040_1000, &code).unwrap();
        assert!(!compiled.code.is_empty());
        assert_eq!(compiled.unsupported_count, 0);
    }

    #[test]
    fn executes_arithmetic_subregs_and_div() {
        // mov eax, 0x12345678
        // mov al, 0xAB
        // mov ah, 0xCD
        // mov ecx, 0x100
        // add ax, cx
        // inc eax
        // imul ebx, eax, 3
        // neg edx
        // ret
        #[rustfmt::skip]
        let code = [
            0xB8, 0x78, 0x56, 0x34, 0x12, // mov eax, 0x12345678
            0xB0, 0xAB,                   // mov al, 0xAB
            0xB4, 0xCD,                   // mov ah, 0xCD -> eax = 0x1234CDAB
            0xB9, 0x00, 0x01, 0x00, 0x00, // mov ecx, 0x100
            0x66, 0x01, 0xC8,             // add ax, cx -> ax = 0xCEAB, eax = 0x1234CEAB
            0x40,                         // inc eax -> 0x1234CEAC
            0x6B, 0xD8, 0x03,             // imul ebx, eax, 3
            0xF7, 0xDA,                   // neg edx (edx=5 -> -5, CF=1)
            0xC3,                         // ret
        ];
        let cpu = execute_snippet(&code, |cpu, _| {
            cpu.set_gpr(Gpr::Edx, 5);
        });
        assert_eq!(cpu.gpr(Gpr::Eax), 0x1234_CEAC);
        assert_eq!(cpu.gpr(Gpr::Edx), (-5i32) as u32);
        assert_eq!(cpu.gpr(Gpr::Ebx), 0x1234_CEACu32.wrapping_mul(3));
        assert_ne!(cpu.eflags & EFLAGS_CF, 0); // neg 5 sets CF=1

        // Unsigned/signed div: eax = 100, edx = 0, ecx = 7 -> div ecx -> eax=14, edx=2
        #[rustfmt::skip]
        let div_code = [
            0xB8, 0x64, 0x00, 0x00, 0x00, // mov eax, 100
            0x33, 0xD2,                   // xor edx, edx
            0xB9, 0x07, 0x00, 0x00, 0x00, // mov ecx, 7
            0xF7, 0xF1,                   // div ecx
            0xC3,
        ];
        let div_cpu = execute_snippet(&div_code, |_, _| {});
        assert_eq!(div_cpu.gpr(Gpr::Eax), 14);
        assert_eq!(div_cpu.gpr(Gpr::Edx), 2);
    }

    #[test]
    fn executes_shifts_and_shift_by_zero_preserves_flags() {
        // stc; mov cl, 0; shl eax, cl; ret -> CF must remain 1!
        #[rustfmt::skip]
        let code = [
            0xF9,             // stc
            0xB1, 0x00,       // mov cl, 0
            0xD3, 0xE0,       // shl eax, cl
            0xC3,             // ret
        ];
        let cpu = execute_snippet(&code, |cpu, _| {
            cpu.set_gpr(Gpr::Eax, 0x1234);
        });
        assert_eq!(cpu.gpr(Gpr::Eax), 0x1234);
        assert_ne!(cpu.eflags & EFLAGS_CF, 0);

        // bswap eax; bsf ecx, edx; bsr esi, edx; ret
        #[rustfmt::skip]
        let bit_code = [
            0x0F, 0xC8,       // bswap eax
            0x0F, 0xBC, 0xCA, // bsf ecx, edx
            0x0F, 0xBD, 0xF2, // bsr esi, edx
            0xC3,
        ];
        let bit_cpu = execute_snippet(&bit_code, |cpu, _| {
            cpu.set_gpr(Gpr::Eax, 0x1122_3344);
            cpu.set_gpr(Gpr::Edx, 0x0010_0008); // lowest bit 3, highest bit 20
        });
        assert_eq!(bit_cpu.gpr(Gpr::Eax), 0x4433_2211);
        assert_eq!(bit_cpu.gpr(Gpr::Ecx), 3);
        assert_eq!(bit_cpu.gpr(Gpr::Esi), 20);
        assert_eq!(bit_cpu.eflags & EFLAGS_ZF, 0);
    }

    #[test]
    fn executes_loop_conditional_branches_setcc_cmov_and_fs_seh() {
        // Sum 1..=5 in EAX using ECX loop, then read fs:[0] into EDX, test setne/cmovg, leave/ret
        // push ebp; mov ebp, esp
        // xor eax, eax
        // mov ecx, 5
        // loop_top: add eax, ecx; dec ecx; jnz loop_top
        // mov edx, fs:[0]
        // cmp eax, 15
        // sete bl
        // mov esi, 99
        // cmove edi, esi
        // leave
        // ret
        #[rustfmt::skip]
        let code = [
            0x55,                         // push ebp
            0x8B, 0xEC,                   // mov ebp, esp
            0x33, 0xC0,                   // xor eax, eax
            0xB9, 0x05, 0x00, 0x00, 0x00, // mov ecx, 5
            // loop_top (+10):
            0x03, 0xC1,                   // add eax, ecx
            0x49,                         // dec ecx
            0x75, 0xFB,                   // jnz -5 (back to +10)
            0x64, 0x8B, 0x15, 0x00, 0x00, 0x00, 0x00, // mov edx, fs:[0]
            0x83, 0xF8, 0x0F,             // cmp eax, 15
            0x0F, 0x94, 0xC3,             // sete bl
            0xBE, 0x63, 0x00, 0x00, 0x00, // mov esi, 99
            0x0F, 0x44, 0xFE,             // cmove edi, esi
            0xC9,                         // leave
            0xC3,                         // ret
        ];
        let cpu = execute_snippet(&code, |_cpu, arena| {
            // Put SEH head at fs:[0] = arena[0x2000..0x2004]
            arena[0x2000..0x2004].copy_from_slice(&0xCAFE_BABEu32.to_le_bytes());
            // Put return address on stack at esp=0x8000
            arena[0x8000..0x8004].copy_from_slice(&0x0040_9999u32.to_le_bytes());
        });
        assert_eq!(cpu.gpr(Gpr::Eax), 15);
        assert_eq!(cpu.gpr(Gpr::Edx), 0xCAFE_BABE);
        assert_eq!(cpu.gpr(Gpr::Ebx) & 0xFF, 1);
        assert_eq!(cpu.gpr(Gpr::Edi), 99);
        assert_eq!(cpu.eip, 0x0040_9999);
        assert_eq!(cpu.gpr(Gpr::Esp), 0x8004);
    }

    unsafe extern "C" fn test_call_import(cpu: *mut GuestCpuState, import_index: u32) {
        let state = unsafe { &mut *cpu };
        // Pop return address (like stdcall with 0 args) and return 0x777 + import_index in EAX
        let esp = state.gpr(Gpr::Esp);
        let ret_bytes = unsafe {
            core::slice::from_raw_parts(state.mem_base.add(esp as usize), 4)
        };
        let ret_va = u32::from_le_bytes([ret_bytes[0], ret_bytes[1], ret_bytes[2], ret_bytes[3]]);
        state.set_gpr(Gpr::Esp, esp + 4);
        state.eip = ret_va;
        state.set_gpr(Gpr::Eax, 0x777 + import_index);
    }

    #[test]
    fn executes_direct_call_stdcall_ret_and_iat_import() {
        let image_base = 0x0040_0000u32;
        let text_rva = 0x1000u32;
        let rdata_rva = 0x2000u32;
        let iat_va = image_base + rdata_rva + 0x70;

        // entry (+0):
        //   push 10; call callee (+0x20)
        //   add eax, 5
        //   call dword ptr [iat_va]
        //   ret
        // callee (+0x20):
        //   mov eax, [esp+4]
        //   add eax, 20
        //   ret 4
        let mut text = vec![0xCCu8; 0x40];
        // push 10 (6A 0A)
        text[0..2].copy_from_slice(&[0x6A, 0x0A]);
        // call +0x20 (rel = 0x20 - 7 = 0x19)
        text[2..7].copy_from_slice(&[0xE8, 0x19, 0x00, 0x00, 0x00]);
        // add eax, 5 (83 C0 05)
        text[7..10].copy_from_slice(&[0x83, 0xC0, 0x05]);
        // call [iat_va] (FF 15 <iat_va>)
        text[10..12].copy_from_slice(&[0xFF, 0x15]);
        text[12..16].copy_from_slice(&iat_va.to_le_bytes());
        // ret
        text[16] = 0xC3;

        // callee at 0x20:
        // mov eax, [esp+4] (8B 44 24 04)
        text[0x20..0x24].copy_from_slice(&[0x8B, 0x44, 0x24, 0x04]);
        // add eax, 20 (83 C0 14)
        text[0x24..0x27].copy_from_slice(&[0x83, 0xC0, 0x14]);
        // ret 4 (C2 04 00)
        text[0x27..0x2A].copy_from_slice(&[0xC2, 0x04, 0x00]);

        let mut rdata = vec![0u8; 256];
        rdata[0..4].copy_from_slice(&(rdata_rva + 0x60).to_le_bytes());
        rdata[12..16].copy_from_slice(&(rdata_rva + 0x80).to_le_bytes());
        rdata[16..20].copy_from_slice(&(rdata_rva + 0x70).to_le_bytes());
        rdata[0x60..0x64].copy_from_slice(&(rdata_rva + 0xB0).to_le_bytes());
        rdata[0x80..0x8D].copy_from_slice(b"kernel32.dll\0");
        rdata[0xB2..0xBF].copy_from_slice(b"GetTickCount\0");

        let pe_bytes = build_test_pe32(
            image_base,
            text_rva,
            0x0102,
            &[(DIR_IMPORT, DataDirectory { rva: rdata_rva, size: 40 })],
            &[
                TestSectionSpec {
                    name: ".text",
                    virtual_address: text_rva,
                    characteristics: IMAGE_SCN_CNT_CODE | IMAGE_SCN_MEM_EXECUTE | IMAGE_SCN_MEM_READ,
                    data: &text,
                },
                TestSectionSpec {
                    name: ".rdata",
                    virtual_address: rdata_rva,
                    characteristics: IMAGE_SCN_MEM_READ,
                    data: &rdata,
                },
            ],
        );

        let image = PeImage::parse(pe_bytes).unwrap();
        let report = discover_functions(&image);
        let arch = TargetArch::current().unwrap();
        let isa = build_isa(arch).unwrap();

        let known: BTreeSet<GuestAddr> = report.functions.keys().copied().collect();
        let jt_map = BTreeMap::new();
        let mut ctx = Context::new();
        let mut fctx = FunctionBuilderContext::new();

        let entry_va = image_base + text_rva;
        let callee_va = entry_va + 0x20;
        let comp_callee = lift_function(
            isa.as_ref(),
            &mut ctx,
            &mut fctx,
            &image,
            &report.functions[&callee_va],
            &jt_map,
            &known,
        )
        .unwrap();
        let comp_entry = lift_function(
            isa.as_ref(),
            &mut ctx,
            &mut fctx,
            &image,
            &report.functions[&entry_va],
            &jt_map,
            &known,
        )
        .unwrap();

        assert!(comp_entry.relocs.iter().any(|r| r.target_va == callee_va));

        // Verify callee executes standalone with stdcall ret 4: (10 + 20) == 30 and ESP popped 8
        let mut arena = vec![0u8; 64 * 1024];
        let mut bridge = HostBridge::new_stub();
        bridge.call_import = test_call_import;
        let mut cpu = GuestCpuState::new(arena.as_mut_ptr(), &bridge);
        cpu.set_gpr(Gpr::Esp, 0x8000);
        arena[0x8000..0x8004].copy_from_slice(&0x1111_2222u32.to_le_bytes());
        arena[0x8004..0x8008].copy_from_slice(&10u32.to_le_bytes());

        let exec_callee = ExecBuffer::new(&comp_callee.code);
        unsafe {
            (exec_callee.as_fn())(&mut cpu);
        }
        assert_eq!(cpu.gpr(Gpr::Eax), 30);
        assert_eq!(cpu.eip, 0x1111_2222);
        assert_eq!(cpu.gpr(Gpr::Esp), 0x8008);

        // Verify entry patched with callee relocation runs end-to-end and calls IAT import
        let total_len = 0x2000usize;
        let mut combined_buf = vec![0u8; total_len];
        combined_buf[..comp_entry.code.len()].copy_from_slice(&comp_entry.code);
        let callee_off = 0x1000usize;
        combined_buf[callee_off..callee_off + comp_callee.code.len()]
            .copy_from_slice(&comp_callee.code);

        for rel in &comp_entry.relocs {
            if rel.target_va == callee_va && rel.kind == RelocKind::X86_64Rel32 {
                let off = rel.code_offset as usize;
                let disp = (callee_off as i64 - off as i64 + rel.addend as i64) as i32;
                combined_buf[off..off + 4].copy_from_slice(&disp.to_le_bytes());
            }
        }

        let exec_combined = ExecBuffer::new(&combined_buf);
        let mut cpu2 = GuestCpuState::new(arena.as_mut_ptr(), &bridge);
        cpu2.set_gpr(Gpr::Esp, 0x8000);
        arena[0x8000..0x8004].copy_from_slice(&0x3333_4444u32.to_le_bytes());
        unsafe {
            (exec_combined.as_fn())(&mut cpu2);
        }
        // call_import returns 0x777 + 0 (import_index = 0)
        assert_eq!(cpu2.gpr(Gpr::Eax), 0x777);
        assert_eq!(cpu2.eip, 0x3333_4444);
        assert_eq!(cpu2.gpr(Gpr::Esp), 0x8004);
    }

    #[test]
    fn executes_x87_fpu_arithmetic_comparisons_and_transcendentals() {
        // Arena addresses used:
        //   0x4000: f32 (3.0)
        //   0x4004: i32 (7)
        //   0x4008: i32 output from fistp
        //   0x400C: f64 (0.0) for fcos check
        //   0x4014: f64 output from fstp
        // Sequence:
        //   fld dword ptr [0x4000]  -> st0 = 3.0
        //   fild dword ptr [0x4004] -> st0 = 7.0, st1 = 3.0
        //   fadd st(0), st(1)       -> st0 = 10.0, st1 = 3.0
        //   fmul st(0), st(1)       -> st0 = 30.0, st1 = 3.0
        //   fsub st(0), st(1)       -> st0 = 27.0, st1 = 3.0
        //   fdiv st(0), st(1)       -> st0 = 9.0, st1 = 3.0
        //   fsqrt                   -> st0 = 3.0, st1 = 3.0
        //   fcomip st(0), st(1)     -> compare 3.0 == 3.0 (ZF=1, CF=0), pops -> st0 = 3.0
        //   jne fail
        //   fnstsw ax
        //   fistp dword ptr [0x4008] -> [0x4008] = 3
        //   fld qword ptr [0x400C]   -> st0 = 0.0
        //   fcos                     -> st0 = 1.0
        //   fld1                     -> st0 = 1.0, st1 = 1.0
        //   faddp st(1), st(0)       -> st0 = 2.0
        //   fstp qword ptr [0x4014]  -> [0x4014] = 2.0
        //   mov ebx, 1
        //   ret
        // fail:
        //   mov ebx, 0
        //   ret
        #[rustfmt::skip]
        let code = [
            0xD9, 0x05, 0x00, 0x40, 0x00, 0x00, // fld dword ptr [0x4000]
            0xDB, 0x05, 0x04, 0x40, 0x00, 0x00, // fild dword ptr [0x4004]
            0xD8, 0xC1,                         // fadd st(0), st(1)
            0xD8, 0xC9,                         // fmul st(0), st(1)
            0xD8, 0xE1,                         // fsub st(0), st(1)
            0xD8, 0xF1,                         // fdiv st(0), st(1)
            0xD9, 0xFA,                         // fsqrt
            0xDF, 0xF1,                         // fcomip st(0), st(1)
            0x75, 0x20,                         // jne +32 -> fail
            0xDF, 0xE0,                         // fnstsw ax
            0xDB, 0x1D, 0x08, 0x40, 0x00, 0x00, // fistp dword ptr [0x4008]
            0xDD, 0x05, 0x0C, 0x40, 0x00, 0x00, // fld qword ptr [0x400C]
            0xD9, 0xFF,                         // fcos
            0xD9, 0xE8,                         // fld1
            0xDE, 0xC1,                         // faddp st(1), st(0)
            0xDD, 0x1D, 0x14, 0x40, 0x00, 0x00, // fstp qword ptr [0x4014]
            0xBB, 0x01, 0x00, 0x00, 0x00,       // mov ebx, 1
            0xC3,                               // ret
            // fail:
            0xBB, 0x00, 0x00, 0x00, 0x00,       // mov ebx, 0
            0xC3,                               // ret
        ];

        let sin_code = [
            0xDD, 0x05, 0x00, 0x40, 0x00, 0x00, // fld qword ptr [0x4000]
            0xD9, 0xFE,                         // fsin
            0xC3,
        ];
        let sin_cpu = execute_snippet(&sin_code, |_cpu, arena| {
            arena[0x4000..0x4008].copy_from_slice(&(core::f64::consts::FRAC_PI_2).to_le_bytes());
        });
        assert!((sin_cpu.x87.st(0) - 1.0).abs() < 1e-12);

        let cpu = execute_snippet(&code, |_cpu, arena| {
            arena[0x4000..0x4004].copy_from_slice(&3.0f32.to_le_bytes());
            arena[0x4004..0x4008].copy_from_slice(&7i32.to_le_bytes());
            arena[0x400C..0x4014].copy_from_slice(&0.0f64.to_le_bytes());
        });
        assert_eq!(cpu.gpr(Gpr::Ebx), 1);
        assert_ne!(cpu.eflags & EFLAGS_ZF, 0);
        // fnstsw ax saw top=7 (after fcomip popped once from top=6)
        assert_eq!((cpu.gpr(Gpr::Eax) as u16 & 0x3800) >> 11, 7);

        let mut arena_check = vec![0u8; 64 * 1024];
        arena_check[0x4000..0x4004].copy_from_slice(&3.0f32.to_le_bytes());
        arena_check[0x4004..0x4008].copy_from_slice(&7i32.to_le_bytes());
        arena_check[0x400C..0x4014].copy_from_slice(&0.0f64.to_le_bytes());
        let arch = TargetArch::current().unwrap();
        let isa = build_isa(arch).unwrap();
        let compiled = lift_raw_snippet(isa.as_ref(), 0x0040_1000, &code).unwrap();
        let mut bridge = HostBridge::new_stub();
        bridge.x87_helper = toluca_core::bridge::default_x87_helper;
        let mut cpu2 = GuestCpuState::new(arena_check.as_mut_ptr(), &bridge);
        cpu2.set_gpr(Gpr::Esp, 0x8000);
        let exec = ExecBuffer::new(&compiled.code);
        unsafe {
            (exec.as_fn())(&mut cpu2);
        }
        let int_out = i32::from_le_bytes(arena_check[0x4008..0x400C].try_into().unwrap());
        let f64_out = f64::from_le_bytes(arena_check[0x4014..0x401C].try_into().unwrap());
        assert_eq!(int_out, 3);
        assert!((f64_out - 2.0).abs() < 1e-12);
        assert_eq!(cpu2.x87.top, 0);
    }

    #[test]
    fn executes_sse_and_mmx_operations() {
        // Memory:
        //   0x4000: [1.0, 2.0, 3.0, 4.0]
        //   0x4010: [10.0, 20.0, 30.0, 40.0]
        //   0x4020: MMX [100u32, 200u32]
        // Sequence:
        //   movaps xmm0, [0x4000]
        //   movaps xmm1, [0x4010]
        //   addps xmm0, xmm1       -> [11, 22, 33, 44]
        //   shufps xmm0, xmm0, 0x1B -> reverse [44, 33, 22, 11]
        //   mulps xmm0, [0x4000]   -> [44, 66, 66, 44]
        //   xorps xmm2, xmm2
        //   mov eax, 7
        //   cvtsi2ss xmm2, eax     -> xmm2[0] = 7.0
        //   ucomiss xmm0, xmm2     -> 44.0 > 7.0 (ZF=0, CF=0)
        //   cvttss2si ecx, xmm0    -> ecx = 44
        //   movq mm0, [0x4020]
        //   paddd mm0, mm0         -> [200, 400]
        //   pxor mm1, mm1
        //   emms
        //   ret
        #[rustfmt::skip]
        let code = [
            0x0F, 0x28, 0x05, 0x00, 0x40, 0x00, 0x00, // movaps xmm0, [0x4000]
            0x0F, 0x28, 0x0D, 0x10, 0x40, 0x00, 0x00, // movaps xmm1, [0x4010]
            0x0F, 0x58, 0xC1,                         // addps xmm0, xmm1
            0x0F, 0xC6, 0xC0, 0x1B,                   // shufps xmm0, xmm0, 0x1B
            0x0F, 0x59, 0x05, 0x00, 0x40, 0x00, 0x00, // mulps xmm0, [0x4000]
            0x0F, 0x57, 0xD2,                         // xorps xmm2, xmm2
            0xB8, 0x07, 0x00, 0x00, 0x00,             // mov eax, 7
            0xF3, 0x0F, 0x2A, 0xD0,                   // cvtsi2ss xmm2, eax
            0x0F, 0x2E, 0xC2,                         // ucomiss xmm0, xmm2
            0xF3, 0x0F, 0x2C, 0xC8,                   // cvttss2si ecx, xmm0
            0x0F, 0x6F, 0x05, 0x20, 0x40, 0x00, 0x00, // movq mm0, [0x4020]
            0x0F, 0xFE, 0xC0,                         // paddd mm0, mm0
            0x0F, 0xEF, 0xC9,                         // pxor mm1, mm1
            0x0F, 0x77,                               // emms
            0xC3,                                     // ret
        ];

        let cpu = execute_snippet(&code, |cpu, arena| {
            cpu.x87.tag_word = 0x0000;
            for (i, v) in [1.0f32, 2.0, 3.0, 4.0].iter().enumerate() {
                arena[0x4000 + i * 4..0x4004 + i * 4].copy_from_slice(&v.to_le_bytes());
            }
            for (i, v) in [10.0f32, 20.0, 30.0, 40.0].iter().enumerate() {
                arena[0x4010 + i * 4..0x4014 + i * 4].copy_from_slice(&v.to_le_bytes());
            }
            arena[0x4020..0x4024].copy_from_slice(&100u32.to_le_bytes());
            arena[0x4024..0x4028].copy_from_slice(&200u32.to_le_bytes());
        });

        assert_eq!(cpu.xmm[0].to_f32x4(), [44.0, 66.0, 66.0, 44.0]);
        assert_eq!(cpu.xmm[2].to_f32x4(), [7.0, 0.0, 0.0, 0.0]);
        assert_eq!(cpu.gpr(Gpr::Ecx), 44);
        assert_eq!(cpu.eflags & (EFLAGS_ZF | EFLAGS_CF), 0);
        assert_eq!(cpu.mm[0], (400u64 << 32) | 200u64);
        assert_eq!(cpu.mm[1], 0);
        assert_eq!(cpu.x87.tag_word, toluca_core::cpu::X87_DEFAULT_TAG_WORD);
    }

    #[test]
    fn executes_string_operations_forward_reverse_and_rep_variants() {
        let arch = TargetArch::current().unwrap();
        let isa = build_isa(arch).unwrap();
        let bridge = HostBridge::new_stub();

        // 1. cld; rep movsd (forward copy 3 dwords)
        let movsd_code = [0xFC, 0xF3, 0xA5, 0xC3];
        let mut arena_fwd = vec![0u8; 64 * 1024];
        arena_fwd[0x4000..0x4004].copy_from_slice(&0x1111_2222u32.to_le_bytes());
        arena_fwd[0x4004..0x4008].copy_from_slice(&0x3333_4444u32.to_le_bytes());
        arena_fwd[0x4008..0x400C].copy_from_slice(&0x5555_6666u32.to_le_bytes());
        let comp_movsd = lift_raw_snippet(isa.as_ref(), 0x0040_1000, &movsd_code).unwrap();
        let mut cpu = GuestCpuState::new(arena_fwd.as_mut_ptr(), &bridge);
        cpu.set_gpr(Gpr::Esp, 0x8000);
        cpu.set_gpr(Gpr::Esi, 0x4000);
        cpu.set_gpr(Gpr::Edi, 0x5000);
        cpu.set_gpr(Gpr::Ecx, 3);
        let exec_movsd = ExecBuffer::new(&comp_movsd.code);
        unsafe {
            (exec_movsd.as_fn())(&mut cpu);
        }
        assert_eq!(cpu.gpr(Gpr::Ecx), 0);
        assert_eq!(cpu.gpr(Gpr::Esi), 0x400C);
        assert_eq!(cpu.gpr(Gpr::Edi), 0x500C);
        assert_eq!(&arena_fwd[0x5000..0x500C], &arena_fwd[0x4000..0x400C]);

        // 2. std; rep movsb (reverse copy 4 bytes)
        let rev_code = [0xFD, 0xF3, 0xA4, 0xFC, 0xC3];
        let comp_rev = lift_raw_snippet(isa.as_ref(), 0x0040_1000, &rev_code).unwrap();
        let mut arena_rev = vec![0u8; 64 * 1024];
        arena_rev[0x4000..0x4004].copy_from_slice(b"ABCD");
        let mut cpu_rev = GuestCpuState::new(arena_rev.as_mut_ptr(), &bridge);
        cpu_rev.set_gpr(Gpr::Esp, 0x8000);
        cpu_rev.set_gpr(Gpr::Esi, 0x4003);
        cpu_rev.set_gpr(Gpr::Edi, 0x5003);
        cpu_rev.set_gpr(Gpr::Ecx, 4);
        let exec_rev = ExecBuffer::new(&comp_rev.code);
        unsafe {
            (exec_rev.as_fn())(&mut cpu_rev);
        }
        assert_eq!(&arena_rev[0x5000..0x5004], b"ABCD");
        assert_eq!(cpu_rev.gpr(Gpr::Esi), 0x3FFF);
        assert_eq!(cpu_rev.gpr(Gpr::Edi), 0x4FFF);

        // 3. rep stosd (memset 4 dwords to 0xDEAD_BEEF)
        let stos_code = [0xFC, 0xF3, 0xAB, 0xC3];
        let comp_stos = lift_raw_snippet(isa.as_ref(), 0x0040_1000, &stos_code).unwrap();
        let mut arena_stos = vec![0u8; 64 * 1024];
        let mut cpu_stos = GuestCpuState::new(arena_stos.as_mut_ptr(), &bridge);
        cpu_stos.set_gpr(Gpr::Esp, 0x8000);
        cpu_stos.set_gpr(Gpr::Eax, 0xDEAD_BEEF);
        cpu_stos.set_gpr(Gpr::Edi, 0x5000);
        cpu_stos.set_gpr(Gpr::Ecx, 4);
        let exec_stos = ExecBuffer::new(&comp_stos.code);
        unsafe {
            (exec_stos.as_fn())(&mut cpu_stos);
        }
        for slot in arena_stos[0x5000..0x5010].chunks_exact(4) {
            assert_eq!(slot, &0xDEAD_BEEFu32.to_le_bytes());
        }

        // 4. repe cmpsb (find first mismatch at index 2 in "ABXD" vs "ABCD")
        let cmps_code = [0xFC, 0xF3, 0xA6, 0xC3];
        let comp_cmps = lift_raw_snippet(isa.as_ref(), 0x0040_1000, &cmps_code).unwrap();
        let mut arena_cmps = vec![0u8; 64 * 1024];
        arena_cmps[0x4000..0x4004].copy_from_slice(b"ABXD");
        arena_cmps[0x5000..0x5004].copy_from_slice(b"ABCD");
        let mut cpu_cmps = GuestCpuState::new(arena_cmps.as_mut_ptr(), &bridge);
        cpu_cmps.set_gpr(Gpr::Esp, 0x8000);
        cpu_cmps.set_gpr(Gpr::Esi, 0x4000);
        cpu_cmps.set_gpr(Gpr::Edi, 0x5000);
        cpu_cmps.set_gpr(Gpr::Ecx, 4);
        let exec_cmps = ExecBuffer::new(&comp_cmps.code);
        unsafe {
            (exec_cmps.as_fn())(&mut cpu_cmps);
        }
        cpu_cmps.materialize_flags();
        // Compared index 0, 1, 2 (mismatch 'X' vs 'C') -> ECX left = 1, EDI = 0x5003, ZF=0
        assert_eq!(cpu_cmps.gpr(Gpr::Ecx), 1);
        assert_eq!(cpu_cmps.gpr(Gpr::Edi), 0x5003);
        assert_eq!(cpu_cmps.eflags & EFLAGS_ZF, 0);

        // 5. repne scasb (strlen of "hello\0" -> finds '\0' at index 5)
        let scas_code = [0xFC, 0xF2, 0xAE, 0xC3];
        let comp_scas = lift_raw_snippet(isa.as_ref(), 0x0040_1000, &scas_code).unwrap();
        let mut arena_scas = vec![0u8; 64 * 1024];
        arena_scas[0x5000..0x5006].copy_from_slice(b"hello\0");
        let mut cpu_scas = GuestCpuState::new(arena_scas.as_mut_ptr(), &bridge);
        cpu_scas.set_gpr(Gpr::Esp, 0x8000);
        cpu_scas.set_gpr(Gpr::Eax, 0);
        cpu_scas.set_gpr(Gpr::Edi, 0x5000);
        cpu_scas.set_gpr(Gpr::Ecx, 16);
        let exec_scas = ExecBuffer::new(&comp_scas.code);
        unsafe {
            (exec_scas.as_fn())(&mut cpu_scas);
        }
        cpu_scas.materialize_flags();
        assert_eq!(cpu_scas.gpr(Gpr::Ecx), 10);
        assert_eq!(cpu_scas.gpr(Gpr::Edi), 0x5006);
        assert_ne!(cpu_scas.eflags & EFLAGS_ZF, 0);
    }

    #[test]
    fn cross_compile_aarch64_mixed_fpu_simd_and_string() {
        let isa = build_isa(TargetArch::AArch64).unwrap();
        #[rustfmt::skip]
        let code = [
            0xD9, 0xE8,             // fld1
            0xD9, 0xFA,             // fsqrt
            0xD9, 0xFE,             // fsin
            0x0F, 0x57, 0xC0,       // xorps xmm0, xmm0
            0x0F, 0x58, 0xC0,       // addps xmm0, xmm0
            0xFC, 0xF3, 0xA4,       // cld; rep movsb
            0xC3,                   // ret
        ];
        let compiled = lift_raw_snippet(isa.as_ref(), 0x0040_1000, &code).unwrap();
        assert!(!compiled.code.is_empty());
        assert_eq!(compiled.unsupported_count, 0);
    }
}
