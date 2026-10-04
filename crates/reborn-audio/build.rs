fn main() {
    let l = pkg_config::Config::new()
        .cargo_metadata(false)
        .probe("alsa")
        .expect("ALSA development library");
    let mut b = cc::Build::new();
    b.file("native/audio.c").flag("-std=c11");
    for p in l.include_paths {
        b.include(p);
    }
    b.compile("reborn_audio");
    // Emit ALSA after the static membrane: GNU ld --as-needed otherwise
    // discards libasound before seeing the membrane's unresolved symbols.
    pkg_config::Config::new()
        .probe("alsa")
        .expect("ALSA link metadata");
    println!("cargo:rerun-if-changed=native/audio.c");
}
