use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use toluca_core::aot::TargetArch;
use toluca_core::cpu::ExitReason;
use toluca_core::memory::DEFAULT_IMAGE_BASE;
use toluca_core::pe::test_helpers::{TestSectionSpec, build_test_pe32};
use toluca_core::pe::{
    DIR_IMPORT, DataDirectory, IMAGE_SCN_CNT_CODE, IMAGE_SCN_MEM_EXECUTE, IMAGE_SCN_MEM_READ,
    IMAGE_SCN_MEM_WRITE, PeImage,
};
use toluca_recomp::cfg::discover_functions;
use toluca_recomp::compiler::{CompileOptions, compile_pe_to_writer};
use toluca_runtime::{ImportRegistry, Runtime, Win32Config, register_kernel32_and_msvcrt};

static TEMP_ID: AtomicU64 = AtomicU64::new(0);

struct TempWorkspace {
    dir: PathBuf,
    aot_path: PathBuf,
}

impl TempWorkspace {
    fn new(prefix: &str) -> Self {
        let id = TEMP_ID.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "toluca_win32_test_{prefix}_{}_{id}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let aot_path = dir.join("test_code.bin");
        Self { dir, aot_path }
    }
}

impl Drop for TempWorkspace {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

struct X86Emitter {
    code: Vec<u8>,
}

impl X86Emitter {
    fn new() -> Self {
        Self { code: Vec::new() }
    }

    fn push_imm32(&mut self, val: u32) {
        self.code.push(0x68);
        self.code.extend_from_slice(&val.to_le_bytes());
    }

    fn push_eax(&mut self) {
        self.code.push(0x50);
    }

    fn push_ebx(&mut self) {
        self.code.push(0x53);
    }

    fn push_esi(&mut self) {
        self.code.push(0x56);
    }

    fn push_edi(&mut self) {
        self.code.push(0x57);
    }

    fn mov_ebx_eax(&mut self) {
        self.code.extend_from_slice(&[0x8B, 0xD8]);
    }

    fn mov_esi_eax(&mut self) {
        self.code.extend_from_slice(&[0x8B, 0xF0]);
    }

    fn mov_edi_eax(&mut self) {
        self.code.extend_from_slice(&[0x8B, 0xF8]);
    }

    fn mov_mem_eax(&mut self, addr: u32) {
        self.code.push(0xA3);
        self.code.extend_from_slice(&addr.to_le_bytes());
    }

    fn mov_edi_mem_imm32(&mut self, val: u32) {
        self.code.extend_from_slice(&[0xC7, 0x07]);
        self.code.extend_from_slice(&val.to_le_bytes());
    }

    fn call_iat(&mut self, iat_va: u32) {
        self.code.extend_from_slice(&[0xFF, 0x15]);
        self.code.extend_from_slice(&iat_va.to_le_bytes());
    }

    fn call_eax(&mut self) {
        self.code.extend_from_slice(&[0xFF, 0xD0]);
    }

    fn add_esp_imm8(&mut self, bytes: u8) {
        self.code.extend_from_slice(&[0x83, 0xC4, bytes]);
    }

    fn ret(&mut self) {
        self.code.push(0xC3);
    }
}

struct RdataBuilder {
    base_rva: u32,
    image_base: u32,
    buf: Vec<u8>,
}

impl RdataBuilder {
    fn new(image_base: u32, base_rva: u32, initial_reserve: usize) -> Self {
        Self {
            base_rva,
            image_base,
            buf: vec![0u8; initial_reserve],
        }
    }

    fn push_cstr(&mut self, s: &str) -> u32 {
        let rva = self.base_rva + self.buf.len() as u32;
        self.buf.extend_from_slice(s.as_bytes());
        self.buf.push(0);
        self.image_base + rva
    }

    fn push_hint_name(&mut self, name: &str) -> u32 {
        if !self.buf.len().is_multiple_of(2) {
            self.buf.push(0);
        }
        let rva = self.base_rva + self.buf.len() as u32;
        self.buf.extend_from_slice(&0u16.to_le_bytes());
        self.buf.extend_from_slice(name.as_bytes());
        self.buf.push(0);
        rva
    }

    fn write_u32_at(&mut self, offset: usize, val: u32) {
        self.buf[offset..offset + 4].copy_from_slice(&val.to_le_bytes());
    }
}

#[test]
fn executes_kernel32_and_msvcrt_shims_end_to_end() {
    let ws = TempWorkspace::new("shims");
    let data_dir = ws.dir.join("data");
    std::fs::create_dir_all(&data_dir).unwrap();
    std::fs::write(data_dir.join("test.txt"), b"TolucaOK!").unwrap();

    let image_base = DEFAULT_IMAGE_BASE;
    let text_rva = 0x1000u32;
    let rdata_rva = 0x2000u32;
    let data_rva = 0x3000u32;
    let data_va = image_base + data_rva;

    let k32_names = [
        "CreateFileA",
        "ReadFile",
        "GetFileSize",
        "CloseHandle",
        "GetProcessHeap",
        "HeapAlloc",
        "HeapFree",
        "VirtualAlloc",
        "VirtualFree",
        "TlsAlloc",
        "TlsSetValue",
        "TlsGetValue",
        "GetModuleHandleA",
        "GetProcAddress",
    ];
    let crt_names = [
        "sprintf", "strlen", "strcmp", "malloc", "free", "fopen", "fread", "fclose",
    ];

    // Reserve space for 3 import descriptors (60 bytes), ILT, IAT, and pointer table
    let mut rdata = RdataBuilder::new(image_base, rdata_rva, 0x300);
    let k32_dll_rva = rdata.push_cstr("KERNEL32.dll") - image_base;
    let crt_dll_rva = rdata.push_cstr("msvcrt.dll") - image_base;

    let str_file_path = rdata.push_cstr(r"DATA\Test.TXT");
    let str_k32_name = rdata.push_cstr("kernel32.dll");
    let str_get_tick = rdata.push_cstr("GetTickCount");
    let str_fmt = rdata.push_cstr("val=%d-%s");
    let str_ok = rdata.push_cstr("ok");
    let str_expected = rdata.push_cstr("val=42-ok");
    let str_rb = rdata.push_cstr("rb");

    let k32_ilt_off = 0x60usize;
    let k32_iat_off = 0xC0usize;
    let crt_ilt_off = 0x120usize;
    let crt_iat_off = 0x180usize;

    let mut k32_iat = Vec::new();
    for (idx, name) in k32_names.iter().enumerate() {
        let hint_rva = rdata.push_hint_name(name);
        rdata.write_u32_at(k32_ilt_off + idx * 4, hint_rva);
        k32_iat.push(image_base + rdata_rva + (k32_iat_off + idx * 4) as u32);
    }

    let mut crt_iat = Vec::new();
    for (idx, name) in crt_names.iter().enumerate() {
        let hint_rva = rdata.push_hint_name(name);
        rdata.write_u32_at(crt_ilt_off + idx * 4, hint_rva);
        crt_iat.push(image_base + rdata_rva + (crt_iat_off + idx * 4) as u32);
    }

    // Descriptor 0: KERNEL32.dll
    rdata.write_u32_at(0, rdata_rva + k32_ilt_off as u32);
    rdata.write_u32_at(12, k32_dll_rva);
    rdata.write_u32_at(16, rdata_rva + k32_iat_off as u32);
    // Descriptor 1: msvcrt.dll
    rdata.write_u32_at(20, rdata_rva + crt_ilt_off as u32);
    rdata.write_u32_at(32, crt_dll_rva);
    rdata.write_u32_at(36, rdata_rva + crt_iat_off as u32);

    // Scratch memory layout inside .data
    let bytes_read_va = data_va;
    let file_size_va = data_va + 0x04;
    let tls_val_va = data_va + 0x08;
    let tick_res_va = data_va + 0x0C;
    let read_buf_va = data_va + 0x10;
    let sprintf_buf_va = data_va + 0x50;
    let fopen_buf_va = data_va + 0x90;
    let strlen_va = data_va + 0x100;
    let strcmp_va = data_va + 0x104;
    let fread_cnt_va = data_va + 0x108;

    let mut k32_fn = X86Emitter::new();
    // CreateFileA(DATA\Test.TXT, GENERIC_READ=0x80000000, 0, 0, OPEN_EXISTING=3, 0x80, 0)
    k32_fn.push_imm32(0);
    k32_fn.push_imm32(0x80);
    k32_fn.push_imm32(3);
    k32_fn.push_imm32(0);
    k32_fn.push_imm32(0);
    k32_fn.push_imm32(0x8000_0000);
    k32_fn.push_imm32(str_file_path);
    k32_fn.call_iat(k32_iat[0]);
    k32_fn.mov_esi_eax();

    // GetFileSize(handle, NULL)
    k32_fn.push_imm32(0);
    k32_fn.push_esi();
    k32_fn.call_iat(k32_iat[2]);
    k32_fn.mov_mem_eax(file_size_va);

    // ReadFile(handle, read_buf, 16, &bytes_read, NULL)
    k32_fn.push_imm32(0);
    k32_fn.push_imm32(bytes_read_va);
    k32_fn.push_imm32(16);
    k32_fn.push_imm32(read_buf_va);
    k32_fn.push_esi();
    k32_fn.call_iat(k32_iat[1]);

    // CloseHandle(handle)
    k32_fn.push_esi();
    k32_fn.call_iat(k32_iat[3]);

    // GetProcessHeap -> HeapAlloc(heap, HEAP_ZERO_MEMORY=8, 64) -> write -> HeapFree
    k32_fn.call_iat(k32_iat[4]);
    k32_fn.mov_ebx_eax();
    k32_fn.push_imm32(64);
    k32_fn.push_imm32(0x08);
    k32_fn.push_ebx();
    k32_fn.call_iat(k32_iat[5]);
    k32_fn.mov_edi_eax();
    k32_fn.mov_edi_mem_imm32(0x1234_5678);
    k32_fn.push_edi();
    k32_fn.push_imm32(0);
    k32_fn.push_ebx();
    k32_fn.call_iat(k32_iat[6]);

    // VirtualAlloc(0, 0x10000, MEM_RESERVE|MEM_COMMIT=0x3000, PAGE_READWRITE=4) -> VirtualFree
    k32_fn.push_imm32(4);
    k32_fn.push_imm32(0x3000);
    k32_fn.push_imm32(0x1_0000);
    k32_fn.push_imm32(0);
    k32_fn.call_iat(k32_iat[7]);
    k32_fn.mov_edi_eax();
    k32_fn.mov_edi_mem_imm32(0xCAFE_BABE);
    k32_fn.push_imm32(0x8000);
    k32_fn.push_imm32(0);
    k32_fn.push_edi();
    k32_fn.call_iat(k32_iat[8]);

    // TlsAlloc -> TlsSetValue -> TlsGetValue
    k32_fn.call_iat(k32_iat[9]);
    k32_fn.mov_ebx_eax();
    k32_fn.push_imm32(0xABCD_1234);
    k32_fn.push_ebx();
    k32_fn.call_iat(k32_iat[10]);
    k32_fn.push_ebx();
    k32_fn.call_iat(k32_iat[11]);
    k32_fn.mov_mem_eax(tls_val_va);

    // GetModuleHandleA("kernel32.dll") -> GetProcAddress(hmod, "GetTickCount") -> call eax
    k32_fn.push_imm32(str_k32_name);
    k32_fn.call_iat(k32_iat[12]);
    k32_fn.push_imm32(str_get_tick);
    k32_fn.push_eax();
    k32_fn.call_iat(k32_iat[13]);
    k32_fn.call_eax();
    k32_fn.mov_mem_eax(tick_res_va);
    k32_fn.ret();

    let mut crt_fn = X86Emitter::new();
    // sprintf(sprintf_buf, "val=%d-%s", 42, "ok")
    crt_fn.push_imm32(str_ok);
    crt_fn.push_imm32(42);
    crt_fn.push_imm32(str_fmt);
    crt_fn.push_imm32(sprintf_buf_va);
    crt_fn.call_iat(crt_iat[0]);
    crt_fn.add_esp_imm8(16);

    // strlen(sprintf_buf)
    crt_fn.push_imm32(sprintf_buf_va);
    crt_fn.call_iat(crt_iat[1]);
    crt_fn.add_esp_imm8(4);
    crt_fn.mov_mem_eax(strlen_va);

    // strcmp(sprintf_buf, "val=42-ok")
    crt_fn.push_imm32(str_expected);
    crt_fn.push_imm32(sprintf_buf_va);
    crt_fn.call_iat(crt_iat[2]);
    crt_fn.add_esp_imm8(8);
    crt_fn.mov_mem_eax(strcmp_va);

    // malloc(32) -> write -> free
    crt_fn.push_imm32(32);
    crt_fn.call_iat(crt_iat[3]);
    crt_fn.add_esp_imm8(4);
    crt_fn.mov_edi_eax();
    crt_fn.mov_edi_mem_imm32(0xDEAD_BEEF);
    crt_fn.push_edi();
    crt_fn.call_iat(crt_iat[4]);
    crt_fn.add_esp_imm8(4);

    // fopen("DATA\\Test.TXT", "rb") -> fread(fopen_buf, 1, 9, fp) -> fclose(fp)
    crt_fn.push_imm32(str_rb);
    crt_fn.push_imm32(str_file_path);
    crt_fn.call_iat(crt_iat[5]);
    crt_fn.add_esp_imm8(8);
    crt_fn.mov_esi_eax();

    crt_fn.push_esi();
    crt_fn.push_imm32(9);
    crt_fn.push_imm32(1);
    crt_fn.push_imm32(fopen_buf_va);
    crt_fn.call_iat(crt_iat[6]);
    crt_fn.add_esp_imm8(16);
    crt_fn.mov_mem_eax(fread_cnt_va);

    crt_fn.push_esi();
    crt_fn.call_iat(crt_iat[7]);
    crt_fn.add_esp_imm8(4);
    crt_fn.ret();

    let mut text = vec![0xCCu8; 0x300];
    let k32_off = 0x00usize;
    text[k32_off..k32_off + k32_fn.code.len()].copy_from_slice(&k32_fn.code);
    let crt_off = 0x180usize;
    text[crt_off..crt_off + crt_fn.code.len()].copy_from_slice(&crt_fn.code);

    let crt_fn_va = image_base + text_rva + crt_off as u32;
    rdata.write_u32_at(0x280, crt_fn_va);

    let data_sec = vec![0u8; 0x200];
    let pe_bytes = build_test_pe32(
        image_base,
        text_rva,
        0x0102,
        &[(DIR_IMPORT, DataDirectory { rva: rdata_rva, size: 60 })],
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
                data: &rdata.buf,
            },
            TestSectionSpec {
                name: ".data",
                virtual_address: data_rva,
                characteristics: IMAGE_SCN_MEM_READ | IMAGE_SCN_MEM_WRITE,
                data: &data_sec,
            },
        ],
    );

    let image = PeImage::parse(pe_bytes.clone()).unwrap();
    let discovery = discover_functions(&image);
    let out_file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(&ws.aot_path)
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
    register_kernel32_and_msvcrt(&mut registry);

    let config = Win32Config {
        game_root: ws.dir.clone(),
        headless: true,
        trace_imports: false,
        trace_d3d8: false,
        shader_cache_path: None,
        max_frames: None,
        command_line: "sh2pc.exe -test".to_owned(),
        exe_name: "sh2pc.exe".to_owned(),
        script_keys: Vec::new(),
    };
    let mut rt = Runtime::load_from_bytes_with_config(pe_bytes, &ws.aot_path, registry, config)
        .unwrap();

    let k32_outcome = rt.run_entry().unwrap();
    assert_eq!(k32_outcome.exit_reason, ExitReason::Returned);
    assert_eq!(rt.arena().read_u32(file_size_va).unwrap(), 9);
    assert_eq!(rt.arena().read_u32(bytes_read_va).unwrap(), 9);
    assert_eq!(rt.arena().read_bytes(read_buf_va, 9).unwrap(), b"TolucaOK!");
    assert_eq!(rt.arena().read_u32(tls_val_va).unwrap(), 0xABCD_1234);

    let crt_outcome = rt.call_va(crt_fn_va, &[]).unwrap();
    assert_eq!(crt_outcome.exit_reason, ExitReason::Returned);
    assert_eq!(rt.arena().read_cstr(sprintf_buf_va, 64).unwrap(), "val=42-ok");
    assert_eq!(rt.arena().read_u32(strlen_va).unwrap(), 9);
    assert_eq!(rt.arena().read_u32(strcmp_va).unwrap(), 0);
    assert_eq!(rt.arena().read_u32(fread_cnt_va).unwrap(), 9);
    assert_eq!(rt.arena().read_bytes(fopen_buf_va, 9).unwrap(), b"TolucaOK!");
}
