//! Embeds the app icon into the Windows executable (Explorer, taskbar, shortcuts).
//! The icon is drawn by `src/icon.rs`, the same code the window icon uses.

#[path = "src/icon.rs"]
#[allow(dead_code)]
mod icon;

use std::path::PathBuf;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=src/icon.rs");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=WINDRES");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let out = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
    let ico = out.join("app.ico");
    let rc = out.join("app.rc");
    if let Err(e) = std::fs::write(&ico, icon::ico()).and_then(|_| std::fs::write(&rc, format!("1 ICON \"{}\"\n{}", ico.display().to_string().replace('\\', "/"), version_info()))) {
        println!("cargo:warning=could not write the app icon: {e}");
        return;
    }
    // MinGW windres: plain on Windows, prefixed when cross-compiling from Linux.
    let windres: Vec<String> = [std::env::var("WINDRES").ok(), Some("windres".into()), Some("x86_64-w64-mingw32-windres".into())].into_iter().flatten().collect();
    let msvc = std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc");
    let linked = if msvc {
        // rc.exe is only on PATH inside a Visual Studio prompt: look in the Windows SDK too.
        // (The MSVC linker also takes a .res made by windres, if that is all there is.)
        let res = out.join("app.res");
        let mut compilers = vec![PathBuf::from("rc.exe")];
        compilers.extend(sdk_resource_compilers());
        let ok = compilers.iter().any(|rc_exe| ran(Command::new(rc_exe).arg("/nologo").arg("/fo").arg(&res).arg(&rc), &rc_exe.display().to_string()))
            || windres.iter().any(|tool| ran(Command::new(tool).arg("--input").arg(&rc).arg("--output").arg(&res).arg("--output-format=res"), tool));
        ok.then_some(res)
    } else {
        let obj = out.join("app_icon.o");
        let ok = windres.iter().any(|tool| ran(Command::new(tool).arg("--input").arg(&rc).arg("--output").arg(&obj).arg("--output-format=coff"), tool));
        ok.then_some(obj)
    };
    match linked {
        Some(file) => println!("cargo:rustc-link-arg-bins={}", file.display()),
        None => println!("cargo:warning=no working resource compiler (rc.exe or windres): the exe gets no embedded icon or version details"),
    }
}

/// Runs a resource compiler. A tool that exists but fails says why (a missing one is just skipped).
fn ran(cmd: &mut Command, name: &str) -> bool {
    match cmd.output() {
        Ok(out) if out.status.success() => true,
        Ok(out) => {
            let log = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
            println!("cargo:warning={name} failed: {}", log.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("no message"));
            false
        }
        Err(e) => {
            if e.kind() != std::io::ErrorKind::NotFound {
                println!("cargo:warning={name} could not be started: {e}");
            }
            false
        }
    }
}

/// `rc.exe` of every installed Windows SDK, newest first.
fn sdk_resource_compilers() -> Vec<PathBuf> {
    let arch = if std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() == Ok("aarch64") { "arm64" } else { "x64" };
    let mut found = Vec::new();
    for root in ["ProgramFiles(x86)", "ProgramFiles"].iter().filter_map(|v| std::env::var_os(v)) {
        let bin = PathBuf::from(root).join("Windows Kits").join("10").join("bin");
        let Ok(entries) = std::fs::read_dir(&bin) else { continue };
        let mut versions: Vec<PathBuf> = entries.flatten().map(|e| e.path()).filter(|p| p.join(arch).join("rc.exe").is_file()).collect();
        versions.sort();
        found.extend(versions.into_iter().rev().map(|p| p.join(arch).join("rc.exe")));
    }
    found
}

/// The details Windows shows for the exe (Properties, Task Manager, "Open with"): the product
/// name and version, taken from Cargo.toml.
fn version_info() -> String {
    let version = std::env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.0.0".into());
    // Four numbers; a pre-release suffix ("0.3.0-beta.1") keeps its digits before the dash.
    let mut parts: Vec<u16> = version.split(['-', '+']).next().unwrap_or("").split('.').filter_map(|p| p.parse().ok()).collect();
    parts.resize(4, 0);
    let numbers = format!("{},{},{},{}", parts[0], parts[1], parts[2], parts[3]);
    format!(
        r#"1 VERSIONINFO
FILEVERSION {numbers}
PRODUCTVERSION {numbers}
FILEOS 0x40004
FILETYPE 0x1
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904B0"
    BEGIN
      VALUE "CompanyName", "AI Video Studio\0"
      VALUE "FileDescription", "AI Video Studio\0"
      VALUE "FileVersion", "{version}\0"
      VALUE "InternalName", "ai-video-studio\0"
      VALUE "OriginalFilename", "ai-video-studio.exe\0"
      VALUE "ProductName", "AI Video Studio\0"
      VALUE "ProductVersion", "{version}\0"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x409, 1200
  END
END
"#
    )
}
