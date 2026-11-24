use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt::Write;

use iced_x86::{
    Code, Decoder, DecoderOptions, FlowControl, Instruction, Mnemonic, OpKind, Register,
};
use toluca_core::GuestAddr;
use toluca_core::aot::ImportSymbol;

use crate::pe::{PeImage, PeSection};

const MAX_JUMP_TABLE_ENTRIES: usize = 1024;
const FALLBACK_MAX_JUMP_TABLE_ENTRIES: usize = 256;
const LOOKBACK_INSTR_LIMIT: usize = 6;
const MAX_FIXPOINT_PASSES: usize = 16;
const MAX_BLOCK_INSTRUCTIONS: usize = 4096;
const DATA_PTR_DECODE_MIN_INSTRS: usize = 3;
const PROLOGUE_ALIGNMENT: usize = 16;

#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum SeedKind {
    EntryPoint,
    TlsCallback,
    Export,
    DirectCall,
    TailCall,
    DataPointer,
    PrologueScan,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BasicBlock {
    pub start_va: GuestAddr,
    pub end_va: GuestAddr,
    pub instruction_count: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JumpTable {
    pub site_va: GuestAddr,
    pub table_va: GuestAddr,
    pub targets: Vec<GuestAddr>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiscoveredFunction {
    pub entry_va: GuestAddr,
    pub seed_kind: SeedKind,
    pub blocks: BTreeMap<GuestAddr, BasicBlock>,
    pub instruction_count: usize,
    pub direct_callees: BTreeSet<GuestAddr>,
    pub called_imports: BTreeSet<GuestAddr>,
    pub has_indirect_call: bool,
    pub has_indirect_jmp: bool,
}

#[derive(Clone, Debug)]
pub struct DiscoveryReport {
    pub functions: BTreeMap<GuestAddr, DiscoveredFunction>,
    pub jump_tables: Vec<JumpTable>,
    pub code_section_bytes: usize,
    pub covered_code_bytes: usize,
    pub imports_by_dll: BTreeMap<String, Vec<String>>,
}

impl DiscoveryReport {
    pub fn coverage_percent(&self) -> f64 {
        if self.code_section_bytes == 0 {
            return 0.0;
        }
        (self.covered_code_bytes as f64) * 100.0 / (self.code_section_bytes as f64)
    }

    pub fn format_summary(&self, include_imports: bool) -> String {
        let mut out = String::new();
        let _ = writeln!(out, "Discovered functions: {}", self.functions.len());
        let _ = writeln!(
            out,
            "Code coverage: {} / {} bytes ({:.2}%)",
            self.covered_code_bytes,
            self.code_section_bytes,
            self.coverage_percent()
        );
        let _ = writeln!(out, "Jump tables: {}", self.jump_tables.len());

        let total_imports: usize = self.imports_by_dll.values().map(Vec::len).sum();
        let _ = writeln!(
            out,
            "Imports: {} symbols across {} DLLs",
            total_imports,
            self.imports_by_dll.len()
        );

        let mut seed_counts = BTreeMap::new();
        for func in self.functions.values() {
            *seed_counts.entry(format!("{:?}", func.seed_kind)).or_insert(0usize) += 1;
        }
        if !seed_counts.is_empty() {
            let _ = writeln!(out, "Seeds breakdown:");
            for (kind, count) in seed_counts {
                let _ = writeln!(out, "  {kind}: {count}");
            }
        }

        if include_imports && !self.imports_by_dll.is_empty() {
            let _ = writeln!(out, "Imported symbols:");
            for (dll, syms) in &self.imports_by_dll {
                let _ = writeln!(out, "  {dll} ({}):", syms.len());
                for sym in syms {
                    let _ = writeln!(out, "    - {sym}");
                }
            }
        }

        out
    }
}

pub fn discover_functions(image: &PeImage) -> DiscoveryReport {
    let mut coverage = CodeCoverage::new(image);
    let mut functions = BTreeMap::new();
    let mut jump_tables = Vec::new();
    let mut seen_jump_table_sites = BTreeSet::new();
    let mut worklist = VecDeque::new();
    let mut queued_entries = BTreeSet::new();

    enqueue_seed(image, image.entry_va(), SeedKind::EntryPoint, &mut queued_entries, &mut worklist);
    for &cb in &image.tls_callbacks {
        enqueue_seed(image, cb, SeedKind::TlsCallback, &mut queued_entries, &mut worklist);
    }
    for &exp in &image.exports {
        enqueue_seed(image, exp, SeedKind::Export, &mut queued_entries, &mut worklist);
    }
    for va in scan_data_section_pointers(image) {
        enqueue_seed(image, va, SeedKind::DataPointer, &mut queued_entries, &mut worklist);
    }

    for _pass in 0..MAX_FIXPOINT_PASSES {
        while let Some((entry_va, seed_kind)) = worklist.pop_front() {
            if functions.contains_key(&entry_va) {
                continue;
            }
            let Some(func) = explore_function(
                image,
                entry_va,
                seed_kind,
                &mut coverage,
                &mut jump_tables,
                &mut seen_jump_table_sites,
                &mut worklist,
                &mut queued_entries,
            ) else {
                continue;
            };
            functions.insert(entry_va, func);
        }

        let new_prologues = scan_uncovered_prologues(&coverage);
        if new_prologues.is_empty() {
            break;
        }
        let mut added_any = false;
        for va in new_prologues {
            if enqueue_seed(
                image,
                va,
                SeedKind::PrologueScan,
                &mut queued_entries,
                &mut worklist,
            ) {
                added_any = true;
            }
        }
        if !added_any {
            break;
        }
    }

    let imports_by_dll = group_imports_by_dll(image);
    DiscoveryReport {
        functions,
        jump_tables,
        code_section_bytes: coverage.total_bytes(),
        covered_code_bytes: coverage.covered_bytes(),
        imports_by_dll,
    }
}

fn enqueue_seed(
    image: &PeImage,
    va: GuestAddr,
    kind: SeedKind,
    queued_entries: &mut BTreeSet<GuestAddr>,
    worklist: &mut VecDeque<(GuestAddr, SeedKind)>,
) -> bool {
    if !is_executable_va(image, va) || !queued_entries.insert(va) {
        return false;
    }
    worklist.push_back((va, kind));
    true
}

#[derive(Clone)]
struct BlockWorkItem {
    va: GuestAddr,
    recent_instrs: Vec<Instruction>,
}

#[allow(clippy::too_many_arguments)]
fn explore_function(
    image: &PeImage,
    entry_va: GuestAddr,
    seed_kind: SeedKind,
    coverage: &mut CodeCoverage,
    jump_tables: &mut Vec<JumpTable>,
    seen_jump_table_sites: &mut BTreeSet<GuestAddr>,
    fn_worklist: &mut VecDeque<(GuestAddr, SeedKind)>,
    queued_entries: &mut BTreeSet<GuestAddr>,
) -> Option<DiscoveredFunction> {
    if !is_executable_va(image, entry_va) {
        return None;
    }

    let mut blocks = BTreeMap::new();
    let mut visited_blocks = BTreeSet::new();
    let mut block_queue = VecDeque::new();
    block_queue.push_back(BlockWorkItem {
        va: entry_va,
        recent_instrs: Vec::new(),
    });

    let mut total_instrs = 0usize;
    let mut direct_callees = BTreeSet::new();
    let mut called_imports = BTreeSet::new();
    let mut has_indirect_call = false;
    let mut has_indirect_jmp = false;

    while let Some(item) = block_queue.pop_front() {
        if !visited_blocks.insert(item.va) {
            continue;
        }
        let Some((sec, slice)) = executable_slice_from_va(image, item.va) else {
            continue;
        };
        let sec_end_va = sec.va_range(image.image_base).1;

        let mut decoder = Decoder::with_ip(32, slice, item.va as u64, DecoderOptions::NONE);
        let mut instr = Instruction::default();
        let mut block_instr_count = 0usize;
        let mut block_end_va = item.va;
        let mut recent = item.recent_instrs;

        while decoder.can_decode() && block_instr_count < MAX_BLOCK_INSTRUCTIONS {
            decoder.decode_out(&mut instr);
            if instr.is_invalid() {
                break;
            }
            let instr_va = instr.ip() as GuestAddr;
            let instr_len = instr.len();
            let next_va = instr_va.saturating_add(instr_len as u32);
            if next_va > sec_end_va {
                break;
            }

            coverage.mark_range(instr_va, instr_len);
            block_instr_count += 1;
            total_instrs += 1;
            block_end_va = next_va;

            enqueue_immediate_code_pointers(image, &instr, queued_entries, fn_worklist);

            if recent.len() == LOOKBACK_INSTR_LIMIT {
                recent.remove(0);
            }
            recent.push(instr);

            if matches!(instr.mnemonic(), Mnemonic::Int3 | Mnemonic::Ud2) {
                break;
            }

            match instr.flow_control() {
                FlowControl::Next => {}
                FlowControl::Call => {
                    let target_va = instr.near_branch32();
                    if is_executable_va(image, target_va) {
                        direct_callees.insert(target_va);
                        enqueue_seed(
                            image,
                            target_va,
                            SeedKind::DirectCall,
                            queued_entries,
                            fn_worklist,
                        );
                    }
                }
                FlowControl::IndirectCall => {
                    if let Some(iat_va) =
                        absolute_memory_disp(&instr).filter(|&va| image.import_at_va(va).is_some())
                    {
                        called_imports.insert(iat_va);
                    } else {
                        has_indirect_call = true;
                    }
                }
                FlowControl::ConditionalBranch => {
                    let target_va = instr.near_branch32();
                    if is_executable_va(image, target_va) && !visited_blocks.contains(&target_va) {
                        block_queue.push_back(BlockWorkItem {
                            va: target_va,
                            recent_instrs: recent.clone(),
                        });
                    }
                    if is_executable_va(image, next_va) && !visited_blocks.contains(&next_va) {
                        block_queue.push_back(BlockWorkItem {
                            va: next_va,
                            recent_instrs: recent.clone(),
                        });
                    }
                    break;
                }
                FlowControl::UnconditionalBranch => {
                    let target_va = instr.near_branch32();
                    if target_va != entry_va
                        && is_executable_va(image, target_va)
                        && (queued_entries.contains(&target_va)
                            || is_preceded_by_padding_prologue(image, target_va))
                    {
                        direct_callees.insert(target_va);
                        enqueue_seed(
                            image,
                            target_va,
                            SeedKind::TailCall,
                            queued_entries,
                            fn_worklist,
                        );
                    } else if is_executable_va(image, target_va)
                        && !visited_blocks.contains(&target_va)
                    {
                        block_queue.push_back(BlockWorkItem {
                            va: target_va,
                            recent_instrs: Vec::new(),
                        });
                    }
                    break;
                }
                FlowControl::IndirectBranch => {
                    if let Some(iat_va) =
                        absolute_memory_disp(&instr).filter(|&va| image.import_at_va(va).is_some())
                    {
                        called_imports.insert(iat_va);
                        break;
                    }

                    has_indirect_jmp = true;
                    if let Some(jt) = try_resolve_jump_table(image, &instr, &recent) {
                        for &target_va in &jt.targets {
                            if !visited_blocks.contains(&target_va) {
                                block_queue.push_back(BlockWorkItem {
                                    va: target_va,
                                    recent_instrs: Vec::new(),
                                });
                            }
                        }
                        if seen_jump_table_sites.insert(jt.site_va) {
                            jump_tables.push(jt);
                        }
                    }
                    break;
                }
                FlowControl::Return | FlowControl::Interrupt | FlowControl::Exception => break,
                FlowControl::XbeginXabortXend => {}
            }
        }

        if block_instr_count > 0 {
            blocks.insert(
                item.va,
                BasicBlock {
                    start_va: item.va,
                    end_va: block_end_va,
                    instruction_count: block_instr_count,
                },
            );
        }
    }

    if blocks.is_empty() {
        return None;
    }

    Some(DiscoveredFunction {
        entry_va,
        seed_kind,
        blocks,
        instruction_count: total_instrs,
        direct_callees,
        called_imports,
        has_indirect_call,
        has_indirect_jmp,
    })
}

fn try_resolve_jump_table(
    image: &PeImage,
    jmp_instr: &Instruction,
    recent_instrs: &[Instruction],
) -> Option<JumpTable> {
    if jmp_instr.code() != Code::Jmp_rm32
        || jmp_instr.op0_kind() != OpKind::Memory
        || jmp_instr.memory_base() != Register::None
        || jmp_instr.memory_index() == Register::None
        || jmp_instr.memory_index_scale() != 4
    {
        return None;
    }

    let table_va = jmp_instr.memory_displacement32();
    if table_va == 0 {
        return None;
    }

    let table_sec = image.section_for_va(table_va)?;
    if !table_sec.is_readable() && !table_sec.is_executable() {
        return None;
    }

    let bound = infer_jump_table_bound(recent_instrs, jmp_instr.memory_index());
    let limit = bound.unwrap_or(FALLBACK_MAX_JUMP_TABLE_ENTRIES);

    let mut targets = Vec::new();
    for idx in 0..limit {
        let slot_va = table_va.wrapping_add((idx * 4) as u32);
        let Some(target_va) = image.read_u32_at_va(slot_va) else {
            break;
        };
        if !is_executable_va(image, target_va) {
            break;
        }
        targets.push(target_va);
    }

    if targets.is_empty() {
        return None;
    }

    Some(JumpTable {
        site_va: jmp_instr.ip() as GuestAddr,
        table_va,
        targets,
    })
}

fn infer_jump_table_bound(recent: &[Instruction], index_reg: Register) -> Option<usize> {
    let mut saw_strict_upper = false;
    let mut saw_exclusive_upper = false;

    for instr in recent.iter().rev().take(LOOKBACK_INSTR_LIMIT) {
        if instr.mnemonic() == Mnemonic::Ja || instr.mnemonic() == Mnemonic::Jg {
            saw_strict_upper = true;
        } else if instr.mnemonic() == Mnemonic::Jae || instr.mnemonic() == Mnemonic::Jge {
            saw_exclusive_upper = true;
        }

        if instr.mnemonic() == Mnemonic::Cmp && instr.op0_kind() == OpKind::Register {
            let cmp_reg = instr.op0_register();
            if cmp_reg != index_reg && !same_gpr_family(cmp_reg, index_reg) {
                continue;
            }
            let imm = read_cmp_immediate(instr)? as usize;
            let count = if saw_exclusive_upper && !saw_strict_upper {
                imm
            } else {
                imm.saturating_add(1)
            };
            if count > 0 {
                return Some(count.min(MAX_JUMP_TABLE_ENTRIES));
            }
        }
    }
    None
}

fn read_cmp_immediate(instr: &Instruction) -> Option<u32> {
    match instr.op1_kind() {
        OpKind::Immediate8 => Some(instr.immediate8() as u32),
        OpKind::Immediate8to16 => Some(instr.immediate8to16() as u16 as u32),
        OpKind::Immediate8to32 => Some(instr.immediate8to32() as u32),
        OpKind::Immediate16 => Some(instr.immediate16() as u32),
        OpKind::Immediate32 => Some(instr.immediate32()),
        _ => None,
    }
}

fn same_gpr_family(a: Register, b: Register) -> bool {
    a.full_register32() == b.full_register32()
}

fn absolute_memory_disp(instr: &Instruction) -> Option<GuestAddr> {
    if instr.op0_kind() == OpKind::Memory
        && instr.memory_base() == Register::None
        && instr.memory_index() == Register::None
    {
        Some(instr.memory_displacement32())
    } else {
        None
    }
}

fn scan_data_section_pointers(image: &PeImage) -> Vec<GuestAddr> {
    let mut seeds = Vec::new();
    for sec in image.data_sections() {
        let raw = sec.raw_bytes(&image.raw);
        let base_va = image.image_base.wrapping_add(sec.virtual_address);
        let mut offset = 0usize;
        while offset + 4 <= raw.len() {
            let candidate = u32::from_le_bytes([
                raw[offset],
                raw[offset + 1],
                raw[offset + 2],
                raw[offset + 3],
            ]);
            if is_valid_data_pointer_target(image, candidate) {
                seeds.push(candidate);
            }
            offset += 4;
        }
        let _ = base_va;
    }
    seeds
}

fn enqueue_immediate_code_pointers(
    image: &PeImage,
    instr: &Instruction,
    queued_entries: &mut BTreeSet<GuestAddr>,
    fn_worklist: &mut VecDeque<(GuestAddr, SeedKind)>,
) {
    for op_idx in 0..instr.op_count() {
        if instr.op_kind(op_idx) == OpKind::Immediate32 {
            let imm = instr.immediate32();
            if is_valid_code_immediate_target(image, imm) {
                enqueue_seed(
                    image,
                    imm,
                    SeedKind::DataPointer,
                    queued_entries,
                    fn_worklist,
                );
            }
        }
    }
}

fn is_valid_code_immediate_target(image: &PeImage, target_va: GuestAddr) -> bool {
    let Some((_sec, slice)) = executable_slice_from_va(image, target_va) else {
        return false;
    };
    if slice.first() == Some(&0xCC) {
        return false;
    }
    is_jmp_thunk_to_function(image, target_va, slice)
        || decodes_cleanly(slice, target_va, DATA_PTR_DECODE_MIN_INSTRS)
}

fn is_valid_data_pointer_target(image: &PeImage, target_va: GuestAddr) -> bool {
    let Some((_sec, slice)) = executable_slice_from_va(image, target_va) else {
        return false;
    };
    if is_jmp_thunk_to_function(image, target_va, slice) {
        return true;
    }
    if !decodes_cleanly(slice, target_va, DATA_PTR_DECODE_MIN_INSTRS) {
        return false;
    }
    looks_like_msvc_prologue(slice)
        || is_preceded_by_padding_or_terminator(image, target_va)
        || (target_va as usize).is_multiple_of(PROLOGUE_ALIGNMENT)
}

fn is_jmp_thunk_to_function(image: &PeImage, va: GuestAddr, slice: &[u8]) -> bool {
    if slice.len() < 5 || slice[0] != 0xE9 {
        return false;
    }
    let Some(sec) = image.section_for_va(va) else {
        return false;
    };
    let start_va = image.image_base.wrapping_add(sec.virtual_address);
    let offset = (va - start_va) as usize;
    let raw = sec.raw_bytes(&image.raw);
    let preceded_by_thunk_or_pad = offset == 0
        || is_preceded_by_padding_or_terminator(image, va)
        || (offset >= 5 && offset <= raw.len() && raw[offset - 5] == 0xE9);
    if !preceded_by_thunk_or_pad {
        return false;
    }
    let rel = i32::from_le_bytes([slice[1], slice[2], slice[3], slice[4]]);
    let target_va = va.wrapping_add(5).wrapping_add(rel as u32);
    let Some((_tsec, target_slice)) = executable_slice_from_va(image, target_va) else {
        return false;
    };
    decodes_cleanly(target_slice, target_va, DATA_PTR_DECODE_MIN_INSTRS)
        && (looks_like_msvc_prologue(target_slice)
            || is_preceded_by_padding_or_terminator(image, target_va))
}

fn decodes_cleanly(slice: &[u8], start_va: GuestAddr, min_instrs: usize) -> bool {
    if slice.len() >= 2 && slice[0] == 0x00 && slice[1] == 0x00 {
        return false;
    }
    let mut decoder = Decoder::with_ip(32, slice, start_va as u64, DecoderOptions::NONE);
    let mut instr = Instruction::default();
    let mut count = 0usize;

    while decoder.can_decode() && count < min_instrs {
        decoder.decode_out(&mut instr);
        if instr.is_invalid() {
            return false;
        }
        count += 1;
        if instr.flow_control() == FlowControl::Return && count >= 1 {
            return true;
        }
    }
    count >= min_instrs
}

fn scan_uncovered_prologues(coverage: &CodeCoverage) -> Vec<GuestAddr> {
    let mut found = Vec::new();
    for sec_cov in &coverage.sections {
        let bytes = &sec_cov.bytes;
        for offset in 0..bytes.len() {
            if sec_cov.covered[offset] {
                continue;
            }
            let tail = &bytes[offset..];
            let prev = offset.checked_sub(1).map(|i| bytes[i]);

            let at_alignment = offset % PROLOGUE_ALIGNMENT == 0;
            let after_padding = matches!(prev, Some(0xCC | 0x90));
            let after_terminator = matches!(prev, Some(0xCC | 0x90 | 0xC3));

            let is_strong_prologue =
                looks_like_frame_prologue(tail) && (at_alignment || after_terminator || offset == 0);
            let is_sub_esp_prologue =
                looks_like_sub_esp_prologue(tail) && (after_padding || offset == 0);

            if (is_strong_prologue || is_sub_esp_prologue)
                && decodes_cleanly(tail, sec_cov.start_va + offset as u32, 2)
            {
                found.push(sec_cov.start_va + offset as u32);
            }
        }
    }
    found
}

fn is_preceded_by_padding_prologue(image: &PeImage, va: GuestAddr) -> bool {
    let Some((_sec, slice)) = executable_slice_from_va(image, va) else {
        return false;
    };
    looks_like_msvc_prologue(slice) && is_preceded_by_padding_or_terminator(image, va)
}

fn is_preceded_by_padding_or_terminator(image: &PeImage, va: GuestAddr) -> bool {
    let Some(sec) = image.section_for_va(va) else {
        return false;
    };
    if !sec.is_executable() {
        return false;
    }
    let start_va = image.image_base.wrapping_add(sec.virtual_address);
    let offset = (va - start_va) as usize;
    if offset == 0 {
        return true;
    }
    let raw = sec.raw_bytes(&image.raw);
    if offset > raw.len() {
        return false;
    }
    matches!(raw[offset - 1], 0xCC | 0x90 | 0xC3)
        || (offset >= 3 && raw[offset - 3] == 0xC2 && raw[offset - 1] == 0x00 && raw[offset - 2].is_multiple_of(4))
        || (offset >= 5 && raw[offset - 5] == 0xE9)
        || (offset >= 6 && raw[offset - 6] == 0xFF && raw[offset - 5] == 0x25)
}

fn looks_like_msvc_prologue(bytes: &[u8]) -> bool {
    looks_like_frame_prologue(bytes) || looks_like_sub_esp_prologue(bytes)
}

fn looks_like_frame_prologue(bytes: &[u8]) -> bool {
    // push ebp; mov ebp, esp (8B EC or 89 E5)
    if bytes.starts_with(&[0x55, 0x8B, 0xEC]) || bytes.starts_with(&[0x55, 0x89, 0xE5]) {
        return true;
    }
    // mov edi, edi; push ebp; mov ebp, esp (hotpatchable MSVC prologue)
    bytes.starts_with(&[0x8B, 0xFF, 0x55, 0x8B, 0xEC])
}

fn looks_like_sub_esp_prologue(bytes: &[u8]) -> bool {
    // sub esp, imm8 (83 EC imm8) or sub esp, imm32 (81 EC imm32)
    (bytes.len() >= 3 && bytes[0] == 0x83 && bytes[1] == 0xEC)
        || (bytes.len() >= 6 && bytes[0] == 0x81 && bytes[1] == 0xEC)
}

fn is_executable_va(image: &PeImage, va: GuestAddr) -> bool {
    executable_slice_from_va(image, va).is_some()
}

fn executable_slice_from_va(image: &PeImage, va: GuestAddr) -> Option<(&PeSection, &[u8])> {
    let sec = image.section_for_va(va)?;
    if !sec.is_executable() {
        return None;
    }
    let start_va = image.image_base.wrapping_add(sec.virtual_address);
    let offset = va.checked_sub(start_va)? as usize;
    let raw = sec.raw_bytes(&image.raw);
    let effective_len = effective_code_len(sec, raw);
    if offset >= effective_len {
        return None;
    }
    Some((sec, &raw[offset..effective_len]))
}

fn effective_code_len(sec: &PeSection, raw: &[u8]) -> usize {
    if sec.virtual_size > 0 {
        (sec.virtual_size as usize).min(raw.len())
    } else {
        raw.len()
    }
}

fn group_imports_by_dll(image: &PeImage) -> BTreeMap<String, Vec<String>> {
    let mut grouped: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for imp in &image.imports {
        let label = match &imp.symbol {
            ImportSymbol::Name(name) => name.clone(),
            ImportSymbol::Ordinal(ord) => format!("#{ord}"),
        };
        grouped.entry(imp.dll_name.clone()).or_default().push(label);
    }
    grouped
}

struct SectionCoverage<'a> {
    start_va: GuestAddr,
    end_va: GuestAddr,
    bytes: &'a [u8],
    covered: Vec<bool>,
}

struct CodeCoverage<'a> {
    sections: Vec<SectionCoverage<'a>>,
}

impl<'a> CodeCoverage<'a> {
    fn new(image: &'a PeImage) -> Self {
        let mut sections = Vec::new();
        for sec in image.executable_sections() {
            let raw = sec.raw_bytes(&image.raw);
            let len = effective_code_len(sec, raw);
            let start_va = image.image_base.wrapping_add(sec.virtual_address);
            let end_va = start_va.saturating_add(len as u32);
            sections.push(SectionCoverage {
                start_va,
                end_va,
                bytes: &raw[..len],
                covered: vec![false; len],
            });
        }
        Self { sections }
    }

    fn mark_range(&mut self, va: GuestAddr, len: usize) {
        for sec in &mut self.sections {
            if va >= sec.start_va && va < sec.end_va {
                let start = (va - sec.start_va) as usize;
                let end = start.saturating_add(len).min(sec.covered.len());
                sec.covered[start..end].fill(true);
                return;
            }
        }
    }

    fn total_bytes(&self) -> usize {
        self.sections.iter().map(|s| s.covered.len()).sum()
    }

    fn covered_bytes(&self) -> usize {
        self.sections
            .iter()
            .flat_map(|s| s.covered.iter())
            .filter(|&&flag| flag)
            .count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pe::test_helpers::{TestSectionSpec, build_test_pe32};
    use crate::pe::{
        DIR_IMPORT, DataDirectory, IMAGE_SCN_CNT_CODE, IMAGE_SCN_MEM_EXECUTE, IMAGE_SCN_MEM_READ,
    };

    #[test]
    fn discovers_direct_calls_iat_jump_table_data_pointer_and_gap_prologue() {
        let image_base = 0x0040_0000u32;
        let text_rva = 0x1000u32;
        let rdata_rva = 0x2000u32;
        let text_va = image_base + text_rva;
        let rdata_va = image_base + rdata_rva;

        // Build .text layout manually at known offsets:
        // 0x00: entry_fn:
        //   call direct_callee (at +0x40)
        //   call dword ptr [iat_exit_process] (at rdata_va + 0x70)
        //   ret
        // 0x40: switch_fn (called by direct_callee):
        //   cmp eax, 2        (83 F8 02)
        //   ja default        (77 07) -> +0x4C
        //   jmp [eax*4 + jt]  (FF 24 85 <table_va>)
        //   ret (at +0x4C, default + shared ret)
        //   case0 (at +0x50): mov eax, 1; ret
        //   case1 (at +0x56): mov eax, 2; ret
        //   case2 (at +0x5C): mov eax, 3; ret
        // 0x70: direct_callee:
        //   push ebp; mov ebp, esp; call switch_fn (+0x40); pop ebp; ret
        // 0x80: 0xCC padding then gap_fn (unreferenced):
        //   push ebp; mov ebp, esp; xor eax, eax; pop ebp; ret
        // 0x90: 0xCC padding then data_cb (referenced only from .rdata):
        //   push ebp; mov ebp, esp; mov eax, 7; pop ebp; ret
        let mut text = vec![0xCCu8; 0xB0];

        let direct_callee_off = 0x70usize;
        let switch_fn_off = 0x40usize;
        let case0_off = 0x50usize;
        let case1_off = 0x56usize;
        let case2_off = 0x5Cusize;
        let gap_fn_off = 0x80usize;
        let data_cb_off = 0x90usize;

        let iat_va = rdata_va + 0x70;
        let jt_va = rdata_va + 0xD0;

        // entry_fn at 0x00:
        // E8 rel32 (call direct_callee): rel = 0x70 - 5 = 0x6B
        text[0..5].copy_from_slice(&[0xE8, 0x6B, 0x00, 0x00, 0x00]);
        // FF 15 <iat_va> (call [ExitProcess])
        text[5..7].copy_from_slice(&[0xFF, 0x15]);
        text[7..11].copy_from_slice(&iat_va.to_le_bytes());
        text[11] = 0xC3; // ret

        // switch_fn at 0x40:
        // 83 F8 02 (cmp eax, 2)
        text[0x40..0x43].copy_from_slice(&[0x83, 0xF8, 0x02]);
        // 77 07 (ja +7 -> 0x4C)
        text[0x43..0x45].copy_from_slice(&[0x77, 0x07]);
        // FF 24 85 <jt_va> (jmp dword ptr [eax*4 + jt_va])
        text[0x45..0x48].copy_from_slice(&[0xFF, 0x24, 0x85]);
        text[0x48..0x4C].copy_from_slice(&jt_va.to_le_bytes());
        text[0x4C] = 0xC3; // default: ret

        // case0 at 0x50: B8 01 00 00 00 C3
        text[case0_off..case0_off + 6].copy_from_slice(&[0xB8, 0x01, 0x00, 0x00, 0x00, 0xC3]);
        // case1 at 0x56: B8 02 00 00 00 C3
        text[case1_off..case1_off + 6].copy_from_slice(&[0xB8, 0x02, 0x00, 0x00, 0x00, 0xC3]);
        // case2 at 0x5C: B8 03 00 00 00 C3
        text[case2_off..case2_off + 6].copy_from_slice(&[0xB8, 0x03, 0x00, 0x00, 0x00, 0xC3]);

        // direct_callee at 0x70: 55 8B EC E8 <rel to switch_fn> 5D C3
        text[direct_callee_off..direct_callee_off + 3].copy_from_slice(&[0x55, 0x8B, 0xEC]);
        let rel_to_switch = (switch_fn_off as isize - (direct_callee_off + 8) as isize) as i32;
        text[direct_callee_off + 3] = 0xE8;
        text[direct_callee_off + 4..direct_callee_off + 8]
            .copy_from_slice(&rel_to_switch.to_le_bytes());
        text[direct_callee_off + 8..direct_callee_off + 10].copy_from_slice(&[0x5D, 0xC3]);

        // gap_fn at 0x80: 55 8B EC 33 C0 5D C3
        text[gap_fn_off..gap_fn_off + 7]
            .copy_from_slice(&[0x55, 0x8B, 0xEC, 0x33, 0xC0, 0x5D, 0xC3]);

        // data_cb at 0x90: 55 8B EC B8 07 00 00 00 5D C3
        text[data_cb_off..data_cb_off + 10]
            .copy_from_slice(&[0x55, 0x8B, 0xEC, 0xB8, 0x07, 0x00, 0x00, 0x00, 0x5D, 0xC3]);

        // Build .rdata with IAT + jump table + callback pointer
        let mut rdata = vec![0u8; 256];
        rdata[0..4].copy_from_slice(&(rdata_rva + 0x60).to_le_bytes());
        rdata[12..16].copy_from_slice(&(rdata_rva + 0x80).to_le_bytes());
        rdata[16..20].copy_from_slice(&(rdata_rva + 0x70).to_le_bytes());
        rdata[0x60..0x64].copy_from_slice(&(rdata_rva + 0xB0).to_le_bytes());
        rdata[0x80..0x8D].copy_from_slice(b"kernel32.dll\0");
        rdata[0xB2..0xBE].copy_from_slice(b"ExitProcess\0");

        // Jump table at rdata + 0xD0
        rdata[0xD0..0xD4].copy_from_slice(&(text_va + case0_off as u32).to_le_bytes());
        rdata[0xD4..0xD8].copy_from_slice(&(text_va + case1_off as u32).to_le_bytes());
        rdata[0xD8..0xDC].copy_from_slice(&(text_va + case2_off as u32).to_le_bytes());

        // Data-section function pointer at rdata + 0xF0
        rdata[0xF0..0xF4].copy_from_slice(&(text_va + data_cb_off as u32).to_le_bytes());

        let bytes = build_test_pe32(
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

        let pe = PeImage::parse(bytes).unwrap();
        let report = discover_functions(&pe);

        assert_eq!(report.functions[&text_va].seed_kind, SeedKind::EntryPoint);
        assert!(report.functions[&text_va].called_imports.contains(&iat_va));
        assert!(
            report.functions[&text_va]
                .direct_callees
                .contains(&(text_va + direct_callee_off as u32))
        );

        assert_eq!(
            report.functions[&(text_va + direct_callee_off as u32)].seed_kind,
            SeedKind::DirectCall
        );
        let switch_fn = &report.functions[&(text_va + switch_fn_off as u32)];
        assert_eq!(switch_fn.seed_kind, SeedKind::DirectCall);
        assert!(switch_fn.has_indirect_jmp);
        assert!(switch_fn.blocks.contains_key(&(text_va + case0_off as u32)));
        assert!(switch_fn.blocks.contains_key(&(text_va + case1_off as u32)));
        assert!(switch_fn.blocks.contains_key(&(text_va + case2_off as u32)));

        assert_eq!(report.jump_tables.len(), 1);
        assert_eq!(
            report.jump_tables[0].targets,
            vec![
                text_va + case0_off as u32,
                text_va + case1_off as u32,
                text_va + case2_off as u32,
            ]
        );

        assert_eq!(
            report.functions[&(text_va + data_cb_off as u32)].seed_kind,
            SeedKind::DataPointer
        );
        assert_eq!(
            report.functions[&(text_va + gap_fn_off as u32)].seed_kind,
            SeedKind::PrologueScan
        );
        assert!(report.coverage_percent() > 0.0);
    }

    #[test]
    fn discovers_aligned_non_frame_function_from_data_pointer() {
        let image_base = 0x0040_0000u32;
        let text_rva = 0x1000u32;
        let data_rva = 0x2000u32;
        let text_va = image_base + text_rva;

        let mut text = vec![0x90u8; 0x60];
        // Entry at 0x00: mov eax, [esp+4]; ret
        text[0..5].copy_from_slice(&[0x8B, 0x44, 0x24, 0x04, 0xC3]);
        // Simulate jump table bytes right before 0x20 ending in 0x00 (not 0xCC/0x90/0xC3)
        text[0x1C..0x20].copy_from_slice(&[0x00, 0x10, 0x40, 0x00]);
        // Function at 0x20: mov eax, 1; xor ecx, ecx; ret (16-byte aligned, no push ebp)
        text[0x20..0x28].copy_from_slice(&[0xB8, 0x01, 0x00, 0x00, 0x00, 0x33, 0xC9, 0xC3]);

        let mut data = vec![0u8; 64];
        data[0..4].copy_from_slice(&(text_va + 0x20).to_le_bytes());

        let bytes = build_test_pe32(
            image_base,
            text_rva,
            0x0102,
            &[],
            &[
                TestSectionSpec {
                    name: ".text",
                    virtual_address: text_rva,
                    characteristics: IMAGE_SCN_CNT_CODE | IMAGE_SCN_MEM_EXECUTE | IMAGE_SCN_MEM_READ,
                    data: &text,
                },
                TestSectionSpec {
                    name: ".data",
                    virtual_address: data_rva,
                    characteristics: IMAGE_SCN_MEM_READ | crate::pe::IMAGE_SCN_MEM_WRITE,
                    data: &data,
                },
            ],
        );

        let pe = PeImage::parse(bytes).unwrap();
        let report = discover_functions(&pe);
        assert_eq!(
            report.functions.get(&(text_va + 0x20)).map(|f| f.seed_kind),
            Some(SeedKind::DataPointer)
        );
    }
}
