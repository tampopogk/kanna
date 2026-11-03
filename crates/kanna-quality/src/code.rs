use std::collections::{BTreeSet, HashMap};
use tree_sitter::{Node, Parser};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Lang {
    Rust,
    Swift,
}

impl Lang {
    pub fn of(path: &str) -> Option<Lang> {
        match path.rsplit_once('.')?.1 {
            "rs" => Some(Lang::Rust),
            "swift" => Some(Lang::Swift),
            _ => None,
        }
    }

    fn grammar(self) -> tree_sitter::Language {
        match self {
            Lang::Rust => tree_sitter_rust::LANGUAGE.into(),
            Lang::Swift => tree_sitter_swift::LANGUAGE.into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Function {
    pub name: String,
    pub line: usize,
    pub end: usize,
    pub lines: usize,
    pub cyclomatic: usize,
    pub cognitive: usize,
    pub nesting: usize,
    pub calls: BTreeSet<String>,
    pub wrapper: bool,
    pub exempt: bool,
}

impl Function {
    pub fn mass(&self) -> f64 {
        self.cyclomatic as f64 * (self.lines as f64).sqrt()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Block {
    pub hash: u64,
    pub line: usize,
    pub end: usize,
    pub lines: Vec<usize>,
}

#[derive(Clone, Debug, Default)]
pub struct Facts {
    pub lines: usize,
    pub test_lines: usize,
    pub functions: Vec<Function>,
    pub blocks: Vec<Block>,
    pub names: HashMap<String, usize>,
}

pub fn analyze(lang: Lang, text: &str) -> Facts {
    let mut parser = Parser::new();
    if parser.set_language(&lang.grammar()).is_err() {
        return Facts::default();
    }
    let Some(tree) = parser.parse(text, None) else { return Facts::default() };
    let mut walk = Walk { lang, text, facts: Facts::default(), code: BTreeSet::new(), test: BTreeSet::new() };
    walk.visit(tree.root_node(), false, false);
    walk.facts.lines = walk.code.len();
    walk.facts.test_lines = walk.test.len();
    walk.facts
}

struct Walk<'a> {
    lang: Lang,
    text: &'a str,
    facts: Facts,
    code: BTreeSet<usize>,
    test: BTreeSet<usize>,
}

pub const BLOCK_LINES: usize = 3;

fn feed(hash: u64, text: &str) -> u64 {
    text.bytes().chain([0]).fold(hash, |h, b| (h ^ b as u64).wrapping_mul(0x100000001b3))
}

fn is_block(lang: Lang, kind: &str) -> bool {
    match lang {
        Lang::Rust => matches!(kind, "function_item" | "if_expression" | "for_expression" | "while_expression" | "loop_expression" | "match_expression"),
        Lang::Swift => matches!(kind, "function_declaration" | "init_declaration" | "if_statement" | "guard_statement" | "for_statement" | "while_statement" | "repeat_while_statement" | "switch_statement" | "do_statement"),
    }
}

fn is_identifier(kind: &str) -> bool {
    matches!(kind, "identifier" | "simple_identifier" | "field_identifier" | "type_identifier" | "shorthand_field_identifier")
}

fn literal(kind: &str) -> Option<&'static str> {
    match kind {
        "string_literal" | "raw_string_literal" | "char_literal" | "line_string_literal" | "multi_line_string_literal" | "raw_string_literal_swift" => Some("$STR"),
        "integer_literal" | "hex_literal" | "oct_literal" | "bin_literal" => Some("$INT"),
        "float_literal" | "real_literal" => Some("$FLOAT"),
        "boolean_literal" => Some("$BOOL"),
        "nil" => Some("$NONE"),
        _ => None,
    }
}

fn is_comment(kind: &str) -> bool {
    matches!(kind, "line_comment" | "block_comment" | "comment" | "multiline_comment")
}

fn is_function(lang: Lang, node: Node) -> bool {
    match lang {
        Lang::Rust => node.kind() == "function_item",
        Lang::Swift => matches!(node.kind(), "function_declaration" | "init_declaration" | "deinit_declaration" | "computed_property" | "subscript_declaration"),
    }
}

fn is_closure(lang: Lang, kind: &str) -> bool {
    match lang {
        Lang::Rust => kind == "closure_expression",
        Lang::Swift => kind == "lambda_literal",
    }
}

impl<'a> Walk<'a> {
    fn text_of(&self, node: Node) -> &'a str {
        &self.text[node.byte_range()]
    }

    fn is_test(&self, node: Node) -> bool {
        match self.lang {
            Lang::Rust => {
                let mut previous = node.prev_sibling();
                while let Some(attribute) = previous.filter(|p| p.kind() == "attribute_item" || is_comment(p.kind())) {
                    let text = self.text_of(attribute);
                    if text.starts_with("#[cfg(test)]") || text.starts_with("#[test]") {
                        return true;
                    }
                    previous = attribute.prev_sibling();
                }
                false
            }
            Lang::Swift => false,
        }
    }

    fn visit(&mut self, node: Node, in_test: bool, in_function: bool) {
        let in_test = in_test || ((node.kind() == "mod_item" || node.kind() == "function_item") && self.is_test(node));
        if node.child_count() == 0 || literal(node.kind()).is_some() {
            if !is_comment(node.kind()) {
                let lines = node.start_position().row + 1..=node.end_position().row + 1;
                if in_test {
                    self.test.extend(lines);
                } else {
                    self.code.extend(lines);
                    if is_identifier(node.kind()) {
                        *self.facts.names.entry(self.text_of(node).to_string()).or_default() += 1;
                    }
                }
            }
            return;
        }
        if is_comment(node.kind()) {
            return;
        }
        if !in_test && is_function(self.lang, node) {
            let function = self.function(node);
            self.facts.functions.push(function);
        }
        if !in_test && is_block(self.lang, node.kind()) {
            let mut names = HashMap::new();
            let mut lines = BTreeSet::new();
            let hash = self.normalize(node, &mut names, 0xcbf29ce484222325, &mut lines);
            if lines.len() >= BLOCK_LINES {
                self.facts.blocks.push(Block { hash: feed(hash, node.kind()), line: node.start_position().row + 1, end: node.end_position().row + 1, lines: lines.into_iter().collect() });
            }
        }
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            self.visit(child, in_test, in_function || is_function(self.lang, node));
        }
    }

    fn normalize(&self, node: Node<'a>, names: &mut HashMap<&'a str, usize>, hash: u64, lines: &mut BTreeSet<usize>) -> u64 {
        let kind = node.kind();
        if is_comment(kind) {
            return hash;
        }
        if node.child_count() == 0 || literal(kind).is_some() {
            lines.extend(node.start_position().row + 1..=node.end_position().row + 1);
            let text = self.text_of(node);
            if let Some(placeholder) = literal(kind) {
                return feed(hash, placeholder);
            }
            if is_identifier(kind) {
                let next = names.len();
                let index = *names.entry(text).or_insert(next);
                return feed(hash, &format!("$VAR{index}"));
            }
            return feed(hash, text);
        }
        let mut hash = hash;
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            hash = self.normalize(child, names, hash, lines);
        }
        hash
    }

    fn name(&self, node: Node) -> String {
        if self.lang == Lang::Swift {
            match node.kind() {
                "init_declaration" => return "init".into(),
                "deinit_declaration" => return "deinit".into(),
                "subscript_declaration" => return "subscript".into(),
                "computed_property" => {
                    let property = node.parent();
                    let pattern = property.and_then(|p| p.child_by_field_name("name")).or_else(|| property.and_then(|p| (0..p.child_count()).filter_map(|i| p.child(i as u32)).find(|c| c.kind() == "pattern")));
                    return pattern.map_or("var".into(), |p| self.text_of(p).to_string());
                }
                _ => {}
            }
        }
        node.child_by_field_name("name").map_or_else(|| "?".into(), |n| self.text_of(n).to_string())
    }

    fn owner(&self, node: Node) -> Option<String> {
        let mut up = node.parent();
        while let Some(parent) = up {
            match (self.lang, parent.kind()) {
                (Lang::Rust, "impl_item") => return parent.child_by_field_name("type").map(|t| self.text_of(t).to_string()),
                (Lang::Swift, "class_declaration" | "protocol_declaration") => {
                    return (0..parent.child_count()).filter_map(|i| parent.child(i as u32)).find(|c| c.kind() == "type_identifier" || c.kind() == "user_type").map(|t| self.text_of(t).to_string())
                }
                _ => {}
            }
            if is_function(self.lang, parent) {
                return Some(self.name(parent));
            }
            up = parent.parent();
        }
        None
    }

    fn exempt(&self, node: Node) -> bool {
        match self.lang {
            Lang::Rust => {
                let in_trait = node.parent().and_then(|p| p.parent()).is_some_and(|p| (p.kind() == "impl_item" && p.child_by_field_name("trait").is_some()) || p.kind() == "trait_item");
                let mut attributes = String::new();
                let mut previous = node.prev_sibling();
                while let Some(attribute) = previous.filter(|p| p.kind() == "attribute_item" || is_comment(p.kind())) {
                    attributes.push_str(self.text_of(attribute));
                    previous = attribute.prev_sibling();
                }
                let head = self.text_of(node).split('{').next().unwrap_or("");
                in_trait || attributes.contains("no_mangle") || head.contains("extern") || self.name(node) == "main"
            }
            Lang::Swift => {
                let head = self.text_of(node).split('{').next().unwrap_or("");
                let private = head.contains("private ") || head.contains("fileprivate ");
                !private || head.contains("override ") || head.contains("@objc") || head.contains("@IBAction") || node.kind() != "function_declaration"
            }
        }
    }

    fn function(&self, node: Node) -> Function {
        let mut measure = Measure { lang: self.lang, text: self.text, cyclomatic: 1, cognitive: 0, deepest: 0, calls: BTreeSet::new(), name: self.name(node) };
        let body = node.child_by_field_name("body").or_else(|| (0..node.child_count()).filter_map(|i| node.child(i as u32)).find(|c| c.kind() == "function_body" || c.kind() == "block"));
        let scope = body.unwrap_or(node);
        let mut cursor = scope.walk();
        for child in scope.children(&mut cursor) {
            measure.walk(child, 0);
        }
        let mut lines = BTreeSet::new();
        collect_lines(node, &mut lines);
        let owner = self.owner(node);
        let name = match owner {
            Some(owner) => format!("{owner}::{}", measure.name),
            None => measure.name.clone(),
        };
        Function {
            name,
            line: node.start_position().row + 1,
            end: node.end_position().row + 1,
            lines: lines.len(),
            cyclomatic: measure.cyclomatic,
            cognitive: measure.cognitive,
            nesting: measure.deepest,
            wrapper: body.is_some_and(|b| self.wrapper(node, b)),
            calls: measure.calls,
            exempt: self.exempt(node),
        }
    }

    fn wrapper(&self, function: Node, body: Node) -> bool {
        let named: Vec<Node> = (0..body.named_child_count()).filter_map(|i| body.named_child(i as u32)).filter(|c| !is_comment(c.kind())).collect();
        let statements: Vec<Node> = match (self.lang, named.as_slice()) {
            (Lang::Swift, [only]) if only.kind() == "statements" => (0..only.named_child_count()).filter_map(|i| only.named_child(i as u32)).filter(|c| !is_comment(c.kind())).collect(),
            _ => named,
        };
        let [only] = statements.as_slice() else { return false };
        let mut call = *only;
        loop {
            match call.kind() {
                "expression_statement" | "return_expression" | "control_transfer_statement" | "try_expression" | "await_expression" => {
                    let Some(inner) = (0..call.named_child_count()).filter_map(|i| call.named_child(i as u32)).find(|c| !is_comment(c.kind())) else { return false };
                    call = inner;
                }
                _ => break,
            }
        }
        if call.kind() != "call_expression" {
            return false;
        }
        let parameters = parameter_names(self.lang, self.text, function);
        let arguments = argument_texts(self.lang, self.text, call);
        if arguments.is_empty() || parameters.is_empty() {
            return false;
        }
        arguments.iter().all(|a| parameters.contains(a) || a == "self") && parameters.iter().all(|p| arguments.contains(p))
    }
}

fn parameter_names(lang: Lang, text: &str, function: Node) -> Vec<String> {
    let mut names = Vec::new();
    let mut stack = vec![function];
    while let Some(node) = stack.pop() {
        if matches!(node.kind(), "block" | "function_body" | "computed_property") && node != function {
            continue;
        }
        match (lang, node.kind()) {
            (Lang::Rust, "parameter") => {
                if let Some(pattern) = node.child_by_field_name("pattern") {
                    names.push(text[pattern.byte_range()].trim_start_matches("mut ").to_string());
                }
                continue;
            }
            (Lang::Swift, "parameter") => {
                let identifiers: Vec<Node> = (0..node.named_child_count()).filter_map(|i| node.named_child(i as u32)).filter(|c| c.kind() == "simple_identifier").collect();
                if let Some(last) = identifiers.last() {
                    names.push(text[last.byte_range()].to_string());
                }
                continue;
            }
            _ => {}
        }
        for i in (0..node.named_child_count()).rev() {
            if let Some(child) = node.named_child(i as u32) {
                stack.push(child);
            }
        }
    }
    names
}

fn argument_texts(lang: Lang, text: &str, call: Node) -> Vec<String> {
    let arguments = match lang {
        Lang::Rust => call.child_by_field_name("arguments"),
        Lang::Swift => (0..call.named_child_count()).filter_map(|i| call.named_child(i as u32)).find(|c| c.kind() == "call_suffix").and_then(|s| (0..s.named_child_count()).filter_map(|i| s.named_child(i as u32)).find(|c| c.kind() == "value_arguments")),
    };
    let Some(arguments) = arguments else { return vec![] };
    (0..arguments.named_child_count())
        .filter_map(|i| arguments.named_child(i as u32))
        .filter(|c| !is_comment(c.kind()))
        .map(|argument| {
            let value = if lang == Lang::Swift { argument.child_by_field_name("value").unwrap_or(argument) } else { argument };
            let mut shown = text[value.byte_range()].to_string();
            if lang == Lang::Swift {
                if let Some((_, after)) = shown.split_once(':') {
                    shown = after.trim().to_string();
                }
            }
            shown.trim_start_matches('&').trim_start_matches("mut ").to_string()
        })
        .collect()
}

fn collect_lines(node: Node, lines: &mut BTreeSet<usize>) {
    if is_comment(node.kind()) {
        return;
    }
    if node.child_count() == 0 {
        lines.extend(node.start_position().row + 1..=node.end_position().row + 1);
        return;
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_lines(child, lines);
    }
}

struct Measure<'a> {
    lang: Lang,
    text: &'a str,
    cyclomatic: usize,
    cognitive: usize,
    deepest: usize,
    calls: BTreeSet<String>,
    name: String,
}

impl<'a> Measure<'a> {
    fn structure(&mut self, nesting: usize) {
        self.cognitive += 1 + nesting;
        self.deepest = self.deepest.max(nesting + 1);
    }

    fn walk(&mut self, node: Node, nesting: usize) {
        let kind = node.kind();
        if is_function(self.lang, node) || matches!(kind, "impl_item" | "trait_item" | "mod_item" | "class_declaration" | "struct_item" | "enum_item") {
            return;
        }
        if is_comment(kind) {
            return;
        }
        let mut inner = nesting;
        match (self.lang, kind) {
            (Lang::Rust, "if_expression") | (Lang::Swift, "if_statement") => {
                let chained = node.parent().is_some_and(|p| p.kind() == "else_clause" || (self.lang == Lang::Swift && p.kind() == "if_statement" && p.child_by_field_name("else").is_some_and(|e| e == node) || is_else_if_swift(node)));
                if chained {
                    self.cognitive += 1;
                    self.deepest = self.deepest.max(nesting + 1);
                } else {
                    self.structure(nesting);
                }
                inner = nesting + 1;
                self.cyclomatic += 1;
                self.else_branch(node);
            }
            (Lang::Rust, "match_expression") | (Lang::Swift, "switch_statement") => {
                self.structure(nesting);
                inner = nesting + 1;
            }
            (Lang::Rust, "match_arm") | (Lang::Swift, "switch_entry") => {
                let is_default = self.text[node.byte_range()].trim_start().starts_with("_ =>") || self.text[node.byte_range()].trim_start().starts_with("default");
                if !is_default {
                    self.cyclomatic += 1;
                }
                inner = nesting;
            }
            (Lang::Rust, "for_expression" | "while_expression" | "loop_expression") | (Lang::Swift, "for_statement" | "while_statement" | "repeat_while_statement") => {
                self.structure(nesting);
                inner = nesting + 1;
                if kind != "loop_expression" {
                    self.cyclomatic += 1;
                }
            }
            (Lang::Swift, "catch_block") => {
                self.structure(nesting);
                inner = nesting + 1;
                self.cyclomatic += 1;
            }
            (Lang::Swift, "guard_statement") => {
                self.cognitive += 1;
                self.cyclomatic += 1;
            }
            (Lang::Rust, "let_declaration") if node.child_by_field_name("alternative").is_some() || (0..node.child_count()).filter_map(|i| node.child(i as u32)).any(|c| c.kind() == "else") => {
                self.cognitive += 1;
                self.cyclomatic += 1;
            }
            (Lang::Swift, "ternary_expression") => {
                self.structure(nesting);
                self.cyclomatic += 1;
                inner = nesting + 1;
            }
            (Lang::Rust, "break_expression" | "continue_expression") | (Lang::Swift, "control_transfer_statement") => {
                if (0..node.child_count()).filter_map(|i| node.child(i as u32)).any(|c| c.kind() == "label" || (self.lang == Lang::Swift && c.kind() == "simple_identifier")) {
                    self.cognitive += 1;
                }
            }
            (Lang::Rust, "binary_expression") | (Lang::Swift, "conjunction_expression" | "disjunction_expression") => {
                if self.is_logical(node) && !node.parent().is_some_and(|p| self.is_logical(p)) {
                    let mut operators = Vec::new();
                    self.logical_operators(node, &mut operators);
                    self.cyclomatic += operators.len();
                    self.cognitive += operators.windows(2).filter(|w| w[0] != w[1]).count() + usize::from(!operators.is_empty());
                }
            }
            (Lang::Rust, "call_expression") => {
                if let Some(function) = node.child_by_field_name("function") {
                    self.call(function);
                }
            }
            (Lang::Swift, "call_expression") => {
                if let Some(function) = node.named_child(0) {
                    self.call(function);
                }
            }
            _ if is_closure(self.lang, kind) => inner = nesting + 1,
            _ => {}
        }
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if child.kind() == "else_clause" {
                let mut inside = child.walk();
                for part in child.children(&mut inside) {
                    self.walk(part, if part.kind() == "if_expression" { nesting } else { inner });
                }
                continue;
            }
            if self.lang == Lang::Swift && child.kind() == "if_statement" && is_else_if_swift(child) {
                self.walk(child, nesting);
                continue;
            }
            self.walk(child, inner);
        }
    }

    fn else_branch(&mut self, node: Node) {
        match self.lang {
            Lang::Rust => {
                if let Some(alternative) = node.child_by_field_name("alternative") {
                    if !(0..alternative.named_child_count()).filter_map(|i| alternative.named_child(i as u32)).any(|c| c.kind() == "if_expression") {
                        self.cognitive += 1;
                    }
                }
            }
            Lang::Swift => {
                let children: Vec<Node> = (0..node.child_count()).filter_map(|i| node.child(i as u32)).collect();
                if let Some(at) = children.iter().position(|c| c.kind() == "else") {
                    if children.get(at + 1).is_some_and(|c| c.kind() != "if_statement") {
                        self.cognitive += 1;
                    }
                }
            }
        }
    }

    fn is_logical(&self, node: Node) -> bool {
        match self.lang {
            Lang::Rust => node.kind() == "binary_expression" && node.child_by_field_name("operator").is_some_and(|o| matches!(&self.text[o.byte_range()], "&&" | "||")),
            Lang::Swift => matches!(node.kind(), "conjunction_expression" | "disjunction_expression"),
        }
    }

    fn logical_operators(&self, node: Node, operators: &mut Vec<String>) {
        if is_closure(self.lang, node.kind()) || node.kind() == "call_expression" {
            return;
        }
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            let text = &self.text[child.byte_range()];
            if child.child_count() == 0 && (text == "&&" || text == "||") {
                operators.push(text.to_string());
            } else if child.child_count() > 0 {
                self.logical_operators(child, operators);
            }
        }
    }

    fn call(&mut self, function: Node) {
        let callee = match function.kind() {
            "field_expression" => function.child_by_field_name("field"),
            "scoped_identifier" => function.child_by_field_name("name"),
            "generic_function" => function.child_by_field_name("function").and_then(|f| f.child_by_field_name("field").or(f.child_by_field_name("name")).or(Some(f))),
            "navigation_expression" => function.child_by_field_name("suffix").and_then(|s| s.child_by_field_name("suffix").or(s.named_child(0))),
            _ => Some(function),
        };
        if let Some(callee) = callee {
            let name = &self.text[callee.byte_range()];
            if !name.is_empty() && name.chars().all(|c| c.is_alphanumeric() || c == '_') {
                if name == self.name {
                    self.cognitive += 1;
                }
                self.calls.insert(name.to_string());
            }
        }
    }
}

fn is_else_if_swift(node: Node) -> bool {
    let Some(parent) = node.parent() else { return false };
    if parent.kind() != "if_statement" {
        return false;
    }
    let children: Vec<Node> = (0..parent.child_count()).filter_map(|i| parent.child(i as u32)).collect();
    children.iter().position(|c| *c == node).is_some_and(|at| at > 0 && children[at - 1].kind() == "else")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn only(lang: Lang, text: &str) -> Function {
        let facts = analyze(lang, text);
        assert_eq!(facts.functions.len(), 1, "{:?}", facts.functions);
        facts.functions.into_iter().next().unwrap()
    }

    #[test]
    fn rust_cognitive_complexity_follows_the_white_paper() {
        let f = only(
            Lang::Rust,
            "fn sum_of_primes(max: u32) -> u32 {
    let mut total = 0;
    'outer: for i in 1..=max {
        for j in 2..i {
            if i % j == 0 {
                continue 'outer;
            }
        }
        total += i;
    }
    total
}",
        );
        assert_eq!(f.cognitive, 7);
        assert_eq!(f.cyclomatic, 4);
        assert_eq!(f.nesting, 3);
        let g = only(
            Lang::Rust,
            "fn words(n: u32) -> &'static str {
    match n {
        1 => \"one\",
        2 => \"a couple\",
        _ => \"lots\",
    }
}",
        );
        assert_eq!(g.cognitive, 1);
        assert_eq!(g.cyclomatic, 3);
    }

    #[test]
    fn else_if_and_else_count_flat_and_logical_sequences_count_once_each() {
        let f = only(
            Lang::Rust,
            "fn f(a: bool, b: bool, c: bool) -> u8 {
    if a && b && c {
        1
    } else if a || b && c {
        2
    } else {
        let g = |x: u8| if x > 1 { 1 } else { 0 };
        g(3)
    }
}",
        );
        assert_eq!(f.cognitive, 1 + 1 + 1 + 2 + 1 + 3 + 1);
        assert_eq!(f.cyclomatic, 1 + 1 + 2 + 1 + 2 + 1);
        assert_eq!(f.calls, BTreeSet::from(["g".to_string()]));
    }

    #[test]
    fn swift_functions_closures_guards_and_computed_properties() {
        let facts = analyze(
            Lang::Swift,
            "final class A: NSView {
    private func f(_ a: Int?, b: Bool) -> Int {
        guard let a else { return 0 }
        if a > 1 && b { return 1 } else if a < 0 { return 2 } else { return 3 }
        switch a {
        case 1, 2: return 1
        default: break
        }
        items.forEach { x in if x { print(x) } }
        return b ? 1 : 2
    }
    var y: Int { compute(1) }
    init() { super.init() }
}",
        );
        let names: Vec<&str> = facts.functions.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, ["A::f", "A::y", "A::init"]);
        let f = &facts.functions[0];
        assert_eq!(f.cognitive, 1 + 1 + 1 + 1 + 1 + 1 + 2 + 1);
        assert_eq!(f.calls, BTreeSet::from(["forEach".to_string(), "print".to_string()]));
        assert!(!f.exempt);
        assert!(facts.functions[1].exempt);
    }

    #[test]
    fn wrappers_pass_their_parameters_straight_on() {
        let swift = analyze(
            Lang::Swift,
            "class C {
    func layoutCommand(_ cmd: String, _ args: [String: JSON], reply: ((JSON) -> Void)? = nil) {
        request(cmd, args, reply: reply)
    }
    func other(_ a: Int) { request(a + 1) }
}",
        );
        assert!(swift.functions[0].wrapper);
        assert!(!swift.functions[1].wrapper);
        let rust = analyze(Lang::Rust, "fn a(x: u8, y: u8) -> u8 { b(x, y) }\nfn c(x: u8) -> u8 { b(x, 1) }\nfn d(&self, x: u8) -> u8 { self.e(x) }");
        assert_eq!(rust.functions.iter().map(|f| f.wrapper).collect::<Vec<_>>(), [true, false, true]);
    }

    #[test]
    fn test_code_is_counted_apart_and_blocks_hash_alike_when_renamed() {
        let facts = analyze(Lang::Rust, "fn a(x: u8) -> u8 {\n    let y = x + 1;\n    y * 2\n}\n\n// note\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn t() { assert!(true); }\n}\n");
        assert_eq!(facts.lines, 5);
        assert_eq!(facts.test_lines, 4);
        assert_eq!(facts.functions.len(), 1);
        let renamed = analyze(Lang::Rust, "fn b(v: u8) -> u8 {\n    let w = v + 7;\n    w * 3\n}\n");
        let swapped = analyze(Lang::Rust, "fn b(v: u8) -> u8 {\n    let w = v + 7;\n    v * 3\n}\n");
        assert_eq!(facts.blocks.len(), 1);
        assert_eq!(facts.blocks[0].lines, vec![1, 2, 3, 4]);
        assert_eq!(facts.blocks[0].hash, renamed.blocks[0].hash, "identifiers renamed in order, literals by type");
        assert_ne!(facts.blocks[0].hash, swapped.blocks[0].hash, "a different variable is a different block");
        assert_eq!(facts.names.get("x"), Some(&2));
    }
}
