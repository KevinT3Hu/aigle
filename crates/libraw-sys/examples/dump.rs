//! Dump LibRaw metadata and per-channel CFA means: `cargo run -p aigle-libraw-sys --example dump -- FILE…`
fn main() {
    for p in std::env::args().skip(1) {
        let mut r = aigle_libraw_sys::Raw::open(std::path::Path::new(&p)).unwrap();
        r.unpack(&|| false).unwrap();
        let m = r.meta().unwrap();
        println!(
            "{p}: lens {:?} / {:?} focal {} (35mm {}) range {:?} f/{}",
            m.lens_make, m.lens_model, m.focal_len, m.focal_35mm, m.lens_focal_range, m.aperture
        );
        println!(" {:?}", m.shot);
        println!(
            "{p}\n black {:?} white {} pattern {:?} cfa {:?}\n",
            m.black,
            m.white,
            m.black_pattern,
            &m.cfa[..2]
        );
        println!(
            " cam_mul {:?}\n pre_mul {:?}\n cam_xyz {:?}\n rgb_cam {:?}",
            m.cam_mul, m.pre_mul, m.cam_xyz, m.rgb_cam
        );
        let (cfa, _, _) = r.copy_cfa(m.width, m.height, None).unwrap();
        let mut s = [0f64; 3];
        let mut n = [0f64; 3];
        for y in 0..m.height as usize {
            for x in 0..m.width as usize {
                let c = m.cfa[y % 6][x % 6] as usize;
                s[c] += cfa[y * m.width as usize + x] as f64;
                n[c] += 1.0;
            }
        }
        println!(" cfa means {:?}", [s[0] / n[0], s[1] / n[1], s[2] / n[2]]);
    }
}
