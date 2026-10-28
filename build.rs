fn main() {
    println!("cargo:rustc-link-search=/usr/lib/libreoffice/program");
    println!("cargo:rustc-link-lib=sofficeapp");

    let bindings = bindgen::Builder::default()
        .header("/usr/include/LibreOfficeKit/LibreOfficeKit.hxx")
        .clang_arg("-I/usr/include/LibreOfficeKit")
        .clang_arg("-std=c++17")
        .clang_arg("-x")
        .clang_arg("c++")
        .parse_callbacks(Box::new(bindgen::CargoCallbacks::new()))
        .generate()
        .expect("无法生成 LOK 绑定");

    bindings
        .write_to_file("src/bindings.rs")
        .expect("无法写入 bindings.rs");
}
