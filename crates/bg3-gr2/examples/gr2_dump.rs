//! Dump the generic object tree of a GR2 file.
//! `cargo run -p bg3-gr2 --example gr2_dump -- <file.gr2> [depth] [max_array]`
fn main() {
    let args: Vec<String> = std::env::args().collect();
    let data = std::fs::read(&args[1]).expect("read");
    let depth = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(6);
    let max_array = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(3);
    let file = bg3_gr2::Gr2File::parse(&data).expect("parse");
    eprintln!("{:?}", file);
    let types = bg3_gr2::Types::new(&file);
    let root = types.root().expect("root");
    let mut s = String::new();
    root.to_value(depth, max_array).write_tree(&mut s, 0);
    print!("{s}");
}
