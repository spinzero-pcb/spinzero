//! Print a board's net classes with their member counts.
fn main() {
    let p = std::env::args().nth(1).unwrap();
    let b = extract::orcad::pcb::load_board(std::path::Path::new(&p)).unwrap();
    for (c, m) in extract::orcad::pcb::net_classes(&b) {
        println!("{c:<24} {:>4}  {}", m.len(), m.iter().take(6).cloned().collect::<Vec<_>>().join(" "));
    }
}
