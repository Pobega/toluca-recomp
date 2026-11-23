use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs::OpenOptions;
use std::io::{Read, Seek, Write};
use std::path::Path;
use std::time::Instant;

use cranelift_codegen::Context;
use cranelift_frontend::FunctionBuilderContext;
use toluca_core::GuestAddr;
use toluca_core::aot::{AotError, AotWriter, FUNC_FLAG_ENTRY_POINT, TargetArch};

use crate::cfg::{DiscoveryReport, JumpTable, discover_functions};
use crate::lifter::{LifterError, build_isa, lift_function};
use crate::pe::{PeError, PeImage};
use crate::verify::{BinaryVerdict, VerificationReport, verify_pe};

const BYTES_PER_MEBIBYTE: f64 = 1024.0 * 1024.0;
const KIB_PER_MEBIBYTE: f64 = 1024.0;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct CompileOptions {
    pub arch: TargetArch,
    pub strict_verify: bool,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct CompileReport {
    pub arch: TargetArch,
    pub function_count: usize,
    pub compiled_functions: usize,
    pub skipped_functions: usize,
    pub unsupported_instructions: usize,
    pub patched_relocs: usize,
    pub import_count: usize,
    pub code_bytes: u64,
    pub total_file_size: u64,
    pub elapsed_ms: u128,
    pub peak_rss_kib: Option<u64>,
}

impl CompileReport {
    pub fn format_summary(&self) -> String {
        use std::fmt::Write;
        let mut out = String::new();
        let _ = writeln!(out, "Target arch: {:?}", self.arch);
        let _ = writeln!(
            out,
            "Functions compiled: {} / {} (skipped: {})",
            self.compiled_functions, self.function_count, self.skipped_functions
        );
        let _ = writeln!(
            out,
            "Unsupported instructions: {}",
            self.unsupported_instructions
        );
        let _ = writeln!(out, "Patched relocations: {}", self.patched_relocs);
        let _ = writeln!(out, "Imports recorded: {}", self.import_count);
        let _ = writeln!(
            out,
            "Code bytes: {} ({:.2} MiB)",
            self.code_bytes,
            (self.code_bytes as f64) / BYTES_PER_MEBIBYTE
        );
        let _ = writeln!(
            out,
            "Total AOT file size: {} ({:.2} MiB)",
            self.total_file_size,
            (self.total_file_size as f64) / BYTES_PER_MEBIBYTE
        );
        let _ = writeln!(out, "Elapsed time: {} ms", self.elapsed_ms);
        if let Some(rss_kib) = self.peak_rss_kib {
            let _ = writeln!(
                out,
                "Peak RSS: {} KiB ({:.2} MiB)",
                rss_kib,
                (rss_kib as f64) / KIB_PER_MEBIBYTE
            );
        }
        out
    }
}

#[derive(Debug)]
pub enum CompileError {
    Pe(PeError),
    Aot(AotError),
    Lifter(LifterError),
    Io(std::io::Error),
    RejectedBinary(String),
}

impl fmt::Display for CompileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Pe(err) => write!(f, "PE parse error: {err}"),
            Self::Aot(err) => write!(f, "AOT write error: {err}"),
            Self::Lifter(err) => write!(f, "lifter error: {err}"),
            Self::Io(err) => write!(f, "I/O error: {err}"),
            Self::RejectedBinary(msg) => write!(f, "binary rejected: {msg}"),
        }
    }
}

impl std::error::Error for CompileError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Pe(err) => Some(err),
            Self::Aot(err) => Some(err),
            Self::Lifter(err) => Some(err),
            Self::Io(err) => Some(err),
            Self::RejectedBinary(_) => None,
        }
    }
}

impl From<PeError> for CompileError {
    fn from(err: PeError) -> Self {
        Self::Pe(err)
    }
}

impl From<AotError> for CompileError {
    fn from(err: AotError) -> Self {
        Self::Aot(err)
    }
}

impl From<LifterError> for CompileError {
    fn from(err: LifterError) -> Self {
        Self::Lifter(err)
    }
}

impl From<std::io::Error> for CompileError {
    fn from(err: std::io::Error) -> Self {
        Self::Io(err)
    }
}

pub fn read_peak_rss_kib() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("VmHWM:") {
            return rest.split_whitespace().next()?.parse::<u64>().ok();
        }
    }
    None
}

pub fn compile_pe_to_writer<W: Read + Write + Seek>(
    image: &PeImage,
    discovery: &DiscoveryReport,
    writer: W,
    options: CompileOptions,
) -> Result<CompileReport, CompileError> {
    let start = Instant::now();
    let isa = build_isa(options.arch)?;
    let mut ctx = Context::new();
    let mut func_ctx = FunctionBuilderContext::new();

    let mut aot = AotWriter::new(
        writer,
        options.arch,
        image.sha1,
        image.entry_va(),
        image.image_base,
    )?;

    for imp in &image.imports {
        aot.add_import(imp.iat_va, &imp.dll_name, imp.symbol.clone());
    }

    let mut jump_table_map: BTreeMap<GuestAddr, &JumpTable> = BTreeMap::new();
    for jt in &discovery.jump_tables {
        jump_table_map.insert(jt.site_va, jt);
    }

    let known_functions: BTreeSet<GuestAddr> = discovery.functions.keys().copied().collect();
    let mut compiled_functions = 0usize;
    let mut skipped_functions = 0usize;
    let mut unsupported_instructions = 0usize;

    for func in discovery.functions.values() {
        let compiled = match lift_function(
            isa.as_ref(),
            &mut ctx,
            &mut func_ctx,
            image,
            func,
            &jump_table_map,
            &known_functions,
        ) {
            Ok(out) => out,
            Err(LifterError::NoInstructions(_)) => {
                skipped_functions += 1;
                continue;
            }
            Err(err) => return Err(CompileError::Lifter(err)),
        };

        let flags = if func.entry_va == image.entry_va() {
            FUNC_FLAG_ENTRY_POINT
        } else {
            0
        };
        let base_offset = aot.write_function(func.entry_va, flags, &compiled.code)?;
        for reloc in &compiled.relocs {
            aot.add_relocation(toluca_core::aot::RelocEntry {
                code_offset: base_offset + reloc.code_offset,
                target_va: reloc.target_va,
                kind: reloc.kind as u16,
                flags: 0,
                addend: reloc.addend,
            });
        }

        unsupported_instructions += compiled.unsupported_count;
        compiled_functions += 1;
        drop(compiled);
    }

    let patched_relocs = aot.patch_internal_relocations()?;
    let stats = aot.finish()?;

    Ok(CompileReport {
        arch: options.arch,
        function_count: discovery.functions.len(),
        compiled_functions,
        skipped_functions,
        unsupported_instructions,
        patched_relocs,
        import_count: stats.import_count,
        code_bytes: stats.code_bytes,
        total_file_size: stats.total_file_size,
        elapsed_ms: start.elapsed().as_millis(),
        peak_rss_kib: read_peak_rss_kib(),
    })
}

pub fn compile_file(
    input_path: &Path,
    output_path: &Path,
    options: CompileOptions,
) -> Result<(VerificationReport, CompileReport), CompileError> {
    let bytes = std::fs::read(input_path)?;
    let image = PeImage::parse(bytes)?;
    let verification = verify_pe(&image);

    if verification.is_rejected() {
        return Err(CompileError::RejectedBinary(
            verification.format_summary(),
        ));
    }
    if options.strict_verify
        && matches!(verification.verdict, BinaryVerdict::UnverifiedCleanPe { .. })
    {
        return Err(CompileError::RejectedBinary(
            "strict verification enabled: PE hash is not in known supported profiles".to_owned(),
        ));
    }

    let discovery = discover_functions(&image);
    let out_file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(output_path)?;

    let report = compile_pe_to_writer(&image, &discovery, out_file, options)?;
    Ok((verification, report))
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use toluca_core::aot::{AOT_CODE_ALIGNMENT, AotManifest};
    use toluca_core::bridge::{HostBridge, RecompiledFn};
    use toluca_core::cpu::{Gpr, GuestCpuState};

    use super::*;
    use crate::pe::test_helpers::{TestSectionSpec, build_test_pe32};
    use crate::pe::{
        DIR_IMPORT, DataDirectory, IMAGE_SCN_CNT_CODE, IMAGE_SCN_MEM_EXECUTE, IMAGE_SCN_MEM_READ,
    };

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
            }
            Self {
                ptr: ptr.cast::<u8>(),
                len,
            }
        }

        fn fn_at_offset(&self, offset: usize) -> RecompiledFn {
            unsafe { core::mem::transmute::<*const u8, RecompiledFn>(self.ptr.add(offset)) }
        }
    }

    impl Drop for ExecBuffer {
        fn drop(&mut self) {
            unsafe {
                libc::munmap(self.ptr.cast::<libc::c_void>(), self.len);
            }
        }
    }

    unsafe extern "C" fn test_import_callback(cpu: *mut GuestCpuState, import_index: u32) {
        let state = unsafe { &mut *cpu };
        let esp = state.gpr(Gpr::Esp);
        let ret_bytes =
            unsafe { core::slice::from_raw_parts(state.mem_base.add(esp as usize), 4) };
        let ret_va = u32::from_le_bytes([ret_bytes[0], ret_bytes[1], ret_bytes[2], ret_bytes[3]]);
        state.set_gpr(Gpr::Esp, esp + 4);
        state.eip = ret_va;
        state.set_gpr(Gpr::Eax, 100 + import_index);
    }

    fn build_multi_function_pe() -> (PeImage, DiscoveryReport, u32) {
        let image_base = 0x0040_0000u32;
        let text_rva = 0x1000u32;
        let rdata_rva = 0x2000u32;
        let iat_va = image_base + rdata_rva + 0x70;

        // Layout in .text:
        //   0x00: entry: call helper_a (0x40), add eax, 1, mov ebx, eax, call [iat], add eax, ebx, ret
        //   0x20: helper_b: mov eax, 20; ret
        //   0x40: helper_a: call helper_b (0x20, backward call in VA & code offset), add eax, 5, ret
        let mut text = vec![0xCCu8; 0x60];

        // entry (+0x00):
        //   E8 rel32 -> 0x40 (rel = 0x40 - 5 = 0x3B)
        text[0..5].copy_from_slice(&[0xE8, 0x3B, 0x00, 0x00, 0x00]);
        //   83 C0 01 (add eax, 1)
        text[5..8].copy_from_slice(&[0x83, 0xC0, 0x01]);
        //   8B D8 (mov ebx, eax)
        text[8..10].copy_from_slice(&[0x8B, 0xD8]);
        //   FF 15 <iat_va> (call [GetMagic])
        text[10..12].copy_from_slice(&[0xFF, 0x15]);
        text[12..16].copy_from_slice(&iat_va.to_le_bytes());
        //   03 C3 (add eax, ebx)
        text[16..18].copy_from_slice(&[0x03, 0xC3]);
        //   C3 (ret)
        text[18] = 0xC3;

        // helper_b (+0x20): B8 14 00 00 00 C3
        text[0x20..0x26].copy_from_slice(&[0xB8, 0x14, 0x00, 0x00, 0x00, 0xC3]);

        // helper_a (+0x40): E8 <rel to 0x20 = 0x20 - 0x45 = -37 = 0xFFFFFFDB> 83 C0 05 C3
        let rel_b = (0x20isize - 0x45isize) as i32;
        text[0x40] = 0xE8;
        text[0x41..0x45].copy_from_slice(&rel_b.to_le_bytes());
        text[0x45..0x49].copy_from_slice(&[0x83, 0xC0, 0x05, 0xC3]);

        let mut rdata = vec![0u8; 256];
        rdata[0..4].copy_from_slice(&(rdata_rva + 0x60).to_le_bytes());
        rdata[12..16].copy_from_slice(&(rdata_rva + 0x80).to_le_bytes());
        rdata[16..20].copy_from_slice(&(rdata_rva + 0x70).to_le_bytes());
        rdata[0x60..0x64].copy_from_slice(&(rdata_rva + 0xB0).to_le_bytes());
        rdata[0x80..0x8D].copy_from_slice(b"kernel32.dll\0");
        rdata[0xB2..0xBB].copy_from_slice(b"GetMagic\0");

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
        let discovery = discover_functions(&image);
        (image, discovery, image_base + text_rva)
    }

    #[test]
    fn streaming_compile_patches_forward_and_backward_calls_and_executes_natively() {
        let (image, discovery, entry_va) = build_multi_function_pe();
        let arch = TargetArch::current().unwrap();
        let mut cursor = Cursor::new(Vec::new());

        let report = compile_pe_to_writer(
            &image,
            &discovery,
            &mut cursor,
            CompileOptions {
                arch,
                strict_verify: false,
            },
        )
        .unwrap();

        assert_eq!(report.compiled_functions, 3);
        assert_eq!(report.skipped_functions, 0);
        assert_eq!(report.unsupported_instructions, 0);
        assert!(report.patched_relocs >= 2);
        assert_eq!(report.import_count, 1);

        cursor.set_position(0);
        let manifest = AotManifest::read_from(&mut cursor, true).unwrap();
        let entry_disp = manifest.lookup_va(entry_va).unwrap();
        assert_ne!(entry_disp.flags & FUNC_FLAG_ENTRY_POINT, 0);

        let raw_aot = cursor.into_inner();
        let code_start = manifest.code_section.file_offset as usize;
        let code_end = code_start + manifest.code_section.byte_len as usize;
        assert_eq!(code_start as u64, AOT_CODE_ALIGNMENT);
        let code_slice = &raw_aot[code_start..code_end];

        let exec = ExecBuffer::new(code_slice);
        let mut arena = vec![0u8; 64 * 1024];
        let mut bridge = HostBridge::new_stub();
        bridge.call_import = test_import_callback;
        let mut cpu = GuestCpuState::new(arena.as_mut_ptr(), &bridge);
        cpu.set_gpr(Gpr::Esp, 0x8000);
        arena[0x8000..0x8004].copy_from_slice(&0x9999_0001u32.to_le_bytes());

        unsafe {
            (exec.fn_at_offset(entry_disp.code_offset as usize))(&mut cpu);
        }

        // helper_b(20) -> helper_a(+5 = 25) -> entry(+1 = 26) + GetMagic(100) = 126
        assert_eq!(cpu.gpr(Gpr::Eax), 126);
        assert_eq!(cpu.eip, 0x9999_0001);
        assert_eq!(cpu.gpr(Gpr::Esp), 0x8004);
    }

    #[test]
    fn streaming_compile_cross_targets_aarch64_with_valid_crc_and_patched_relocs() {
        let (image, discovery, entry_va) = build_multi_function_pe();
        let mut cursor = Cursor::new(Vec::new());

        let report = compile_pe_to_writer(
            &image,
            &discovery,
            &mut cursor,
            CompileOptions {
                arch: TargetArch::AArch64,
                strict_verify: false,
            },
        )
        .unwrap();

        assert_eq!(report.compiled_functions, 3);
        assert_eq!(report.unsupported_instructions, 0);
        assert!(report.patched_relocs >= 2);
        assert!(report.code_bytes > 0);

        cursor.set_position(0);
        let manifest = AotManifest::read_from(&mut cursor, true).unwrap();
        assert_eq!(manifest.header.arch, TargetArch::AArch64 as u16);
        assert!(manifest.lookup_va(entry_va).is_some());
    }
}
