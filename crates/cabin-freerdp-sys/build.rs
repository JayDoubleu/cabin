//! Compiles the FreeRDP shim and links against FreeRDP 3.
//!
//! FreeRDP is found through pkg-config. On a system without it installed,
//! run "just freerdp-sysroot" and export the variables it prints.

fn main() {
    println!("cargo:rerun-if-changed=src/shim.c");
    println!("cargo:rerun-if-changed=src/shim.h");
    println!("cargo:rerun-if-env-changed=PKG_CONFIG_PATH");
    println!("cargo:rerun-if-env-changed=PKG_CONFIG_SYSROOT_DIR");

    // freerdp-client3 pulls in freerdp3 and winpr3, and carries the client
    // entry points the shim uses.
    let lib = pkg_config::Config::new()
        .atleast_version("3.0.0")
        .probe("freerdp-client3")
        .expect(
            "FreeRDP 3 was not found. Install freerdp-devel, or run \
             \"just freerdp-sysroot\" and export the variables it prints.",
        );

    let mut build = cc::Build::new();
    build.file("src/shim.c").std("gnu17").warnings(true);
    // FreeRDP's own headers use symbols it has since deprecated, so every
    // build prints pages of warnings about code that is not ours. Silence
    // those specifically, and keep the rest.
    build.flag_if_supported("-Wno-deprecated-declarations");
    for path in &lib.include_paths {
        build.include(path);
    }
    build.compile("cabin_freerdp_shim");
}
