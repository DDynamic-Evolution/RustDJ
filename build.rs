use std::{env, fs, path::PathBuf, process::Command};

fn main() {
    println!("cargo:rerun-if-changed=assets/branding/rustdj.ico");
    println!("cargo:rerun-if-changed=Cargo.toml");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows")
        || env::var("CARGO_CFG_TARGET_ENV").as_deref() != Ok("msvc")
    {
        return;
    }

    let arch = env::var("CARGO_CFG_TARGET_ARCH").expect("Cargo target architecture");
    let sdk = find_msvc_tools::find_windows_sdk(&arch)
        .expect("Install the Windows SDK to embed the RustDJ executable icon");
    let compiler = sdk
        .path()
        .map(|path| path.join("rc.exe"))
        .find(|path| path.is_file())
        .expect("Windows SDK resource compiler rc.exe not found");
    let out = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo output directory"));
    let icon = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap())
        .join("assets/branding/rustdj.ico")
        .to_string_lossy()
        .replace('\\', "/");
    let version = env::var("CARGO_PKG_VERSION").unwrap();
    let numeric_version = format!("{},0", version.replace('.', ","));
    let script = out.join("rustdj.rc");
    fs::write(
        &script,
        format!(
            r#"1 ICON "{icon}"
1 VERSIONINFO
 FILEVERSION {numeric_version}
 PRODUCTVERSION {numeric_version}
 FILEFLAGSMASK 0x3fL
 FILEFLAGS 0
 FILEOS 0x40004L
 FILETYPE 1
BEGIN
 BLOCK "StringFileInfo"
 BEGIN
  BLOCK "040904b0"
  BEGIN
   VALUE "FileDescription", "RustDJ - DJ mixing application\0"
   VALUE "FileVersion", "{version}\0"
   VALUE "InternalName", "rustdj\0"
   VALUE "OriginalFilename", "RustDJ.exe\0"
   VALUE "ProductName", "RustDJ\0"
   VALUE "ProductVersion", "{version}\0"
  END
 END
 BLOCK "VarFileInfo"
 BEGIN
  VALUE "Translation", 0x0409, 1200
 END
END
"#
        ),
    )
    .expect("Write Windows resource script");
    let resource = out.join("rustdj.res");
    let status = Command::new(compiler)
        .arg("/nologo")
        .arg("/fo")
        .arg(&resource)
        .arg(&script)
        .status()
        .expect("Run Windows resource compiler");
    assert!(status.success(), "Windows icon resource compilation failed");
    println!("cargo:rustc-link-arg-bin=rustdj={}", resource.display());
}
