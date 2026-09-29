fn build_windows() {
    let file = "src/platform/windows_stub.c";
    cc::Build::new()
        .file(file)
        .compile("windows");
    println!("cargo:rustc-link-lib=WtsApi32");
    println!("cargo:rerun-if-changed={}", file);
}

#[cfg(target_os = "macos")]
fn build_mac() {
    let file = "src/platform/macos.mm";
    let mut b = cc::Build::new();
    if let Ok(os_version::OsVersion::MacOS(v)) = os_version::detect() {
        let v = v.version;
        if v.contains("10.14") {
            b.flag("-DNO_InputMonitoringAuthStatus=1");
        }
    }
    b.flag("-std=c++17").file(file).compile("macos");
    println!("cargo:rerun-if-changed={}", file);
}

fn build_manifest() {
    use std::io::Write;
    if std::env::var("PROFILE").unwrap() == "release" {
        let mut res = winres::WindowsResource::new();
        const LANG_ENGLISH: u16 = 0x09;
        const SUBLANG_ENGLISH_US: u16 = 0x01;
        const MAKELANGID: u16 = (SUBLANG_ENGLISH_US << 10) | LANG_ENGLISH;
        res.set_icon("res/icon.ico")
            .set_language(MAKELANGID)
            .set_manifest_file("res/manifest.xml");
        match res.compile() {
            Err(e) => {
                write!(std::io::stderr(), "{}", e).unwrap();
                std::process::exit(1);
            }
            Ok(_) => {}
        }
    }
}

// bionic only exports getifaddrs()/freeifaddrs() from API 24, while the jniLibs
// are built against the API 21 sysroot (flutter/ndk_*.sh). webrtc-util calls
// them, so without this the android link fails on undefined symbols.
fn build_android_ifaddrs() {
    let file = "src/platform/android_ifaddrs.c";
    cc::Build::new().file(file).compile("android_ifaddrs");
    println!("cargo:rerun-if-changed={}", file);
}

fn install_android_deps() {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap();
    if target_os != "android" {
        return;
    }
    let mut target_arch = std::env::var("CARGO_CFG_TARGET_ARCH").unwrap();
    if target_arch == "x86_64" {
        target_arch = "x64".to_owned();
    } else if target_arch == "x86" {
        target_arch = "x86".to_owned();
    } else if target_arch == "aarch64" {
        target_arch = "arm64".to_owned();
    } else {
        target_arch = "arm".to_owned();
    }
    let target = format!("{}-android", target_arch);
    let vcpkg_root = std::env::var("VCPKG_ROOT").unwrap();
    let mut path: std::path::PathBuf = vcpkg_root.into();
    if let Ok(vcpkg_root) = std::env::var("VCPKG_INSTALLED_ROOT") {
        path = vcpkg_root.into();
    } else {
        path.push("installed");
    }
    path.push(target);
    println!(
        "cargo:rustc-link-search={}",
        path.join("lib").to_str().unwrap()
    );
    println!("cargo:rustc-link-lib=ndk_compat");
    println!("cargo:rustc-link-lib=c++");
    println!("cargo:rustc-link-lib=OpenSLES");
}

fn find_native_lib_dir(crate_name: &str) -> Option<String> {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").ok()?;
    let target_dir = std::path::Path::new(&manifest_dir)
        .join("target")
        .join("x86_64-pc-windows-gnu")
        .join("release")
        .join("build");
    let lib_name = format!("lib{}.a", crate_name);
    for entry in std::fs::read_dir(&target_dir).ok()?.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with(&format!("{}-", crate_name)) {
            let out = entry.path().join("out");
            if out.join(&lib_name).exists() {
                return Some(out.to_string_lossy().to_string());
            }
        }
    }
    None
}

fn main() {
    hbb_common::gen_version();
    install_android_deps();
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap();
    if target_os == "windows" {
        if cfg!(feature = "inline") {
            build_manifest();
        }
        build_windows();
        // Link vcpkg static libraries for Windows cross-compilation
        let vcpkg_lib = "/opt/vcpkg/installed/x64-mingw-static/lib";
        println!("cargo:rustc-link-search={}", vcpkg_lib);
        println!("cargo:rustc-link-lib=static=vpx");
        println!("cargo:rustc-link-lib=static=yuv");
        println!("cargo:rustc-link-lib=static=opus");
        println!("cargo:rustc-link-lib=static=aom");
        println!("cargo:rustc-link-lib=static=jpeg");
        // machine-uid crate's native lib is not propagated without `links` key
        if let Some(dir) = find_native_lib_dir("machine-uid") {
            println!("cargo:rustc-link-search=native={}", dir);
            println!("cargo:rustc-link-lib=static=machine-uid");
        }
        // COM interface IID symbols (IID_IDataObject, IID_IStream, etc.)
        println!("cargo:rustc-link-lib=uuid");
        println!("cargo:rustc-link-lib=ole32");
        println!("cargo:rustc-link-lib=oleaut32");
    }
    if target_os == "macos" {
        #[cfg(target_os = "macos")]
        build_mac();
        println!("cargo:rustc-link-lib=framework=ApplicationServices");
    }
    if target_os == "android" {
        build_android_ifaddrs();
    }
    println!("cargo:rerun-if-changed=build.rs");
}
