fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("linux") {
        // Keep upstream's Windows workspace checks usable; the binary there is a stub.
        return;
    }
    let pw = pkg_config::Config::new().atleast_version("0.3.65")
        .probe("libpipewire-0.3").expect("Install pipewire development headers");
    let opus = pkg_config::Config::new().probe("opus").expect("Install libopus development headers");
    println!("cargo:rustc-env=OPUS_VERSION={}", opus.version);
    println!("cargo:rerun-if-changed=src/pipewire.c");
    cc::Build::new().file("src/pipewire.c").includes(pw.include_paths)
        .flag("-std=gnu11").warnings(true).compile("dualwave_pipewire");
}
