fn main() {
    let linker_script = concat!(env!("CARGO_MANIFEST_DIR"), "/src/arch/aarch64/link.ld");
    println!("cargo:rustc-link-arg=-T{linker_script}");
    println!("cargo:rerun-if-changed={linker_script}");
}
