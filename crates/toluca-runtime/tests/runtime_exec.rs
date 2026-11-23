use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use toluca_core::aot::{ImportSymbol, TargetArch};
use toluca_core::cpu::{ExitReason, GuestCpuState};
use toluca_core::memory::DEFAULT_IMAGE_BASE;
use toluca_core::pe::test_helpers::{TestSectionSpec, build_test_pe32};
use toluca_core::pe::{
    DIR_IMPORT, DataDirectory, IMAGE_SCN_CNT_CODE, IMAGE_SCN_MEM_EXECUTE, IMAGE_SCN_MEM_READ,
    IMAGE_SCN_MEM_WRITE, PeImage,
};
use toluca_recomp::cfg::discover_functions;
use toluca_recomp::compiler::{CompileOptions, compile_pe_to_writer};
use toluca_runtime::callconv::{peek_arg_u32, return_stdcall};
use toluca_runtime::{ImportRegistry, Runtime, RuntimeContext, RuntimeError, TrapInfo};

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

struct TempAotFile {
    path: PathBuf,
}

impl TempAotFile {
    fn new(prefix: &str) -> Self {
        let id = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let pid = std::process::id();
        let path = std::env::temp_dir().join(format!("toluca_runtime_{prefix}_{pid}_{id}.bin"));
        Self { path }
    }
}

impl Drop for TempAotFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

fn shim_add_two_stdcall(
    ctx: &mut RuntimeContext<'_>,
    cpu: &mut GuestCpuState,
) -> Result<(), RuntimeError> {
    let a = peek_arg_u32(cpu, ctx.arena, 0)?;
    let b = peek_arg_u32(cpu, ctx.arena, 1)?;
    return_stdcall(cpu, ctx.arena, 2, a.wrapping_add(b))?;
    Ok(())
}

struct TestBinaryLayout {
    pe_bytes: Vec<u8>,
    indirect_iat_va: u32,
    indirect_internal_va: u32,
    unresolved_import_va: u32,
    unresolved_indirect_va: u32,
    cpuid_va: u32,
    rdtsc_va: u32,
    scratch_va: u32,
    iat_missing_index: u32,
}

fn build_runtime_test_pe() -> TestBinaryLayout {
    let image_base = DEFAULT_IMAGE_BASE;
    let text_rva = 0x1000u32;
    let rdata_rva = 0x2000u32;
    let data_rva = 0x3000u32;

    let iat_add_va = image_base + rdata_rva + 0x70;
    let iat_missing_va = image_base + rdata_rva + 0x74;
    let helper_bwd_va = image_base + text_rva + 0x20;

    let indirect_iat_va = image_base + text_rva + 0x60;
    let indirect_internal_va = image_base + text_rva + 0x80;
    let unresolved_import_va = image_base + text_rva + 0xA0;
    let unresolved_indirect_va = image_base + text_rva + 0xC0;
    let cpuid_va = image_base + text_rva + 0xE0;
    let rdtsc_va = image_base + text_rva + 0x100;

    let mut text = vec![0xCCu8; 0x140];

    // +0x00: entry -> call helper_fwd (+0x40), mov ebx, eax, push 17, push 25, call [iat_add], add eax, ebx, ret
    text[0x00..0x05].copy_from_slice(&[0xE8, 0x3B, 0x00, 0x00, 0x00]);
    text[0x05..0x07].copy_from_slice(&[0x8B, 0xD8]);
    text[0x07..0x09].copy_from_slice(&[0x6A, 17]);
    text[0x09..0x0B].copy_from_slice(&[0x6A, 25]);
    text[0x0B..0x0D].copy_from_slice(&[0xFF, 0x15]);
    text[0x0D..0x11].copy_from_slice(&iat_add_va.to_le_bytes());
    text[0x11..0x13].copy_from_slice(&[0x03, 0xC3]);
    text[0x13] = 0xC3;

    // +0x20: helper_bwd -> mov eax, 20; ret
    text[0x20..0x26].copy_from_slice(&[0xB8, 20, 0, 0, 0, 0xC3]);

    // +0x40: helper_fwd -> call helper_bwd (+0x20); add eax, 5; ret
    let rel_bwd = (0x20isize - 0x45isize) as i32;
    text[0x40] = 0xE8;
    text[0x41..0x45].copy_from_slice(&rel_bwd.to_le_bytes());
    text[0x45..0x49].copy_from_slice(&[0x83, 0xC0, 0x05, 0xC3]);

    // +0x60: indirect_iat_caller -> mov eax, [iat_add]; push 10; push 32; call eax; ret
    text[0x60] = 0xA1;
    text[0x61..0x65].copy_from_slice(&iat_add_va.to_le_bytes());
    text[0x65..0x67].copy_from_slice(&[0x6A, 10]);
    text[0x67..0x69].copy_from_slice(&[0x6A, 32]);
    text[0x69..0x6B].copy_from_slice(&[0xFF, 0xD0]);
    text[0x6B] = 0xC3;

    // +0x80: indirect_internal_caller -> mov eax, helper_bwd_va; call eax; add eax, 7; ret
    text[0x80] = 0xB8;
    text[0x81..0x85].copy_from_slice(&helper_bwd_va.to_le_bytes());
    text[0x85..0x87].copy_from_slice(&[0xFF, 0xD0]);
    text[0x87..0x8A].copy_from_slice(&[0x83, 0xC0, 0x07]);
    text[0x8A] = 0xC3;

    // +0xA0: unresolved_import_caller -> call [iat_missing]; ret
    text[0xA0..0xA2].copy_from_slice(&[0xFF, 0x15]);
    text[0xA2..0xA6].copy_from_slice(&iat_missing_va.to_le_bytes());
    text[0xA6] = 0xC3;

    // +0xC0: unresolved_indirect_caller -> mov eax, 0x0050_1234; call eax; ret
    text[0xC0] = 0xB8;
    text[0xC1..0xC5].copy_from_slice(&0x0050_1234u32.to_le_bytes());
    text[0xC5..0xC7].copy_from_slice(&[0xFF, 0xD0]);
    text[0xC7] = 0xC3;

    // +0xE0: cpuid(out_ptr, leaf) -> writes eax, ebx, ecx, edx to [out_ptr]
    text[0xE0..0xE4].copy_from_slice(&[0x8B, 0x7C, 0x24, 0x04]);
    text[0xE4..0xE8].copy_from_slice(&[0x8B, 0x44, 0x24, 0x08]);
    text[0xE8..0xEA].copy_from_slice(&[0x0F, 0xA2]);
    text[0xEA..0xEC].copy_from_slice(&[0x89, 0x07]);
    text[0xEC..0xEF].copy_from_slice(&[0x89, 0x5F, 0x04]);
    text[0xEF..0xF2].copy_from_slice(&[0x89, 0x4F, 0x08]);
    text[0xF2..0xF5].copy_from_slice(&[0x89, 0x57, 0x0C]);
    text[0xF5] = 0xC3;

    // +0x100: rdtsc(out_ptr) -> writes eax, edx to [out_ptr]
    text[0x100..0x104].copy_from_slice(&[0x8B, 0x7C, 0x24, 0x04]);
    text[0x104..0x106].copy_from_slice(&[0x0F, 0x31]);
    text[0x106..0x108].copy_from_slice(&[0x89, 0x07]);
    text[0x108..0x10B].copy_from_slice(&[0x89, 0x57, 0x04]);
    text[0x10B] = 0xC3;

    let mut rdata = vec![0u8; 0x100];
    rdata[0..4].copy_from_slice(&(rdata_rva + 0x60).to_le_bytes());
    rdata[12..16].copy_from_slice(&(rdata_rva + 0x80).to_le_bytes());
    rdata[16..20].copy_from_slice(&(rdata_rva + 0x70).to_le_bytes());

    rdata[0x60..0x64].copy_from_slice(&(rdata_rva + 0xA0).to_le_bytes());
    rdata[0x64..0x68].copy_from_slice(&(rdata_rva + 0xB2).to_le_bytes());

    rdata[0x80..0x8D].copy_from_slice(b"KERNEL32.dll\0");
    rdata[0xA2..0xB0].copy_from_slice(b"AddTwoStdcall\0");
    rdata[0xB4..0xC2].copy_from_slice(b"MissingImport\0");

    // Function pointer table in .rdata so CFG discovers all test entry functions
    let test_fns = [
        indirect_iat_va,
        indirect_internal_va,
        unresolved_import_va,
        unresolved_indirect_va,
        cpuid_va,
        rdtsc_va,
    ];
    for (idx, &fn_va) in test_fns.iter().enumerate() {
        let off = 0xD0 + idx * 4;
        rdata[off..off + 4].copy_from_slice(&fn_va.to_le_bytes());
    }

    let data = vec![0u8; 64];
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
            TestSectionSpec {
                name: ".data",
                virtual_address: data_rva,
                characteristics: IMAGE_SCN_MEM_READ | IMAGE_SCN_MEM_WRITE,
                data: &data,
            },
        ],
    );

    TestBinaryLayout {
        pe_bytes,
        indirect_iat_va,
        indirect_internal_va,
        unresolved_import_va,
        unresolved_indirect_va,
        cpuid_va,
        rdtsc_va,
        scratch_va: image_base + data_rva,
        iat_missing_index: 1,
    }
}

fn compile_and_load_runtime(layout: &TestBinaryLayout, temp: &TempAotFile) -> Runtime {
    let image = PeImage::parse(layout.pe_bytes.clone()).unwrap();
    let discovery = discover_functions(&image);
    let out_file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(&temp.path)
        .unwrap();

    compile_pe_to_writer(
        &image,
        &discovery,
        out_file,
        CompileOptions {
            arch: TargetArch::current().unwrap(),
            strict_verify: false,
        },
    )
    .unwrap();

    let mut registry = ImportRegistry::new();
    // Register with lowercase DLL name to verify case-insensitive matching against KERNEL32.dll
    registry.register_name("kernel32.dll", "AddTwoStdcall", shim_add_two_stdcall);

    Runtime::load_from_bytes(layout.pe_bytes.clone(), &temp.path, registry).unwrap()
}

#[test]
fn executes_direct_indirect_cpuid_rdtsc_and_trap_paths() {
    let layout = build_runtime_test_pe();
    let temp = TempAotFile::new("exec");
    let mut rt = compile_and_load_runtime(&layout, &temp);

    // Direct calls (forward + backward) + direct IAT import with stdcall args
    let entry_outcome = rt.run_entry().unwrap();
    assert_eq!(entry_outcome.exit_reason, ExitReason::Returned);
    assert_eq!(entry_outcome.eax, 67);

    // Indirect call through loaded IAT pointer -> synthetic thunk dispatch
    let indirect_iat = rt.call_va(layout.indirect_iat_va, &[]).unwrap();
    assert_eq!(indirect_iat.exit_reason, ExitReason::Returned);
    assert_eq!(indirect_iat.eax, 42);

    // Indirect call to internal guest function pointer
    let indirect_internal = rt.call_va(layout.indirect_internal_va, &[]).unwrap();
    assert_eq!(indirect_internal.exit_reason, ExitReason::Returned);
    assert_eq!(indirect_internal.eax, 27);

    // Unresolved import reports HostTrap + UnresolvedImport cleanly
    let missing_imp = rt.call_va(layout.unresolved_import_va, &[]).unwrap();
    assert_eq!(missing_imp.exit_reason, ExitReason::HostTrap);
    assert_eq!(missing_imp.exit_detail, layout.iat_missing_index);
    assert_eq!(
        missing_imp.trap,
        Some(TrapInfo::UnresolvedImport {
            index: layout.iat_missing_index,
            dll_name: "KERNEL32.dll".to_owned(),
            symbol: ImportSymbol::Name("MissingImport".to_owned()),
        })
    );

    // Unresolved indirect target reports UnmappedTarget + UnresolvedIndirectCall cleanly
    let missing_va = rt.call_va(layout.unresolved_indirect_va, &[]).unwrap();
    assert_eq!(missing_va.exit_reason, ExitReason::UnmappedTarget);
    assert_eq!(missing_va.exit_detail, 0x0050_1234);
    assert_eq!(
        missing_va.trap,
        Some(TrapInfo::UnresolvedIndirectCall(0x0050_1234))
    );

    // CPUID leaves 0 and 1
    let cpuid0 = rt.call_va(layout.cpuid_va, &[layout.scratch_va, 0]).unwrap();
    assert_eq!(cpuid0.exit_reason, ExitReason::Returned);
    assert_eq!(rt.arena().read_u32(layout.scratch_va).unwrap(), 1);
    assert_eq!(rt.arena().read_u32(layout.scratch_va + 4).unwrap(), 0x756E_6547);
    assert_eq!(rt.arena().read_u32(layout.scratch_va + 8).unwrap(), 0x6C65_746E);
    assert_eq!(rt.arena().read_u32(layout.scratch_va + 12).unwrap(), 0x4965_6E69);

    let cpuid1 = rt.call_va(layout.cpuid_va, &[layout.scratch_va, 1]).unwrap();
    assert_eq!(cpuid1.exit_reason, ExitReason::Returned);
    let edx = rt.arena().read_u32(layout.scratch_va + 12).unwrap();
    assert_ne!(edx & (1 << 25), 0);
    assert_ne!(edx & (1 << 26), 0);

    // RDTSC monotonic ticks
    let r1 = rt.call_va(layout.rdtsc_va, &[layout.scratch_va]).unwrap();
    assert_eq!(r1.exit_reason, ExitReason::Returned);
    let t1 = rt.arena().read_u64(layout.scratch_va).unwrap();
    let r2 = rt.call_va(layout.rdtsc_va, &[layout.scratch_va]).unwrap();
    assert_eq!(r2.exit_reason, ExitReason::Returned);
    let t2 = rt.arena().read_u64(layout.scratch_va).unwrap();
    assert!(t2 >= t1);
}
