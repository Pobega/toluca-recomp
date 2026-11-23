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
use toluca_runtime::runtime::COM_THUNK_BASE;
use toluca_runtime::{ImportRegistry, Runtime, Win32Config, register_all_win32};

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

struct TempDir {
    root: PathBuf,
    aot_path: PathBuf,
}

impl TempDir {
    fn new(prefix: &str) -> Self {
        let id = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "toluca_boot_d3d8_{prefix}_{}_{id}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let aot_path = root.join("toluca_code.bin");
        Self { root, aot_path }
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

struct X86Builder {
    code: Vec<u8>,
}

impl X86Builder {
    fn new() -> Self {
        Self { code: Vec::new() }
    }

    fn pos(&self) -> usize {
        self.code.len()
    }

    fn push_imm32(&mut self, v: u32) {
        self.code.push(0x68);
        self.code.extend_from_slice(&v.to_le_bytes());
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

    fn push_esp_disp8(&mut self, disp: u8) {
        self.code.extend_from_slice(&[0xFF, 0x74, 0x24, disp]);
    }

    fn mov_eax_esp_disp8(&mut self, disp: u8) {
        self.code.extend_from_slice(&[0x8B, 0x44, 0x24, disp]);
    }

    fn mov_ebx_eax(&mut self) {
        self.code.extend_from_slice(&[0x8B, 0xD8]);
    }

    fn mov_esi_eax(&mut self) {
        self.code.extend_from_slice(&[0x8B, 0xF0]);
    }

    fn mov_mem_eax(&mut self, addr: u32) {
        self.code.push(0xA3);
        self.code.extend_from_slice(&addr.to_le_bytes());
    }

    fn mov_mem_imm32(&mut self, addr: u32, val: u32) {
        self.code.extend_from_slice(&[0xC7, 0x05]);
        self.code.extend_from_slice(&addr.to_le_bytes());
        self.code.extend_from_slice(&val.to_le_bytes());
    }

    fn cmp_eax_imm32(&mut self, val: u32) {
        self.code.push(0x3D);
        self.code.extend_from_slice(&val.to_le_bytes());
    }

    fn jne_rel8_placeholder(&mut self) -> usize {
        self.code.extend_from_slice(&[0x75, 0x00]);
        self.code.len() - 1
    }

    fn patch_rel8(&mut self, disp_offset: usize, target: usize) {
        let next_ip = (disp_offset + 1) as isize;
        let delta = (target as isize) - next_ip;
        self.code[disp_offset] = i8::try_from(delta).unwrap() as u8;
    }

    fn call_iat(&mut self, iat_va: u32) {
        self.code.extend_from_slice(&[0xFF, 0x15]);
        self.code.extend_from_slice(&iat_va.to_le_bytes());
    }

    fn add_esp_imm8(&mut self, bytes: u8) {
        self.code.extend_from_slice(&[0x83, 0xC4, bytes]);
    }

    fn ret(&mut self) {
        self.code.push(0xC3);
    }

    fn ret_imm16(&mut self, bytes: u16) {
        self.code.push(0xC2);
        self.code.extend_from_slice(&bytes.to_le_bytes());
    }
}

struct RdataTable {
    image_base: u32,
    base_rva: u32,
    buf: Vec<u8>,
}

impl RdataTable {
    fn new(image_base: u32, base_rva: u32, reserve: usize) -> Self {
        Self {
            image_base,
            base_rva,
            buf: vec![0u8; reserve],
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

    fn write_u32_at(&mut self, off: usize, val: u32) {
        self.buf[off..off + 4].copy_from_slice(&val.to_le_bytes());
    }
}

fn append_import_dll(
    rdata: &mut RdataTable,
    desc_idx: usize,
    dll_name: &str,
    symbols: &[&str],
    ilt_off: usize,
    iat_off: usize,
) -> Vec<u32> {
    let dll_rva = rdata.push_cstr(dll_name) - rdata.image_base;
    let mut iat_vas = Vec::with_capacity(symbols.len());
    for (idx, sym) in symbols.iter().enumerate() {
        let hint_rva = rdata.push_hint_name(sym);
        rdata.write_u32_at(ilt_off + idx * 4, hint_rva);
        iat_vas.push(rdata.image_base + rdata.base_rva + (iat_off + idx * 4) as u32);
    }

    let desc_off = desc_idx * 20;
    rdata.write_u32_at(desc_off, rdata.base_rva + ilt_off as u32);
    rdata.write_u32_at(desc_off + 12, dll_rva);
    rdata.write_u32_at(desc_off + 16, rdata.base_rva + iat_off as u32);
    iat_vas
}

#[test]
fn milestone_gate_boots_synthetic_pe_through_window_pump_to_direct3d_create8() {
    let tmp = TempDir::new("milestone");
    let data_dir = tmp.root.join("data");
    std::fs::create_dir_all(&data_dir).unwrap();
    std::fs::write(data_dir.join("stage.txt"), b"hospital").unwrap();

    let image_base = DEFAULT_IMAGE_BASE;
    let text_rva = 0x1000u32;
    let rdata_rva = 0x2000u32;
    let data_rva = 0x3000u32;
    let data_va = image_base + data_rva;

    let mut rdata = RdataTable::new(image_base, rdata_rva, 0x300);
    let k32_iat = append_import_dll(
        &mut rdata,
        0,
        "KERNEL32.dll",
        &["CreateFileA", "ReadFile", "CloseHandle", "GetProcessHeap", "HeapAlloc"],
        0x80,
        0xC0,
    );
    let crt_iat = append_import_dll(&mut rdata, 1, "msvcrt.dll", &["sprintf"], 0x100, 0x110);
    let u32_iat = append_import_dll(
        &mut rdata,
        2,
        "USER32.dll",
        &[
            "RegisterClassA",
            "CreateWindowExA",
            "ShowWindow",
            "PostMessageA",
            "PeekMessageA",
            "DispatchMessageA",
            "DefWindowProcA",
        ],
        0x120,
        0x160,
    );
    let d3d_iat = append_import_dll(&mut rdata, 3, "d3d8.dll", &["Direct3DCreate8"], 0x1A0, 0x1B0);

    let str_stage_path = rdata.push_cstr(r"DATA\STAGE.TXT");
    let str_title_fmt = rdata.push_cstr("Toluca-%s");
    let str_class_name = rdata.push_cstr("TolucaClass");

    let bytes_read_va = data_va;
    let wm_create_seen_va = data_va + 0x04;
    let wm_app_wparam_va = data_va + 0x08;
    let hwnd_saved_va = data_va + 0x0C;
    let file_buf_va = data_va + 0x10;
    let title_ptr_va = data_va + 0x30;
    let wc_va = data_va + 0x40;
    let msg_va = data_va + 0x80;

    let wndproc_off = 0x200usize;
    let wndproc_va = image_base + text_rva + wndproc_off as u32;
    rdata.write_u32_at(0x280, wndproc_va);

    let mut entry = X86Builder::new();
    // CreateFileA("DATA\\STAGE.TXT", GENERIC_READ, 0, 0, OPEN_EXISTING, 0x80, 0)
    entry.push_imm32(0);
    entry.push_imm32(0x80);
    entry.push_imm32(3);
    entry.push_imm32(0);
    entry.push_imm32(0);
    entry.push_imm32(0x8000_0000);
    entry.push_imm32(str_stage_path);
    entry.call_iat(k32_iat[0]);
    entry.mov_esi_eax();

    // ReadFile(hFile, file_buf, 16, &bytes_read, NULL); CloseHandle(hFile)
    entry.push_imm32(0);
    entry.push_imm32(bytes_read_va);
    entry.push_imm32(16);
    entry.push_imm32(file_buf_va);
    entry.push_esi();
    entry.call_iat(k32_iat[1]);
    entry.push_esi();
    entry.call_iat(k32_iat[2]);

    // title_buf = HeapAlloc(GetProcessHeap(), HEAP_ZERO_MEMORY, 64)
    entry.call_iat(k32_iat[3]);
    entry.push_imm32(64);
    entry.push_imm32(0x08);
    entry.push_eax();
    entry.call_iat(k32_iat[4]);
    entry.mov_esi_eax();
    entry.mov_mem_eax(title_ptr_va);

    // sprintf(title_buf, "Toluca-%s", file_buf); add esp, 12
    entry.push_imm32(file_buf_va);
    entry.push_imm32(str_title_fmt);
    entry.push_esi();
    entry.call_iat(crt_iat[0]);
    entry.add_esp_imm8(12);

    // Populate WNDCLASSA { lpfnWndProc = wndproc_va, lpszClassName = "TolucaClass" } & RegisterClassA(&wc)
    entry.mov_mem_imm32(wc_va + 4, wndproc_va);
    entry.mov_mem_imm32(wc_va + 36, str_class_name);
    entry.push_imm32(wc_va);
    entry.call_iat(u32_iat[0]);

    // hwnd = CreateWindowExA(0, "TolucaClass", title_buf, WS_OVERLAPPEDWINDOW, 0, 0, 640, 480, 0, 0, 0, 0)
    entry.push_imm32(0);
    entry.push_imm32(0);
    entry.push_imm32(0);
    entry.push_imm32(0);
    entry.push_imm32(480);
    entry.push_imm32(640);
    entry.push_imm32(0);
    entry.push_imm32(0);
    entry.push_imm32(0x00CF_0000);
    entry.push_esi();
    entry.push_imm32(str_class_name);
    entry.push_imm32(0);
    entry.call_iat(u32_iat[1]);
    entry.mov_ebx_eax();
    entry.mov_mem_eax(hwnd_saved_va);

    // ShowWindow(hwnd, SW_SHOW=5)
    entry.push_imm32(5);
    entry.push_ebx();
    entry.call_iat(u32_iat[2]);

    // PostMessageA(hwnd, WM_APP + 1 = 0x8001, 0x1234, 0)
    entry.push_imm32(0);
    entry.push_imm32(0x1234);
    entry.push_imm32(0x8001);
    entry.push_ebx();
    entry.call_iat(u32_iat[3]);

    // PeekMessageA(&msg, 0, 0, 0, PM_REMOVE=1) -> DispatchMessageA(&msg)
    entry.push_imm32(1);
    entry.push_imm32(0);
    entry.push_imm32(0);
    entry.push_imm32(0);
    entry.push_imm32(msg_va);
    entry.call_iat(u32_iat[4]);
    entry.push_imm32(msg_va);
    entry.call_iat(u32_iat[5]);

    // return Direct3DCreate8(220)
    entry.push_imm32(220);
    entry.call_iat(d3d_iat[0]);
    entry.ret();

    let mut wndproc = X86Builder::new();
    wndproc.mov_eax_esp_disp8(8);
    wndproc.cmp_eax_imm32(1);
    let skip_create = wndproc.jne_rel8_placeholder();
    wndproc.mov_mem_imm32(wm_create_seen_va, 1);
    let after_create = wndproc.pos();
    wndproc.patch_rel8(skip_create, after_create);

    wndproc.cmp_eax_imm32(0x8001);
    let skip_app = wndproc.jne_rel8_placeholder();
    wndproc.mov_eax_esp_disp8(12);
    wndproc.mov_mem_eax(wm_app_wparam_va);
    let after_app = wndproc.pos();
    wndproc.patch_rel8(skip_app, after_app);

    wndproc.push_esp_disp8(16);
    wndproc.push_esp_disp8(16);
    wndproc.push_esp_disp8(16);
    wndproc.push_esp_disp8(16);
    wndproc.call_iat(u32_iat[6]);
    wndproc.ret_imm16(16);

    let mut text = vec![0xCCu8; 0x300];
    text[..entry.code.len()].copy_from_slice(&entry.code);
    text[wndproc_off..wndproc_off + wndproc.code.len()].copy_from_slice(&wndproc.code);

    let data_sec = vec![0u8; 0x100];
    let pe_bytes = build_test_pe32(
        image_base,
        text_rva,
        0x0102,
        &[(DIR_IMPORT, DataDirectory { rva: rdata_rva, size: 100 })],
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
        .open(&tmp.aot_path)
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
    register_all_win32(&mut registry);

    let config = Win32Config {
        game_root: tmp.root.clone(),
        headless: true,
        trace_imports: false,
        trace_d3d8: false,
        command_line: "sh2pc.exe".to_owned(),
        exe_name: "sh2pc.exe".to_owned(),
    };

    let mut rt = Runtime::load_from_bytes_with_config(pe_bytes, &tmp.aot_path, registry, config)
        .unwrap();
    let outcome = rt.run_entry().unwrap();

    assert_eq!(outcome.exit_reason, ExitReason::Returned);
    assert_eq!(rt.win32().d3d8_create_calls, vec![220]);

    let d3d8_obj = outcome.eax;
    assert_ne!(d3d8_obj, 0);
    let vtable_ptr = rt.arena().read_u32(d3d8_obj).unwrap();
    assert_ne!(vtable_ptr, 0);
    let first_method = rt.arena().read_u32(vtable_ptr).unwrap();
    assert_eq!(first_method, COM_THUNK_BASE);
    let create_device_method = rt.arena().read_u32(vtable_ptr + 15 * 4).unwrap();
    assert_eq!(create_device_method, COM_THUNK_BASE + 15 * 4);

    assert_eq!(rt.arena().read_u32(wm_create_seen_va).unwrap(), 1);
    assert_eq!(rt.arena().read_u32(wm_app_wparam_va).unwrap(), 0x1234);
    assert_ne!(rt.arena().read_u32(hwnd_saved_va).unwrap(), 0);

    assert_eq!(rt.arena().read_u32(bytes_read_va).unwrap(), 8);
    assert_eq!(rt.arena().read_bytes(file_buf_va, 8).unwrap(), b"hospital");

    let title_va = rt.arena().read_u32(title_ptr_va).unwrap();
    assert_eq!(rt.arena().read_cstr(title_va, 64).unwrap(), "Toluca-hospital");
}
