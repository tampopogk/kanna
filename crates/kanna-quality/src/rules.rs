use crate::code::Lang;
use tree_sitter::Node;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hit {
    pub rule: &'static str,
    pub line: usize,
    pub end: usize,
}

pub const RULES: &[(&str, &str)] = &[
    ("unnecessary-else", "unnecessary-elif, unnecessary-else-raise"),
    ("collapsible-if", "nested-if-no-else"),
    ("needless-bool", "boolean-return-if-else, redundant-bool-ternary"),
    ("same-branches", "ternary-same-value"),
    ("same-condition", "duplicated-if-condition"),
    ("needless-continue", "redundant-continue"),
    ("needless-return", "redundant-return-none"),
    ("redundant-closure", "unnecessary-lambda"),
    ("identity-map", "redundant-list-comprehension"),
    ("len-zero", "len-comparison, len-as-condition"),
    ("bool-comparison", "bool-comparison"),
    ("loop-push", "verbose-list-append-loop, verbose-dict-update, set-add-loop, list-extend-from-loop"),
    ("loop-accumulate", "manual-sum-loop, manual-count-loop, manual-str-join"),
    ("consecutive-push", "consecutive-append-calls"),
    ("repeated-key-assignment", "repeated-dict-key-assignment"),
    ("nested-guard-invert", "nested-guard-invert"),
    ("repeated-if-continue", "repeated-if-continue"),
    ("manual-find", "manual-find-return-none"),
    ("manual-min-max", "manual-min-max"),
    ("manual-default", "verbose-none-default"),
    ("manual-entry", "manual-dict-setdefault"),
    ("check-then-unwrap", "verbose-dict-key-access, get-then-none-check"),
    ("none-guard", "guard-return-none, defensive-none-return-heavy"),
    ("error-rethrow", "except-bare-reraise"),
    ("error-swallowed", "except-pass-silence"),
    ("error-to-sentinel", "except-return-static-sentinel"),
    ("error-rewrap", "wrap-raise-fstring"),
    ("error-soup", "defensive-try-soup-function"),
    ("formatted-error-heavy", "defensive-fstring-raise-heavy"),
    ("string-dispatch", "dispatch-by-string-equality"),
    ("needless-collect", "unnecessary-list-call, verbose-sorted-list"),
    ("long-tuple", "long-tuple-unpacking"),
    ("index-unpack", "manual-index-unpack"),
    ("banner-comment", "section-banner-comment"),
    ("empty-init", "empty-init"),
];

pub fn check(lang: Lang, root: Node, text: &str) -> Vec<Hit> {
    let mut rules = Rules { lang, text, hits: Vec::new() };
    if lang != Lang::Python {
        rules.visit(root);
    }
    rules.hits.sort_by(|a, b| (a.line, a.end, a.rule).cmp(&(b.line, b.end, b.rule)));
    rules.hits.dedup();
    rules.hits
}

fn named<'t>(node: Node<'t>) -> Vec<Node<'t>> {
    (0..node.named_child_count()).filter_map(|i| node.named_child(i as u32)).filter(|c| !matches!(c.kind(), "line_comment" | "block_comment" | "comment" | "multiline_comment")).collect()
}

fn all<'t>(node: Node<'t>) -> Vec<Node<'t>> {
    (0..node.child_count()).filter_map(|i| node.child(i as u32)).collect()
}

fn squash(text: &str) -> String {
    text.chars().filter(|c| !c.is_whitespace()).collect()
}

struct If<'t> {
    condition: String,
    then: Vec<Node<'t>>,
    otherwise: Option<Else<'t>>,
}

enum Else<'t> {
    If(Node<'t>),
    Block(Vec<Node<'t>>),
}

struct Rules<'a> {
    lang: Lang,
    text: &'a str,
    hits: Vec<Hit>,
}

impl<'a> Rules<'a> {
    fn text_of(&self, node: Node) -> &'a str {
        &self.text[node.byte_range()]
    }

    fn hit(&mut self, rule: &'static str, node: Node) {
        self.hits.push(Hit { rule, line: node.start_position().row + 1, end: node.end_position().row + 1 });
    }

    fn hit_span(&mut self, rule: &'static str, first: Node, last: Node) {
        self.hits.push(Hit { rule, line: first.start_position().row + 1, end: last.end_position().row + 1 });
    }

    fn inner<'t>(&self, statement: Node<'t>) -> Node<'t> {
        if statement.kind() == "expression_statement" {
            named(statement).into_iter().next().unwrap_or(statement)
        } else {
            statement
        }
    }

    fn swift_blocks<'t>(&self, node: Node<'t>) -> Vec<Vec<Node<'t>>> {
        let children = all(node);
        let mut blocks = Vec::new();
        let mut at = 0;
        while at < children.len() {
            if children[at].kind() == "{" {
                let content = children.get(at + 1).filter(|c| c.kind() == "statements").map(|c| named(*c)).unwrap_or_default();
                blocks.push(content);
            }
            at += 1;
        }
        blocks
    }

    fn block<'t>(&self, node: Node<'t>) -> Vec<Node<'t>> {
        match self.lang {
            Lang::Rust => named(node),
            _ => self.swift_blocks(node).into_iter().next().unwrap_or_default(),
        }
    }

    fn as_if<'t>(&self, node: Node<'t>) -> Option<If<'t>> {
        match (self.lang, node.kind()) {
            (Lang::Rust, "if_expression") => {
                let condition = node.child_by_field_name("condition")?;
                let then = named(node.child_by_field_name("consequence")?);
                let otherwise = node.child_by_field_name("alternative").and_then(|a| named(a).into_iter().next()).map(|a| if a.kind() == "if_expression" { Else::If(a) } else { Else::Block(named(a)) });
                Some(If { condition: self.text_of(condition).to_string(), then, otherwise })
            }
            (Lang::Swift, "if_statement") => {
                let children = all(node);
                let open = children.iter().position(|c| c.kind() == "{")?;
                let condition = self.text[children.first()?.end_byte()..children[open].start_byte()].trim().to_string();
                let then = children.get(open + 1).filter(|c| c.kind() == "statements").map(|c| named(*c)).unwrap_or_default();
                let otherwise = children.iter().position(|c| c.kind() == "else").and_then(|at| children.get(at + 1)).map(|next| {
                    if next.kind() == "if_statement" {
                        Else::If(*next)
                    } else {
                        Else::Block(self.swift_blocks(node).into_iter().nth(1).unwrap_or_default())
                    }
                });
                Some(If { condition, then, otherwise })
            }
            _ => None,
        }
    }

    fn is_jump(&self, statement: Node) -> bool {
        let node = self.inner(statement);
        match self.lang {
            Lang::Rust => matches!(node.kind(), "return_expression" | "break_expression" | "continue_expression"),
            _ => node.kind() == "control_transfer_statement",
        }
    }

    fn is_return(&self, statement: Node) -> bool {
        let node = self.inner(statement);
        match self.lang {
            Lang::Rust => node.kind() == "return_expression",
            _ => node.kind() == "control_transfer_statement" && self.text_of(node).starts_with("return"),
        }
    }

    fn is_throw(&self, statement: Node) -> bool {
        let node = self.inner(statement);
        match self.lang {
            Lang::Rust => (node.kind() == "return_expression" && self.text_of(node).trim_start_matches("return").trim_start().starts_with("Err(")) || (node.kind() == "macro_invocation" && matches!(self.macro_name(node), "panic" | "bail")),
            _ => node.kind() == "control_transfer_statement" && self.text_of(node).starts_with("throw"),
        }
    }

    fn macro_name(&self, node: Node) -> &'a str {
        node.child_by_field_name("macro").map_or("", |m| self.text_of(m))
    }

    fn loop_body<'t>(&self, node: Node<'t>) -> Option<Vec<Node<'t>>> {
        match (self.lang, node.kind()) {
            (Lang::Rust, "for_expression" | "while_expression" | "loop_expression") => node.child_by_field_name("body").map(named),
            (Lang::Swift, "for_statement" | "while_statement" | "repeat_while_statement") => Some(self.block(node)),
            _ => None,
        }
    }

    fn function_body<'t>(&self, node: Node<'t>) -> Option<Vec<Node<'t>>> {
        match (self.lang, node.kind()) {
            (Lang::Rust, "function_item") => node.child_by_field_name("body").map(named),
            (Lang::Swift, "function_declaration" | "init_declaration") => {
                let body = node.child_by_field_name("body").or_else(|| all(node).into_iter().find(|c| c.kind() == "function_body"))?;
                Some(self.block(body))
            }
            _ => None,
        }
    }

    fn is_function(&self, node: Node) -> bool {
        matches!((self.lang, node.kind()), (Lang::Rust, "function_item") | (Lang::Swift, "function_declaration" | "init_declaration"))
    }

    fn method_call(&self, statement: Node) -> Option<(String, &'a str)> {
        let node = self.inner(statement);
        if node.kind() != "call_expression" {
            return None;
        }
        match self.lang {
            Lang::Rust => {
                let function = node.child_by_field_name("function").filter(|f| f.kind() == "field_expression")?;
                Some((squash(self.text_of(function.child_by_field_name("value")?)), self.text_of(function.child_by_field_name("field")?)))
            }
            _ => {
                let navigation = named(node).into_iter().next().filter(|n| n.kind() == "navigation_expression")?;
                let suffix = navigation.child_by_field_name("suffix")?.child_by_field_name("suffix")?;
                Some((squash(self.text_of(navigation.child_by_field_name("target")?)), self.text_of(suffix)))
            }
        }
    }

    fn adds(&self, statement: Node) -> Option<String> {
        if let Some((receiver, method)) = self.method_call(statement) {
            let adding = match self.lang {
                Lang::Rust => matches!(method, "push" | "push_str" | "insert" | "push_back" | "extend"),
                _ => matches!(method, "append" | "insert" | "formUnion"),
            };
            return adding.then_some(receiver);
        }
        let node = self.inner(statement);
        if self.lang == Lang::Swift && node.kind() == "assignment" && node.child_by_field_name("operator").is_some_and(|o| self.text_of(o) == "=") {
            let target = squash(self.text_of(node.child_by_field_name("target")?));
            if let Some(open) = target.find('[') {
                return Some(target[..open].to_string());
            }
        }
        None
    }

    fn pushes(&self, statement: Node) -> bool {
        self.method_call(statement).is_some_and(|(_, method)| match self.lang {
            Lang::Rust => matches!(method, "push" | "push_str" | "push_back"),
            _ => method == "append",
        })
    }

    fn keyed_call(&self, statement: Node) -> Option<(String, String)> {
        let node = self.inner(statement);
        if self.lang != Lang::Swift || node.kind() != "assignment" || node.child_by_field_name("operator").is_none_or(|o| self.text_of(o) != "=") {
            return None;
        }
        let target = squash(self.text_of(node.child_by_field_name("target")?));
        let base = target.strip_suffix(']')?.split_once('[')?.0.to_string();
        let value = node.child_by_field_name("result").filter(|v| v.kind() == "call_expression")?;
        let callee = named(value).into_iter().next()?;
        all(value).iter().find(|c| c.kind() == "call_suffix").filter(|s| all(**s).iter().all(|c| c.kind() != "lambda_literal"))?;
        Some((base, squash(self.text_of(callee))))
    }

    fn accumulates(&self, statement: Node) -> bool {
        let node = self.inner(statement);
        match self.lang {
            Lang::Rust => node.kind() == "compound_assignment_expr" && node.child_by_field_name("operator").is_some_and(|o| matches!(self.text_of(o), "+=" | "-=" | "*=")),
            _ => node.kind() == "assignment" && node.child_by_field_name("operator").is_some_and(|o| matches!(self.text_of(o), "+=" | "-=" | "*=")),
        }
    }

    fn visit(&mut self, node: Node) {
        self.node(node);
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            self.visit(child);
        }
    }

    fn node(&mut self, node: Node) {
        if matches!(node.kind(), "line_comment" | "comment") {
            let text = self.text_of(node);
            let rest = text.trim_start_matches('/').trim();
            if rest.chars().count() >= 10 && rest.chars().all(|c| matches!(c, '-' | '=' | '─' | '*')) {
                self.hit("banner-comment", node);
            }
            return;
        }
        if let Some(statement) = self.as_if(node) {
            self.if_rules(node, &statement);
        }
        if let Some(body) = self.loop_body(node) {
            self.loop_rules(node, &body);
        }
        if self.is_function(node) {
            if let Some(body) = self.function_body(node) {
                self.function_rules(node, &body);
            }
        }
        self.sequence_rules(node);
        match (self.lang, node.kind()) {
            (Lang::Rust, "closure_expression") | (Lang::Swift, "lambda_literal") => self.closure_rules(node),
            (Lang::Rust, "binary_expression") | (Lang::Swift, "equality_expression" | "comparison_expression") => self.comparison_rules(node),
            (Lang::Rust, "match_arm") => self.arm_rules(node),
            (Lang::Swift, "catch_block") => self.catch_rules(node),
            (Lang::Swift, "ternary_expression") => self.ternary_rules(node),
            (Lang::Rust, "let_declaration") => self.let_rules(node),
            (Lang::Swift, "property_declaration") => self.let_rules(node),
            (Lang::Rust, "call_expression") => self.call_rules(node),
            _ => {}
        }
    }

    fn chained(&self, node: Node) -> bool {
        node.parent().is_some_and(|p| p.kind() == "else_clause" || (p.kind() == "if_statement" && all(p).iter().position(|c| *c == node).is_some_and(|at| at > 0 && all(p)[at - 1].kind() == "else")))
    }

    fn chain<'t>(&self, node: Node<'t>) -> Vec<If<'t>> {
        let mut out = Vec::new();
        let mut current = Some(node);
        while let Some(at) = current.and_then(|n| self.as_if(n)) {
            current = match &at.otherwise {
                Some(Else::If(next)) => Some(*next),
                _ => None,
            };
            out.push(at);
        }
        out
    }

    fn if_rules(&mut self, node: Node, statement: &If) {
        let chained = self.chained(node);
        if !chained {
            let chain = self.chain(node);
            let has_else_if = chain.len() > 1;
            let first_exits = statement.then.iter().any(|s| self.is_return(*s) || self.is_throw(*s));
            let first_throws = statement.then.iter().any(|s| self.is_throw(*s));
            if (has_else_if && first_exits) || (statement.otherwise.is_some() && first_throws) {
                self.hit("unnecessary-else", node);
            }
            let mut seen = std::collections::HashSet::new();
            if chain.iter().any(|c| !seen.insert(squash(&c.condition))) {
                self.hit("same-condition", node);
            }
            let compared: Vec<Option<String>> = chain.iter().map(|c| self.string_equality(&c.condition)).collect();
            if chain.len() >= 3 && compared.iter().all(|c| c.is_some()) && compared.windows(2).all(|w| w[0] == w[1]) {
                self.hit("string-dispatch", node);
            }
        }
        let is_let = |c: &str| c.starts_with("let ") || c.starts_with("case ") || c.starts_with("var ");
        if statement.otherwise.is_none() && !(self.lang == Lang::Rust && is_let(&statement.condition)) {
            if let [only] = statement.then.as_slice() {
                if let Some(inner) = self.as_if(self.inner(*only)) {
                    if inner.otherwise.is_none() && !(self.lang == Lang::Rust && is_let(&inner.condition)) {
                        self.hit("collapsible-if", node);
                    }
                }
            }
        }
        if let Some(Else::Block(otherwise)) = &statement.otherwise {
            if !chained {
                let value = |items: &[Node]| -> Option<String> {
                    match items {
                        [only] => {
                            let inner = self.inner(*only);
                            let text = self.text_of(inner).trim_end_matches(';').trim();
                            Some(text.trim_start_matches("return").trim().to_string())
                        }
                        _ => None,
                    }
                };
                match (value(&statement.then), value(otherwise)) {
                    (Some(a), Some(b)) if (a == "true" && b == "false") || (a == "false" && b == "true") => self.hit("needless-bool", node),
                    (Some(a), Some(b)) if self.lang == Lang::Rust && self.min_max(&statement.condition, &a, &b) => self.hit("manual-min-max", node),
                    _ => {}
                }
                let then: String = statement.then.iter().map(|s| squash(self.text_of(*s))).collect();
                let other: String = otherwise.iter().map(|s| squash(self.text_of(*s))).collect();
                if !then.is_empty() && then == other {
                    self.hit("same-branches", node);
                }
            }
        }
        if statement.otherwise.is_none() && statement.then.len() >= 2 {
            let guard = |rules: &Self, s: Node| rules.as_if(rules.inner(s)).is_some_and(|inner| inner.otherwise.is_none() && inner.then.iter().any(|t| rules.is_jump(*t)));
            let statement_node = node.parent().filter(|p| p.kind() == "expression_statement").unwrap_or(node);
            let mut next = statement_node.next_named_sibling();
            while next.is_some_and(|n| matches!(n.kind(), "line_comment" | "block_comment" | "comment" | "multiline_comment")) {
                next = next.and_then(|n| n.next_named_sibling());
            }
            let tail = next.is_none() && !self.chained(node);
            let plain = statement.then.iter().any(|s| self.as_if(self.inner(*s)).is_none());
            if tail && plain && statement.then.iter().any(|s| guard(self, *s)) {
                self.hit("nested-guard-invert", node);
            }
        }
        if statement.otherwise.is_none() {
            let condition = squash(&statement.condition);
            let assigns = |rules: &Self, target: &str| statement.then.len() == 1 && {
                let inner = rules.inner(statement.then[0]);
                let left = inner.child_by_field_name("left").or(inner.child_by_field_name("target"));
                matches!(inner.kind(), "assignment_expression" | "assignment") && inner.child_by_field_name("operator").is_none_or(|o| rules.text_of(o) == "=") && left.is_some_and(|l| squash(rules.text_of(l)) == target)
            };
            match self.lang {
                Lang::Rust => {
                    if let Some(target) = condition.strip_suffix(".is_none()") {
                        if assigns(self, target) {
                            self.hit("manual-default", node);
                        }
                    }
                    if let Some(rest) = condition.strip_prefix('!').and_then(|c| c.split_once(".contains_key(")) {
                        if statement.then.len() == 1 && self.method_call(statement.then[0]).is_some_and(|(r, m)| r == rest.0 && m == "insert") {
                            self.hit("manual-entry", node);
                        }
                    }
                }
                _ => {
                    if let Some(target) = condition.strip_suffix("==nil") {
                        if assigns(self, target) {
                            self.hit(if target.ends_with(']') { "manual-entry" } else { "manual-default" }, node);
                        }
                    }
                }
            }
        }
        let condition = squash(&statement.condition);
        let body: String = statement.then.iter().map(|s| squash(self.text_of(*s))).collect();
        let unwrapped = match self.lang {
            Lang::Rust => {
                if let Some((map, key)) = condition.split_once(".contains_key(") {
                    let key = key.strip_suffix(')').unwrap_or(key).trim_start_matches('&');
                    !map.starts_with('!') && (body.contains(&format!("{map}.get({key}).unwrap()")) || body.contains(&format!("{map}.get(&{key}).unwrap()")) || body.contains(&format!("{map}[{key}]")) || body.contains(&format!("{map}[&{key}]")))
                } else if let Some(value) = condition.strip_suffix(".is_some()").or(condition.strip_suffix(".is_ok()")) {
                    !value.starts_with('!') && (body.contains(&format!("{value}.unwrap()")) || body.contains(&format!("{value}.expect(")))
                } else {
                    false
                }
            }
            _ => condition.strip_suffix("!=nil").is_some_and(|value| body.contains(&format!("{value}!"))),
        };
        if unwrapped {
            self.hit("check-then-unwrap", node);
        }
    }

    fn string_equality(&self, condition: &str) -> Option<String> {
        let (left, right) = condition.split_once("==")?;
        let (left, right) = (left.trim(), right.trim());
        let quoted = |s: &str| s.len() >= 2 && s.starts_with('"') && s.ends_with('"') && !s[1..s.len() - 1].contains('"');
        let ident = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '.' || c == '*' || c == '&');
        if ident(left) && quoted(right) {
            Some(left.trim_start_matches(['*', '&']).to_string())
        } else if ident(right) && quoted(left) {
            Some(right.trim_start_matches(['*', '&']).to_string())
        } else {
            None
        }
    }

    fn min_max(&self, condition: &str, a: &str, b: &str) -> bool {
        for op in [">=", "<=", ">", "<"] {
            if let Some((left, right)) = condition.split_once(op) {
                let (left, right) = (squash(left), squash(right));
                if left.contains(['=', '<', '>', '&', '|']) || right.contains(['=', '<', '>', '&', '|']) {
                    return false;
                }
                let (a, b) = (squash(a), squash(b));
                return (a == left && b == right) || (a == right && b == left);
            }
        }
        false
    }

    fn loop_rules(&mut self, node: Node, body: &[Node]) {
        if let Some(last) = body.last() {
            let inner = self.inner(*last);
            let plain = match self.lang {
                Lang::Rust => inner.kind() == "continue_expression" && named(inner).is_empty(),
                _ => inner.kind() == "control_transfer_statement" && self.text_of(inner).trim() == "continue",
            };
            if plain {
                self.hit("needless-continue", *last);
            }
        }
        let is_for = matches!(node.kind(), "for_expression" | "for_statement");
        if is_for {
            if let [only] = body {
                if self.adds(*only).is_some() {
                    self.hit("loop-push", node);
                } else if self.accumulates(*only) {
                    self.hit("loop-accumulate", node);
                }
            }
            let collection = match self.lang {
                Lang::Rust => node.child_by_field_name("value"),
                _ => node.child_by_field_name("collection"),
            };
            if let Some(collection) = collection {
                let text = squash(self.text_of(collection));
                let copied = match self.lang {
                    Lang::Rust => text.ends_with(".to_vec()") || (text.ends_with(".clone()") && !text.contains("..")) || text.contains(".collect::<Vec"),
                    _ => text.starts_with("Array(") && text.ends_with(')'),
                };
                if copied {
                    self.hit("needless-collect", collection);
                }
            }
        }
        let continues = |rules: &Self, s: Node| rules.as_if(rules.inner(s)).is_some_and(|i| i.otherwise.is_none() && i.then.iter().any(|t| {
            let t = rules.inner(*t);
            match rules.lang {
                Lang::Rust => t.kind() == "continue_expression",
                _ => t.kind() == "control_transfer_statement" && rules.text_of(t).trim_start().starts_with("continue"),
            }
        }));
        if body.windows(2).any(|w| continues(self, w[0]) && continues(self, w[1])) {
            self.hit("repeated-if-continue", node);
        }
    }

    fn function_rules(&mut self, node: Node, body: &[Node]) {
        let head = &self.text[node.start_byte()..node.child_by_field_name("body").map_or(node.end_byte(), |b| b.start_byte())];
        if let Some(last) = body.last() {
            let inner = self.inner(*last);
            let needless = match self.lang {
                Lang::Rust => inner.kind() == "return_expression",
                _ => inner.kind() == "control_transfer_statement" && self.text_of(inner).trim() == "return",
            };
            if needless {
                self.hit("needless-return", *last);
            }
        }
        let windows: Vec<(Node, Node)> = body.windows(2).map(|w| (w[0], w[1])).collect();
        for (first, second) in windows {
            let returns_none = |rules: &Self, s: Node| {
                let s = rules.inner(s);
                let text = squash(rules.text_of(s));
                match rules.lang {
                    Lang::Rust => text == "None" || text == "returnNone" || text == "returnNone;",
                    _ => text == "returnnil",
                }
            };
            if let Some(loop_body) = self.loop_body(self.inner(first)).filter(|_| matches!(self.inner(first).kind(), "for_expression" | "for_statement")) {
                if let [only] = loop_body.as_slice() {
                    if let Some(found) = self.as_if(self.inner(*only)) {
                        let returns_value = found.otherwise.is_none() && found.then.len() == 1 && self.is_return(found.then[0]) && !returns_none(self, found.then[0]);
                        if returns_value && returns_none(self, second) && body.last() == Some(&second) {
                            self.hit_span("manual-find", first, second);
                        }
                    }
                }
            }
        }
        if node.kind() == "init_declaration" && body.iter().all(|s| squash(self.text_of(*s)) == "super.init()") && self.text_of(node).contains('{') && !head.contains('(') {
            self.hit("empty-init", node);
        }
        if node.kind() == "init_declaration" && body.is_empty() && head.trim_end().ends_with("init()") {
            self.hit("empty-init", node);
        }
        let mut catches = 0;
        let mut formatted = 0;
        self.count(node, &mut catches, &mut formatted, true);
        if catches >= 3 {
            self.hit("error-soup", node);
        }
        if formatted >= 3 {
            self.hit("formatted-error-heavy", node);
        }
        if self.lang == Lang::Swift {
            let guards = self.none_guards(node, true);
            if guards >= 3 {
                self.hit("none-guard", node);
            }
        }
    }

    fn none_guards(&self, node: Node, top: bool) -> usize {
        if !top && self.is_function(node) {
            return 0;
        }
        let mut count = 0;
        if node.kind() == "guard_statement" {
            let blocks = self.swift_blocks(node);
            if blocks.last().is_some_and(|b| b.len() == 1 && squash(self.text_of(b[0])) == "returnnil") {
                count += 1;
            }
        }
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            count += self.none_guards(child, false);
        }
        count
    }

    fn count(&self, node: Node, catches: &mut usize, formatted: &mut usize, top: bool) {
        if !top && (self.is_function(node) || matches!(node.kind(), "closure_expression" | "lambda_literal")) {
            return;
        }
        match self.lang {
            Lang::Rust => {
                if node.kind() == "match_expression" && node.child_by_field_name("body").is_some_and(|b| named(b).iter().any(|arm| arm.child_by_field_name("pattern").is_some_and(|p| self.text_of(p).trim_start().starts_with("Err(")))) {
                    *catches += 1;
                }
                if node.kind() == "let_condition" && node.child_by_field_name("pattern").is_some_and(|p| self.text_of(p).starts_with("Err(")) {
                    *catches += 1;
                }
                if node.kind() == "let_declaration" && node.child_by_field_name("alternative").is_some() && node.child_by_field_name("pattern").is_some_and(|p| self.text_of(p).starts_with("Ok(")) {
                    *catches += 1;
                }
                let text = self.text_of(node);
                let interpolated = |s: &str| s.contains('{') && s.contains('}');
                if node.kind() == "call_expression" && text.starts_with("Err(format!(") && interpolated(text) {
                    *formatted += 1;
                }
                if node.kind() == "macro_invocation" && matches!(self.macro_name(node), "bail" | "anyhow") && interpolated(text) {
                    *formatted += 1;
                }
            }
            _ => {
                if node.kind() == "do_statement" && all(node).iter().any(|c| c.kind() == "catch_block") {
                    *catches += 1;
                }
                if node.kind() == "control_transfer_statement" && self.text_of(node).starts_with("throw") && self.text_of(node).contains("\\(") {
                    *formatted += 1;
                }
            }
        }
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            self.count(child, catches, formatted, false);
        }
    }

    fn sequence_rules(&mut self, node: Node) {
        let statements = match (self.lang, node.kind()) {
            (Lang::Rust, "block") => named(node),
            (Lang::Swift, "statements") => named(node),
            _ => return,
        };
        for pair in statements.windows(2) {
            if self.pushes(pair[0]) && self.pushes(pair[1]) {
                self.hit("consecutive-push", pair[1]);
            }
            if let (Some(a), Some(b)) = (self.keyed_call(pair[0]), self.keyed_call(pair[1])) {
                if a == b {
                    self.hit("repeated-key-assignment", pair[1]);
                }
            }
            if let (Some((a, i)), Some((b, j))) = (self.indexed_let(pair[0]), self.indexed_let(pair[1])) {
                if a == b && i != j {
                    self.hit_span("index-unpack", pair[0], pair[1]);
                }
            }
        }
    }

    fn indexed_let(&self, statement: Node) -> Option<(String, String)> {
        let value = match (self.lang, statement.kind()) {
            (Lang::Rust, "let_declaration") => statement.child_by_field_name("value")?,
            (Lang::Swift, "property_declaration") => statement.child_by_field_name("value")?,
            _ => return None,
        };
        let text = squash(self.text_of(value));
        let text = text.trim_start_matches('&');
        if let Some((base, index)) = text.rsplit_once('.') {
            if index.chars().all(|c| c.is_ascii_digit()) && !index.is_empty() && !base.is_empty() && !base.ends_with(')') {
                return Some((base.to_string(), index.to_string()));
            }
        }
        let inner = text.strip_suffix(']')?;
        let (base, index) = inner.rsplit_once('[')?;
        (index.chars().all(|c| c.is_ascii_digit()) && !index.is_empty() && !base.is_empty()).then(|| (base.to_string(), index.to_string()))
    }

    fn let_rules(&mut self, node: Node) {
        match self.lang {
            Lang::Rust => {
                if let Some(pattern) = node.child_by_field_name("pattern").filter(|p| p.kind() == "tuple_pattern") {
                    if named(pattern).len() >= 5 {
                        self.hit("long-tuple", node);
                    }
                }
                if let Some(alternative) = node.child_by_field_name("alternative") {
                    let pattern = node.child_by_field_name("pattern").map_or("", |p| self.text_of(p));
                    let exit = named(alternative);
                    if pattern.starts_with("Some(") && exit.len() == 1 && matches!(squash(self.text_of(exit[0])).as_str(), "returnNone" | "returnNone;") {
                        self.hit("none-guard", node);
                    }
                }
            }
            _ => {
                if let Some(pattern) = node.child_by_field_name("name").filter(|p| p.kind() == "pattern") {
                    let elements = named(pattern).iter().filter(|c| c.kind() == "pattern" || c.kind() == "simple_identifier").count();
                    if self.text_of(pattern).starts_with('(') && elements >= 5 {
                        self.hit("long-tuple", node);
                    }
                }
            }
        }
    }

    fn closure_rules(&mut self, node: Node) {
        match self.lang {
            Lang::Rust => {
                let Some(parameters) = node.child_by_field_name("parameters") else { return };
                let params: Vec<String> = named(parameters).iter().map(|p| self.text_of(*p).to_string()).collect();
                if params.len() != 1 || params.iter().any(|p| !p.chars().all(|c| c.is_alphanumeric() || c == '_')) {
                    return;
                }
                let Some(body) = node.child_by_field_name("body") else { return };
                let body = squash(self.text_of(body));
                let parent_method = node.parent().and_then(|a| a.parent()).filter(|c| c.kind() == "call_expression").and_then(|c| self.method_call(c)).map(|(_, m)| m);
                if params.len() == 1 && parent_method == Some("map") && (body == params[0] || body == format!("{}.clone()", params[0]) || body == format!("*{}", params[0])) {
                    self.hit("identity-map", node);
                    return;
                }
                if let Some((callee, args)) = body.strip_suffix(')').and_then(|b| b.split_once('(')) {
                    let path = !callee.is_empty() && callee.chars().all(|c| c.is_alphanumeric() || c == '_' || c == ':') && !callee.ends_with("::") && callee.chars().next().is_some_and(|c| c.is_alphabetic());
                    let macro_like = callee.ends_with('!');
                    if path && !macro_like && args == params.join(",") && !callee.starts_with("Some") && !callee.starts_with("Ok") && !callee.starts_with("Err") && !callee.starts_with("Box::new") {
                        self.hit("redundant-closure", node);
                    }
                }
            }
            _ => {
                let statements: Vec<Node> = all(node).into_iter().filter(|c| c.kind() == "statements").flat_map(named).collect();
                let [only] = statements.as_slice() else { return };
                let body = squash(self.text_of(*only));
                let typed = all(node).iter().any(|c| c.kind() == "lambda_function_type");
                let parent_method = node.parent().and_then(|s| s.parent()).filter(|c| c.kind() == "call_expression").and_then(|c| self.method_call(c)).map(|(_, m)| m);
                if !typed && body == "$0" && matches!(parent_method, Some("map") | Some("compactMap") | Some("flatMap")) {
                    self.hit("identity-map", node);
                    return;
                }
                if !typed {
                    if let Some((callee, args)) = body.strip_suffix(')').and_then(|b| b.split_once('(')) {
                        let simple = !callee.is_empty() && callee.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '.') && callee.chars().next().is_some_and(|c| c.is_alphabetic());
                        if simple && args == "$0" {
                            self.hit("redundant-closure", node);
                        }
                    }
                }
            }
        }
    }

    fn comparison_rules(&mut self, node: Node) {
        let (left, operator, right) = match self.lang {
            Lang::Rust => (node.child_by_field_name("left"), node.child_by_field_name("operator"), node.child_by_field_name("right")),
            _ => (node.child_by_field_name("lhs"), node.child_by_field_name("op"), node.child_by_field_name("rhs")),
        };
        let (Some(left), Some(operator), Some(right)) = (left, operator, right) else { return };
        let (left, operator, right) = (squash(self.text_of(left)), self.text_of(operator), squash(self.text_of(right)));
        let size = match self.lang {
            Lang::Rust => ".len()",
            _ => ".count",
        };
        let zero_test = matches!((operator, right.as_str()), ("==" | "!=" | ">", "0") | ("<" | ">=", "1"));
        if left.ends_with(size) && zero_test {
            self.hit("len-zero", node);
        }
        if self.lang == Lang::Rust && matches!(operator, "==" | "!=") && !left.contains('[') && !right.contains('[') && (matches!(right.as_str(), "true" | "false") || matches!(left.as_str(), "true" | "false")) {
            self.hit("bool-comparison", node);
        }
    }

    fn arm_rules(&mut self, node: Node) {
        let Some(pattern) = node.child_by_field_name("pattern") else { return };
        let pattern = squash(self.text_of(pattern));
        let Some(value) = node.child_by_field_name("value") else { return };
        let value = squash(self.text_of(value));
        let value = value.trim_end_matches(',');
        if let Some(name) = pattern.strip_prefix("Err(").and_then(|p| p.strip_suffix(')')) {
            if name != "_" && (value == format!("returnErr({name})") || value == format!("returnErr({name});") || value == format!("{{returnErr({name});}}") || value == format!("Err({name})?")) {
                self.hit("error-rethrow", node);
            }
            if value == "{}" || value == "()" {
                self.hit("error-swallowed", node);
            }
            let sentinel = value.trim_start_matches('{').trim_end_matches('}').trim_end_matches(';').trim_start_matches("return");
            if matches!(sentinel, "None" | "false" | "Vec::new()" | "vec![]" | "Default::default()" | "String::new()" | "0" | "\"\"" | "HashMap::new()") {
                self.hit("error-to-sentinel", node);
            }
        }
    }

    fn catch_rules(&mut self, node: Node) {
        let blocks = self.swift_blocks(node);
        let body = blocks.first().cloned().unwrap_or_default();
        let bound = node.child_by_field_name("error").map(|p| self.text_of(p).trim_start_matches("let ").trim().to_string());
        let name = bound.unwrap_or_else(|| "error".into());
        match body.as_slice() {
            [] => self.hit("error-swallowed", node),
            [only] => {
                let text = squash(self.text_of(*only));
                if text == format!("throw{name}") {
                    self.hit("error-rethrow", node);
                } else if matches!(text.as_str(), "returnnil" | "returnfalse" | "return[]" | "return[:]" | "return\"\"" | "return0") {
                    self.hit("error-to-sentinel", node);
                } else if text.starts_with("throw") && text.contains(&format!("\\({name}")) {
                    self.hit("error-rewrap", node);
                }
            }
            _ => {}
        }
    }

    fn ternary_rules(&mut self, node: Node) {
        let (Some(condition), Some(yes), Some(no)) = (node.child_by_field_name("condition"), node.child_by_field_name("if_true"), node.child_by_field_name("if_false")) else { return };
        let (condition, yes, no) = (self.text_of(condition), squash(self.text_of(yes)), squash(self.text_of(no)));
        if (yes == "true" && no == "false") || (yes == "false" && no == "true") {
            self.hit("needless-bool", node);
        } else if yes == no {
            self.hit("same-branches", node);
        } else if self.min_max(condition, &yes, &no) {
            self.hit("manual-min-max", node);
        }
    }

    fn call_rules(&mut self, node: Node) {
        let Some((_, method)) = self.method_call(node) else { return };
        let text = squash(self.text_of(node));
        if method == "map_err" {
            if let Some((_, closure)) = text.split_once(".map_err(|") {
                if let Some((parameter, body)) = closure.split_once('|') {
                    if body.starts_with("format!(") && !parameter.is_empty() && (body.contains(&format!("{{{parameter}")) || body.contains(&format!(",{parameter})")) || body.contains(&format!(",{parameter},"))) {
                        self.hit("error-rewrap", node);
                    }
                }
            }
        }
        if matches!(method, "into_iter" | "iter" | "len" | "is_empty") {
            if let Some(function) = node.child_by_field_name("function") {
                if let Some(value) = function.child_by_field_name("value") {
                    let inner = squash(self.text_of(value));
                    if inner.ends_with(")") && (inner.contains(".collect::<Vec<") && inner.rsplit_once(".collect::<Vec<").is_some_and(|(_, tail)| tail.ends_with(">>()") && !tail.contains('.'))) {
                        self.hit("needless-collect", node);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tree_sitter::Parser;

    fn rules(lang: Lang, text: &str) -> Vec<&'static str> {
        let mut parser = Parser::new();
        let grammar: tree_sitter::Language = match lang {
            Lang::Rust => tree_sitter_rust::LANGUAGE.into(),
            _ => tree_sitter_swift::LANGUAGE.into(),
        };
        parser.set_language(&grammar).unwrap();
        let tree = parser.parse(text, None).unwrap();
        let mut found: Vec<&str> = check(lang, tree.root_node(), text).into_iter().map(|h| h.rule).collect();
        found.sort();
        found.dedup();
        found
    }

    #[test]
    fn rust_rules_find_what_clippy_and_scb_would_rewrite() {
        let cases: &[(&str, &[&str])] = &[
            ("fn f(a: u8) -> u8 { if a > 1 { return 1; } else if a > 0 { 2 } else { 3 } }", &["unnecessary-else"]),
            ("fn f(a: bool, b: bool) { if a { if b { g(); } } }", &["collapsible-if"]),
            ("fn f(a: Option<u8>, b: bool) { if let Some(x) = a { if b { g(x); } } }", &[]),
            ("fn f(a: bool) -> bool { if a { true } else { false } }", &["needless-bool"]),
            ("fn f(a: bool) -> u8 { if a { g(1) } else { g(1) } }", &["same-branches"]),
            ("fn f(a: u8) -> u8 { if a == 1 { 1 } else if a == 1 { 2 } else { 3 } }", &["same-condition"]),
            ("fn f(xs: &[u8]) { for x in xs { g(*x); continue; } }", &["needless-continue"]),
            ("fn f() -> u8 { return 1; }", &["needless-return"]),
            ("fn f(xs: &[u8]) -> Vec<u8> { xs.iter().map(|x| g(x)).collect() }", &["redundant-closure"]),
            ("fn f(xs: &[u8]) -> Vec<u8> { xs.iter().map(|x| x.clone()).collect() }", &["identity-map"]),
            ("fn f(xs: &[u8]) -> bool { xs.len() == 0 }", &["len-zero"]),
            ("fn f(a: bool) -> bool { a == true }", &["bool-comparison"]),
            ("fn f(xs: &[u8]) -> Vec<u8> { let mut v = Vec::new(); for x in xs { v.push(x + 1); } v }", &["loop-push"]),
            ("fn f(xs: &[u8]) -> u8 { let mut t = 0; for x in xs { t += x; } t }", &["loop-accumulate"]),
            ("fn f(v: &mut Vec<u8>) { v.push(1); v.push(2); }", &["consecutive-push"]),
            ("fn f(a: bool, b: bool) { if a { if b { return; } g(); } }", &["nested-guard-invert"]),
            ("fn f(a: bool, b: bool) { if a { if b { return; } g(); } h(); }", &[]),
            ("fn f(v: &serde_json::Value) -> bool { v[\"ok\"] == true }", &[]),
            ("fn f(xs: &[u8]) -> Vec<u8> { xs.iter().map(|x| g(x, 1)).collect() }", &[]),
            ("fn f(xs: &[u8]) { for x in xs { if *x == 1 { continue; } if *x == 2 { continue; } g(x); } }", &["repeated-if-continue"]),
            ("fn f(xs: &[u8]) -> Option<u8> { for x in xs { if *x > 1 { return Some(*x); } } None }", &["manual-find"]),
            ("fn f(a: u8, b: u8) -> u8 { if a > b { a } else { b } }", &["manual-min-max"]),
            ("fn f(mut a: Option<u8>) { if a.is_none() { a = Some(1); } }", &["manual-default"]),
            ("fn f(m: &mut M) { if !m.contains_key(&k) { m.insert(k, 1); } }", &["manual-entry"]),
            ("fn f(a: Option<u8>) -> u8 { if a.is_some() { a.unwrap() } else { 0 } }", &["check-then-unwrap"]),
            ("fn f(a: Option<u8>) -> Option<u8> { let Some(x) = a else { return None; }; Some(x) }", &["none-guard"]),
            ("fn f(r: R) -> R { match r { Ok(v) => Ok(v), Err(e) => return Err(e), } }", &["error-rethrow"]),
            ("fn f(r: R) { match r { Ok(v) => g(v), Err(_) => {} } }", &["error-swallowed"]),
            ("fn f(r: R) -> Option<u8> { match r { Ok(v) => Some(v), Err(_) => None } }", &["error-to-sentinel"]),
            ("fn f(r: R) -> Result<u8, String> { r.map_err(|e| format!(\"bad: {e}\")) }", &["error-rewrap"]),
            ("fn f(a: &str) -> u8 { if a == \"x\" { 1 } else if a == \"y\" { 2 } else if a == \"z\" { 3 } else { 4 } }", &["string-dispatch"]),
            ("fn f(xs: &[u8]) -> usize { xs.iter().collect::<Vec<_>>().len() }", &["needless-collect"]),
            ("fn f(t: T) { let (a, b, c, d, e) = t; }", &["long-tuple"]),
            ("fn f(p: (u8, u8)) { let a = p.0; let b = p.1; }", &["index-unpack"]),
            ("// ==========\nfn f() {}", &["banner-comment"]),
            ("fn f(a: u8) -> u8 { if a > 1 { 1 } else { 2 } }", &[]),
            ("fn f(xs: &[u8]) -> Vec<u8> { xs.iter().map(|x| x + 1).collect() }", &[]),
        ];
        for (text, expected) in cases {
            assert_eq!(rules(Lang::Rust, text), *expected, "{text}");
        }
        let soup = "fn f() -> Result<(), String> {\n    if let Err(e) = a() { log(e); }\n    match b() { Ok(_) => {}, Err(e) => log(e) }\n    let Ok(c) = c() else { return Ok(()) };\n    Ok(())\n}";
        assert!(rules(Lang::Rust, soup).contains(&"error-soup"));
        let formatted = "fn f(a: u8) -> Result<(), String> {\n    if a > 1 { return Err(format!(\"{a} big\")); }\n    if a > 2 { return Err(format!(\"{a} bigger\")); }\n    Err(format!(\"{a} biggest\"))\n}";
        assert!(rules(Lang::Rust, formatted).contains(&"formatted-error-heavy"));
    }

    #[test]
    fn swift_rules_find_the_same_shapes() {
        let cases: &[(&str, &[&str])] = &[
            ("func f(a: Int) -> Int { if a > 1 { return 1 } else if a > 0 { return 2 } else { return 3 } }", &["unnecessary-else"]),
            ("func f(a: Bool, b: Bool) { if a { if b { g() } } }", &["collapsible-if"]),
            ("func f(a: Bool) -> Bool { return a ? true : false }", &["needless-bool"]),
            ("func f(xs: [Int]) { for x in xs { out.append(x * 2) } }", &["loop-push"]),
            ("func f() { do { try g() } catch { } }", &["error-swallowed"]),
            ("func f() throws { do { try g() } catch { throw error } }", &["error-rethrow"]),
            ("func f() -> Int? { do { return try g() } catch { return nil } }", &["error-to-sentinel"]),
            ("func f(xs: [Int]) -> [Int] { xs.map { $0 } }", &["identity-map"]),
            ("func f(xs: [Int]) -> [Int] { xs.map { g($0) } }", &["redundant-closure"]),
            ("func f(xs: [Int]) -> Bool { xs.count == 0 }", &["len-zero"]),
            ("func f() { d[\"a\"] = g(1)\n    d[\"b\"] = g(2)\n}", &["repeated-key-assignment"]),
            ("func f() { d[\"a\"] = { x in x }\n    d[\"b\"] = { x in x }\n}", &[]),
            ("func f() { if v == nil { v = 3 } }", &["manual-default"]),
            ("func f() { if d[k] == nil { d[k] = 3 } }", &["manual-entry"]),
            ("func f() { if v != nil { g(v!) } }", &["check-then-unwrap"]),
            ("func f(a: Int, b: Int) -> Int { a > b ? a : b }", &["manual-min-max"]),
            ("func f() { g()\n    return\n}", &["needless-return"]),
            ("func f(a: A?, b: B?, c: C?) -> D? {\n    guard let a else { return nil }\n    guard let b else { return nil }\n    guard let c else { return nil }\n    return D(a, b, c)\n}", &["none-guard"]),
            ("func f(a: Int) -> Int { if a > 1 { return 1 } else { return 2 } }", &[]),
        ];
        for (text, expected) in cases {
            assert_eq!(rules(Lang::Swift, text), *expected, "{text}");
        }
    }
}
