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
    //
    // This first probe only reads the include paths. It must not print the
    // link directives yet: rustc links libraries in the order the build
    // script names them, and GNU ld with --as-needed drops a shared library
    // that comes before the static shim which uses it. rust-lld does not
    // care about the order, so the fault shows only with the system linker.
    let lib = probe(false);

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

    // Now name the FreeRDP libraries, after the shim, so the linker sees
    // the shim's undefined symbols before it reaches the libraries.
    probe(true);
}

fn probe(link: bool) -> pkg_config::Library {
    pkg_config::Config::new()
        .atleast_version("3.0.0")
        .cargo_metadata(link)
        .probe("freerdp-client3")
        .expect(
            "FreeRDP 3 was not found. Install freerdp-devel, or run \
             \"just freerdp-sysroot\" and export the variables it prints.",
        )
}
