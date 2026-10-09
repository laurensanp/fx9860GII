// Embeds the icon and version information into the Windows executable.
fn main() {
    println!("cargo:rerun-if-changed=res/fxemu.rc");
    println!("cargo:rerun-if-changed=res/fxemu.ico");
    println!("cargo:rerun-if-changed=../dump/fx9860gii2_full_4MB.bin");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("fxemu_res.o");
    let windres = std::env::var("WINDRES").unwrap_or_else(|_| "x86_64-w64-mingw32-windres".into());
    let status = std::process::Command::new(&windres)
        .args(["--input", "res/fxemu.rc", "--output-format=coff", "--output"])
        .arg(&out)
        .args(["--include-dir", "res"])
        .status();
    match status {
        Ok(s) if s.success() => println!("cargo:rustc-link-arg-bins={}", out.display()),
        _ => println!("cargo:warning=could not run {} - building without icon", windres),
    }
}
