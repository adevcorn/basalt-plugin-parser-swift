fn main() {
    if std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() != Ok("wasm32") { return; }
    // tree-sitter-swift compiles into libparser.a
    println!("cargo:rustc-link-arg=--whole-archive");
    println!("cargo:rustc-link-arg=-lparser");
    println!("cargo:rustc-link-arg=--no-whole-archive");
}
