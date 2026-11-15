use std::collections::{HashMap, HashSet};
use std::path::Path;

use tree_sitter::{Language, Node, Parser};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SourceLang {
    Rust,
    TypeScript,
    Tsx,
    JavaScript,
    Python,
    Go,
}

impl SourceLang {
    pub(crate) fn from_path(path: &Path) -> Option<Self> {
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        match ext.as_str() {
            "rs" => Some(Self::Rust),
            "ts" | "mts" | "cts" => Some(Self::TypeScript),
            "tsx" => Some(Self::Tsx),
            "js" | "jsx" | "mjs" | "cjs" => Some(Self::JavaScript),
            "py" | "pyi" => Some(Self::Python),
            "go" => Some(Self::Go),
            _ => None,
        }
    }

    fn language(self) -> Language {
        match self {
            Self::Rust => tree_sitter_rust::LANGUAGE.into(),
            Self::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            Self::Tsx => tree_sitter_typescript::LANGUAGE_TSX.into(),
            Self::JavaScript => tree_sitter_javascript::LANGUAGE.into(),
            Self::Python => tree_sitter_python::LANGUAGE.into(),
            Self::Go => tree_sitter_go::LANGUAGE.into(),
        }
    }

    fn is_ident_kind(self, kind: &str) -> bool {
        match self {
            Self::Rust => matches!(kind, "identifier" | "type_identifier" | "field_identifier"),
            Self::TypeScript | Self::Tsx | Self::JavaScript => matches!(
                kind,
                "identifier"
                    | "type_identifier"
                    | "property_identifier"
                    | "shorthand_property_identifier"
                    | "shorthand_property_identifier_pattern"
            ),
            Self::Python => kind == "identifier",
            Self::Go => matches!(
                kind,
                "identifier" | "type_identifier" | "field_identifier" | "package_identifier"
            ),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SymbolDef {
    pub(crate) kind: &'static str,
    pub(crate) name: String,
    pub(crate) line: usize,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct FileTags {
    pub(crate) definitions: Vec<SymbolDef>,
    pub(crate) references: HashMap<String, usize>,
}

pub(crate) fn extract_tags(lang: SourceLang, source: &str) -> FileTags {
    let mut parser = Parser::new();
    if parser.set_language(&lang.language()).is_err() {
        return FileTags::default();
    }
    let Some(tree) = parser.parse(source, None) else {
        return FileTags::default();
    };

    let bytes = source.as_bytes();
    let mut definitions = Vec::new();
    let mut def_spans = HashSet::new();
    collect_defs(
        lang,
        tree.root_node(),
        bytes,
        true,
        &mut definitions,
        &mut def_spans,
    );

    let mut references = HashMap::new();
    collect_refs(lang, tree.root_node(), bytes, &def_spans, &mut references);

    FileTags {
        definitions,
        references,
    }
}

fn node_text<'a>(node: Node<'_>, bytes: &'a [u8]) -> Option<&'a str> {
    node.utf8_text(bytes).ok().filter(|s| !s.is_empty())
}

fn push_def(
    kind: &'static str,
    name_node: Node<'_>,
    bytes: &[u8],
    definitions: &mut Vec<SymbolDef>,
    def_spans: &mut HashSet<(usize, usize)>,
) {
    if let Some(name) = node_text(name_node, bytes) {
        definitions.push(SymbolDef {
            kind,
            name: name.to_string(),
            line: name_node.start_position().row + 1,
        });
        def_spans.insert((name_node.start_byte(), name_node.end_byte()));
    }
}

fn collect_defs(
    lang: SourceLang,
    node: Node<'_>,
    bytes: &[u8],
    top_level: bool,
    definitions: &mut Vec<SymbolDef>,
    def_spans: &mut HashSet<(usize, usize)>,
) {
    match lang {
        SourceLang::Rust => collect_rust(node, bytes, definitions, def_spans),
        SourceLang::TypeScript | SourceLang::Tsx => {
            collect_ts_js(lang, node, bytes, top_level, definitions, def_spans);
        }
        SourceLang::JavaScript => {
            collect_ts_js(lang, node, bytes, top_level, definitions, def_spans);
        }
        SourceLang::Python => collect_python(node, bytes, top_level, definitions, def_spans),
        SourceLang::Go => collect_go(node, bytes, top_level, definitions, def_spans),
    }
}

fn collect_rust(
    node: Node<'_>,
    bytes: &[u8],
    definitions: &mut Vec<SymbolDef>,
    def_spans: &mut HashSet<(usize, usize)>,
) {
    let kind = node.kind();
    let sym_kind = match kind {
        "function_item" | "function_signature_item" => Some("fn"),
        "struct_item" => Some("struct"),
        "enum_item" => Some("enum"),
        "trait_item" => Some("trait"),
        "type_item" => Some("type"),
        "const_item" | "static_item" => Some("const"),
        "mod_item" => Some("mod"),
        _ => None,
    };

    if let Some(sym_kind) = sym_kind {
        if let Some(name_node) = node.child_by_field_name("name") {
            push_def(sym_kind, name_node, bytes, definitions, def_spans);
        }
        if matches!(kind, "mod_item" | "trait_item") {
            recurse_children(SourceLang::Rust, node, bytes, false, definitions, def_spans);
        }
        return;
    }

    if kind == "impl_item" {
        let target = node
            .child_by_field_name("type")
            .and_then(|n| node_text(n, bytes));
        let trait_name = node
            .child_by_field_name("trait")
            .and_then(|n| node_text(n, bytes));
        let label = match (trait_name, target) {
            (Some(tr), Some(ty)) => Some(format!("{tr} for {ty}")),
            (None, Some(ty)) => Some(ty.to_string()),
            _ => None,
        };
        if let Some(name) = label {
            definitions.push(SymbolDef {
                kind: "impl",
                name,
                line: node.start_position().row + 1,
            });
        }
        recurse_children(SourceLang::Rust, node, bytes, false, definitions, def_spans);
        return;
    }

    recurse_children(SourceLang::Rust, node, bytes, true, definitions, def_spans);
}

fn collect_ts_js(
    lang: SourceLang,
    node: Node<'_>,
    bytes: &[u8],
    top_level: bool,
    definitions: &mut Vec<SymbolDef>,
    def_spans: &mut HashSet<(usize, usize)>,
) {
    let kind = node.kind();
    match kind {
        "function_declaration" | "function_signature" | "generator_function_declaration" => {
            if let Some(name_node) = node.child_by_field_name("name") {
                push_def("fn", name_node, bytes, definitions, def_spans);
            }
            return;
        }
        "method_definition" | "method_signature" | "abstract_method_signature" => {
            if let Some(name_node) = node.child_by_field_name("name")
                && node_text(name_node, bytes) != Some("constructor")
            {
                push_def("fn", name_node, bytes, definitions, def_spans);
            }
            return;
        }
        "class_declaration" | "abstract_class_declaration" => {
            if let Some(name_node) = node.child_by_field_name("name") {
                push_def("class", name_node, bytes, definitions, def_spans);
            }
            recurse_children(lang, node, bytes, false, definitions, def_spans);
            return;
        }
        "interface_declaration" => {
            if let Some(name_node) = node.child_by_field_name("name") {
                push_def("interface", name_node, bytes, definitions, def_spans);
            }
            return;
        }
        "type_alias_declaration" => {
            if let Some(name_node) = node.child_by_field_name("name") {
                push_def("type", name_node, bytes, definitions, def_spans);
            }
            return;
        }
        "enum_declaration" => {
            if let Some(name_node) = node.child_by_field_name("name") {
                push_def("enum", name_node, bytes, definitions, def_spans);
            }
            return;
        }
        "internal_module" | "module" => {
            if let Some(name_node) = node.child_by_field_name("name") {
                push_def("mod", name_node, bytes, definitions, def_spans);
            }
            recurse_children(lang, node, bytes, false, definitions, def_spans);
            return;
        }
        "lexical_declaration" | "variable_declaration" => {
            let is_const = node
                .child(0)
                .and_then(|c| node_text(c, bytes))
                .is_some_and(|t| t == "const");
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                if child.kind() == "variable_declarator"
                    && let Some(name_node) = child.child_by_field_name("name")
                {
                    let is_fn = child.child_by_field_name("value").is_some_and(|v| {
                        matches!(v.kind(), "arrow_function" | "function_expression")
                    });
                    if is_fn {
                        push_def("fn", name_node, bytes, definitions, def_spans);
                    } else if top_level && is_const && name_node.kind() == "identifier" {
                        push_def("const", name_node, bytes, definitions, def_spans);
                    }
                }
            }
            return;
        }
        "export_statement" => {
            recurse_children(lang, node, bytes, top_level, definitions, def_spans);
            return;
        }
        _ => {}
    }

    recurse_children(lang, node, bytes, top_level, definitions, def_spans);
}

fn collect_python(
    node: Node<'_>,
    bytes: &[u8],
    top_level: bool,
    definitions: &mut Vec<SymbolDef>,
    def_spans: &mut HashSet<(usize, usize)>,
) {
    let kind = node.kind();
    match kind {
        "function_definition" => {
            if let Some(name_node) = node.child_by_field_name("name") {
                push_def("fn", name_node, bytes, definitions, def_spans);
            }
            return;
        }
        "class_definition" => {
            if let Some(name_node) = node.child_by_field_name("name") {
                push_def("class", name_node, bytes, definitions, def_spans);
            }
            recurse_children(
                SourceLang::Python,
                node,
                bytes,
                false,
                definitions,
                def_spans,
            );
            return;
        }
        "decorated_definition" => {
            recurse_children(
                SourceLang::Python,
                node,
                bytes,
                top_level,
                definitions,
                def_spans,
            );
            return;
        }
        "expression_statement" if top_level => {
            if let Some(assign) = node.named_child(0)
                && assign.kind() == "assignment"
                && let Some(left) = assign.child_by_field_name("left")
                && left.kind() == "identifier"
            {
                push_def("const", left, bytes, definitions, def_spans);
            }
            return;
        }
        _ => {}
    }

    recurse_children(
        SourceLang::Python,
        node,
        bytes,
        top_level,
        definitions,
        def_spans,
    );
}

fn collect_go(
    node: Node<'_>,
    bytes: &[u8],
    top_level: bool,
    definitions: &mut Vec<SymbolDef>,
    def_spans: &mut HashSet<(usize, usize)>,
) {
    let kind = node.kind();
    match kind {
        "function_declaration" | "method_declaration" => {
            if let Some(name_node) = node.child_by_field_name("name") {
                push_def("fn", name_node, bytes, definitions, def_spans);
            }
            return;
        }
        "type_spec" => {
            if let Some(name_node) = node.child_by_field_name("name") {
                let sym_kind = match node.child_by_field_name("type").map(|t| t.kind()) {
                    Some("struct_type") => "struct",
                    Some("interface_type") => "interface",
                    _ => "type",
                };
                push_def(sym_kind, name_node, bytes, definitions, def_spans);
            }
            return;
        }
        "const_spec" if top_level => {
            if let Some(name_node) = node.child_by_field_name("name") {
                push_def("const", name_node, bytes, definitions, def_spans);
            }
            return;
        }
        _ => {}
    }

    recurse_children(
        SourceLang::Go,
        node,
        bytes,
        top_level,
        definitions,
        def_spans,
    );
}

fn recurse_children(
    lang: SourceLang,
    node: Node<'_>,
    bytes: &[u8],
    top_level: bool,
    definitions: &mut Vec<SymbolDef>,
    def_spans: &mut HashSet<(usize, usize)>,
) {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        collect_defs(lang, child, bytes, top_level, definitions, def_spans);
    }
}

fn collect_refs(
    lang: SourceLang,
    node: Node<'_>,
    bytes: &[u8],
    def_spans: &HashSet<(usize, usize)>,
    references: &mut HashMap<String, usize>,
) {
    if lang.is_ident_kind(node.kind())
        && !def_spans.contains(&(node.start_byte(), node.end_byte()))
        && let Some(text) = node_text(node, bytes)
        && text.len() > 1
    {
        *references.entry(text.to_string()).or_insert(0) += 1;
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_refs(lang, child, bytes, def_spans, references);
    }
}
