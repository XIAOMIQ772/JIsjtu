fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }
    let source = "native/pdf_vision.m";
    println!("cargo:rerun-if-changed={source}");
    cc::Build::new()
        .file(source)
        .flag("-fobjc-arc")
        .compile("pdf_vision");
    for framework in ["Foundation", "CoreGraphics", "PDFKit", "Vision"] {
        println!("cargo:rustc-link-lib=framework={framework}");
    }
}
