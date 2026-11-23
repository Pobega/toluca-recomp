fn main() {
    println!(
        "toluca-runtime v{} (AOT format v{})",
        env!("CARGO_PKG_VERSION"),
        toluca_core::AOT_FORMAT_VERSION
    );
}
