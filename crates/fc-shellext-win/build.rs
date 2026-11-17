fn main() {
    // DllGetClassObject / DllCanUnloadNow are standard COM exports; MSVC's link.exe warns
    // (LNK4104) that they "should be PRIVATE" (i.e. not in an import library). Harmless here.
    let target = std::env::var("TARGET").unwrap_or_default();
    if target.contains("windows-msvc") {
        println!("cargo:rustc-cdylib-link-arg=/IGNORE:4104");
    }
    println!("cargo:rerun-if-changed=build.rs");
}
