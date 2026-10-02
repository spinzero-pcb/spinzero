//! Print the net of given pads on a board: `orcad_padnets <brd> R9.2 R7.1 ...`.
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let b = extract::orcad::pcb::load_board(std::path::Path::new(&a[1])).unwrap();
    let nl = extract::orcad::pcb::board_netlist(&b);
    for q in &a[2..] {
        let (r, p) = q.split_once('.').unwrap();
        let net = nl.iter().find(|(_, t)| t.iter().any(|(d, n)| d == r && n == p)).map(|(n, t)| format!("{n} {:?}", t));
        println!("{q}: {}", net.unwrap_or("-".into()));
    }
}
