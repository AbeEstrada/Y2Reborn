//! The QR encoder against a real reader: zbar decodes every version (finder,
//! format and version bits, masking, interleaving and Reed-Solomon together).
//! Skipped where zbarimg is not installed.

#[test]
fn a_real_reader_decodes_every_version() {
    let Ok(zbar) = std::process::Command::new("zbarimg")
        .arg("--version")
        .output()
    else {
        eprintln!("zbarimg not installed; skipped");
        return;
    };
    if !zbar.status.success() {
        return;
    }
    let dir = std::env::temp_dir().join(format!("reborn-qr-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    for len in [5usize, 20, 40, 60, 90, 109, 130, 160, 190, 213] {
        let text: String = (0..len)
            .map(|i| (b'a' + (i * 7 % 26) as u8) as char)
            .collect();
        let code = reborn_ui::qr::encode(text.as_bytes()).unwrap();
        let (scale, border) = (6, 4);
        let w = (code.size + 2 * border) * scale;
        let mut pgm = format!("P5 {w} {w} 255\n").into_bytes();
        for py in 0..w {
            for px in 0..w {
                let (x, y) = (px / scale, py / scale);
                let inside =
                    x >= border && y >= border && x < border + code.size && y < border + code.size;
                pgm.push(if inside && code.dark(x - border, y - border) {
                    0
                } else {
                    255
                });
            }
        }
        let path = dir.join(format!("{len}.pgm"));
        std::fs::write(&path, pgm).unwrap();
        let out = std::process::Command::new("zbarimg")
            .args(["-q", "--raw"])
            .arg(&path)
            .output()
            .unwrap();
        assert_eq!(
            String::from_utf8_lossy(&out.stdout).trim_end(),
            text,
            "length {len}"
        );
    }
    std::fs::remove_dir_all(dir).unwrap();
}
