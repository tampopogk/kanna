fn main() {
    let args: Vec<String> = std::env::args().collect();
    let text = std::fs::read_to_string(&args[2]).unwrap();
    let lang: tree_sitter::Language = if args[1] == "rust" { tree_sitter_rust::LANGUAGE.into() } else { tree_sitter_swift::LANGUAGE.into() };
    let mut p = tree_sitter::Parser::new();
    p.set_language(&lang).unwrap();
    let t = p.parse(&text, None).unwrap();
    fn walk(n: tree_sitter::Node, d: usize, src: &str) {
        let s = &src[n.byte_range()];
        let s: String = s.chars().take(40).collect::<String>().replace('\n', " ");
        println!("{}{} [{}] {:?}", "  ".repeat(d), n.kind(), n.start_position().row + 1, if n.child_count() == 0 { s } else { String::new() });
        let mut c = n.walk();
        for ch in n.children(&mut c) { walk(ch, d + 1, src); }
    }
    walk(t.root_node(), 0, &text);
}
