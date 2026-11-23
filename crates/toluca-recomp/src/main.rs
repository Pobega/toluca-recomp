fn main() {
    println!(
        "toluca-recomp v{} (AOT format v{})",
        env!("CARGO_PKG_VERSION"),
        toluca_core::AOT_FORMAT_VERSION
    );
}
