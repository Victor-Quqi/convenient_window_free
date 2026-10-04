use std::{env, path::PathBuf, process::Command};
fn main() {
    println!("cargo:rerun-if-changed=native/taskbar-appearance.cpp");
    println!("cargo:rerun-if-changed=native/taskbar-appearance-policy.h");
    println!("cargo:rerun-if-changed=native/build.ps1");
    println!("cargo:rerun-if-changed=native/taskbar-appearance.def");
    println!("cargo:rerun-if-env-changed=VSINSTALLDIR");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    if env::var("CARGO_CFG_TARGET_ARCH").as_deref() != Ok("x86_64") {
        return;
    }
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let status = Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
            "native/build.ps1",
            "-OutputDir",
        ])
        .arg(&out)
        .status()
        .expect("run native taskbar compiler");
    assert!(status.success(), "Native taskbar component failed to compile; install VS C++ Build Tools and Windows SDK or set VSINSTALLDIR");
    let dll = out.join("taskbar-appearance.dll");
    println!(
        "cargo:rustc-env=CW_TASKBAR_APPEARANCE_DLL={}",
        dll.display()
    );
}
