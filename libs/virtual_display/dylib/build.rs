use cc;

fn build_c_impl() {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os != "windows" {
        return;
    }
    let mut build = cc::Build::new();

    build.file("src/win10/IddController.c");

    build.flag_if_supported("-Wno-c++0x-extensions");
    build.flag_if_supported("-Wno-return-type-c-linkage");
    build.flag_if_supported("-Wno-invalid-offsetof");
    build.flag_if_supported("-Wno-unused-parameter");

    if build.get_compiler().is_like_msvc() {
        build.define("WIN32", "");
        build.flag("-Z7");
        build.flag("-GR-");
        // build.flag("-std:c++11");
    } else {
        // build.flag("-fPIC");
        // build.flag("-std=c++11");
        // build.flag("-include");
        // build.flag(&confdefs_path.to_string_lossy());
    }

    build.compile("win_virtual_display");

    println!("cargo:rerun-if-changed=src/win10/IddController.c");
}

fn main() {
    build_c_impl();
}
