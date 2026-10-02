//! Padstacks, and every placed pad's position computed from its footprint
//! (local position rotated, mirrored for the bottom) against the centre of
//! the bounding box the pad block itself stores.
use eda_parse_orcad::allegro::{self, Data};

fn main() {
    let path = std::env::args().nth(1).unwrap();
    let verbose = std::env::args().nth(2).is_some();
    let db = allegro::open(std::path::Path::new(&path)).unwrap();
    let sc = db.header.scale();
    for b in &db.stream.blocks {
        if let Data::Padstack(p) = &b.data {
            if verbose {
                let comps: Vec<String> = p.comps.iter().map(|c| format!("{:x}:{}x{}", c.kind, c.w as f64 * sc, c.h as f64 * sc)).collect();
                println!("PS {} start {} n {} drill {}x{} plated {} kind {:?} fixed {} per {} {:?}",
                    db.s(p.name), p.start_layer, p.layer_count, p.drill_w as f64 * sc, p.drill_h as f64 * sc, p.plated, p.kind, p.fixed, p.per_layer, comps);
            }
        }
    }
    let (mut ok, mut bad) = (0, 0);
    for b in &db.stream.blocks {
        let Data::PlacedPad { parent_fp, pad, coords, layer, .. } = &b.data else { continue };
        let Some(Data::FootprintInst { bottom, rotation, at, .. }) = db.stream.get(*parent_fp).map(|x| &x.data) else { continue };
        let Some(Data::Pad { at: lp, rotation: pr, name, .. }) = db.stream.get(*pad).map(|x| &x.data) else { continue };
        let a = (*rotation as f64 / 1000.0).to_radians();
        let lx = if *bottom { -(lp.0 as f64) } else { lp.0 as f64 };
        let ly = lp.1 as f64;
        let x = at.0 as f64 + lx * a.cos() - ly * a.sin();
        let y = at.1 as f64 + lx * a.sin() + ly * a.cos();
        let cx = (coords[0] as f64 + coords[2] as f64) / 2.0;
        let cy = (coords[1] as f64 + coords[3] as f64) / 2.0;
        let d = ((x - cx).hypot(y - cy)) * sc;
        if d < 0.002 { ok += 1 } else {
            bad += 1;
            if bad < 6 { println!("pad {} rot {} pr {} bottom {bottom} calc ({:.3},{:.3}) box ({:.3},{:.3}) layer {layer:?}", db.s(*name), rotation, pr, x * sc, y * sc, cx * sc, cy * sc); }
        }
    }
    println!("{ok} ok, {bad} off  {path}");
}
