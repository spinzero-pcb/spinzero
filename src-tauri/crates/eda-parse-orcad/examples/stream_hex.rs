//! Hex dump part of one stream of a compound file:
//! `stream_hex <file> <stream path> [offset] [len]`; with no stream, list them.
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let bytes = std::fs::read(&a[1]).unwrap();
    let cfb = ole_cfb::Cfb::parse(&bytes).unwrap();
    let Some(path) = a.get(2) else {
        for p in cfb.paths() {
            println!("{p} {}", cfb.stream(p).map(|d| d.len()).unwrap_or(0));
        }
        return;
    };
    // `#N` picks the Nth listed stream, for names holding control bytes.
    let path = match path.strip_prefix('#') {
        Some(n) => cfb.paths().nth(n.parse::<usize>().unwrap()).unwrap().to_string(),
        None => path.clone(),
    };
    let d = cfb.stream(&path).expect("stream");
    let off: usize = a.get(3).and_then(|s| s.parse().ok()).unwrap_or(0);
    let len: usize = a.get(4).and_then(|s| s.parse().ok()).unwrap_or(256);
    println!("{} bytes", d.len());
    for (i, ch) in d[off.min(d.len())..(off + len).min(d.len())].chunks(16).enumerate() {
        let hex: Vec<String> = ch.iter().map(|b| format!("{b:02x}")).collect();
        let asc: String = ch.iter().map(|&b| if (32..127).contains(&b) { b as char } else { '.' }).collect();
        println!("{:06x}  {:<48} {asc}", off + i * 16, hex.join(" "));
    }
}
