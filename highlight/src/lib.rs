mod escape;

use serde::{Deserialize, Deserializer};
use serde_derive::*;
use std::cmp;
use std::fmt::Write;
use std::mem::transmute;
use std::str;
use std::usize;
use tree_sitter::{Language, Node, Parser, Point, PropertySheet, Range, Tree, TreePropertyCursor};

pub trait LanguageRegistry {
    fn language_for_injection_string<'a>(
        &'a self,
        s: &str,
    ) -> Option<(Language, &'a PropertySheet<Properties>)>;
}

#[derive(Debug)]
enum TreeStep {
    Child {
        index: isize,
        kinds: Option<Vec<u16>>,
    },
    Children {
        kinds: Option<Vec<u16>>,
    },
    Next {
        kinds: Option<Vec<u16>>,
    },
}

#[derive(Debug)]
enum InjectionLanguage {
    Literal(String),
    TreePath(Vec<TreeStep>),
}

#[derive(Debug)]
struct Injection {
    language: InjectionLanguage,
    content: Vec<TreeStep>,
}

#[derive(Debug)]
pub struct Properties {
    scope: Option<Scope>,
    injections: Vec<Injection>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(u16)]
pub enum Scope {
    Attribute,
    Comment,
    Constant,
    ConstantBuiltin,
    Constructor,
    ConstructorBuiltin,
    Embedded,
    Escape,
    Function,
    FunctionBuiltin,
    Keyword,
    Number,
    Operator,
    Property,
    PropertyBuiltin,
    Punctuation,
    PunctuationBracket,
    PunctuationDelimiter,
    PunctuationSpecial,
    String,
    StringSpecial,
    Tag,
    Type,
    TypeBuiltin,
    Variable,
    VariableBuiltin,
    Unknown,
}

struct Layer<'a> {
    _tree: Tree,
    cursor: TreePropertyCursor<'a, Properties>,
    ranges: Vec<Range>,
    at_node_end: bool,
}

struct Highlighter<'a, T: LanguageRegistry> {
    language_registry: &'a T,
    source: &'a [u8],
    source_offset: usize,
    parser: Parser,
    layers: Vec<Layer<'a>>,
    utf8_error_len: Option<usize>,
}

#[derive(Copy, Clone, Debug)]
pub enum HighlightEvent<'a> {
    Source(&'a str),
    ScopeStart(Scope),
    ScopeEnd(Scope),
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum TreePathArgJSON {
    TreePath(TreePathJSON),
    Number(isize),
    String(String),
}

#[derive(Debug, Deserialize)]
#[serde(tag = "name")]
enum TreePathJSON {
    #[serde(rename = "this")]
    This,
    #[serde(rename = "child")]
    Child { args: Vec<TreePathArgJSON> },
    #[serde(rename = "next")]
    Next { args: Vec<TreePathArgJSON> },
    #[serde(rename = "children")]
    Children { args: Vec<TreePathArgJSON> },
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum InjectionLanguageJSON {
    List(Vec<InjectionLanguageJSON>),
    TreePath(TreePathJSON),
    Literal(String),
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum InjectionContentJSON {
    List(Vec<InjectionContentJSON>),
    TreePath(TreePathJSON),
}

#[derive(Debug, Deserialize)]
struct PropertiesJSON {
    scope: Option<Scope>,
    #[serde(rename = "injection-language")]
    injection_language: Option<InjectionLanguageJSON>,
    #[serde(rename = "injection-content")]
    injection_content: Option<InjectionContentJSON>,
}

#[derive(Debug)]
pub enum PropertySheetError {
    InvalidJSON(serde_json::Error),
    InvalidRegex(regex::Error),
    InvalidFormat(String),
}

pub fn load_property_sheet(
    language: Language,
    json: &str,
) -> Result<PropertySheet<Properties>, PropertySheetError> {
    let sheet = PropertySheet::new(language, json).map_err(|e| match e {
        tree_sitter::PropertySheetError::InvalidJSON(e) => PropertySheetError::InvalidJSON(e),
        tree_sitter::PropertySheetError::InvalidRegex(e) => PropertySheetError::InvalidRegex(e),
    })?;
    let sheet = sheet
        .map(|p| Properties::new(p, language))
        .map_err(PropertySheetError::InvalidFormat)?;
    Ok(sheet)
}

impl Properties {
    fn new(json: PropertiesJSON, language: Language) -> Result<Self, String> {
        let injections = match (json.injection_language, json.injection_content) {
            (None, None) => Ok(Vec::new()),
            (Some(_), None) => Err(
                "Must specify an injection-content along with an injection-language".to_string(),
            ),
            (None, Some(_)) => Err(
                "Must specify an injection-language along with an injection-content".to_string(),
            ),
            (Some(language_json), Some(content_json)) => {
                let languages = match language_json {
                    InjectionLanguageJSON::List(list) => {
                        let mut result = Vec::with_capacity(list.len());
                        for element in list {
                            result.push(match element {
                                InjectionLanguageJSON::TreePath(p) => {
                                    let mut result = Vec::new();
                                    Self::flatten_tree_path(p, &mut result, language)?;
                                    InjectionLanguage::TreePath(result)
                                }
                                InjectionLanguageJSON::Literal(s) => InjectionLanguage::Literal(s),
                                InjectionLanguageJSON::List(_) => {
                                    panic!("Injection-language cannot be a list of lists")
                                }
                            })
                        }
                        result
                    }
                    InjectionLanguageJSON::TreePath(p) => vec![{
                        let mut result = Vec::new();
                        Self::flatten_tree_path(p, &mut result, language)?;
                        InjectionLanguage::TreePath(result)
                    }],
                    InjectionLanguageJSON::Literal(s) => vec![InjectionLanguage::Literal(s)],
                };

                let contents = match content_json {
                    InjectionContentJSON::List(l) => {
                        let mut result = Vec::with_capacity(l.len());
                        for element in l {
                            result.push(match element {
                                InjectionContentJSON::TreePath(p) => {
                                    let mut result = Vec::new();
                                    Self::flatten_tree_path(p, &mut result, language)?;
                                    result
                                }
                                InjectionContentJSON::List(_) => {
                                    panic!("Injection-content cannot be a list of lists")
                                }
                            })
                        }
                        result
                    }
                    InjectionContentJSON::TreePath(p) => vec![{
                        let mut result = Vec::new();
                        Self::flatten_tree_path(p, &mut result, language)?;
                        result
                    }],
                };

                if languages.len() == contents.len() {
                    Ok(languages
                        .into_iter()
                        .zip(contents.into_iter())
                        .map(|(language, content)| Injection { language, content })
                        .collect())
                } else {
                    Err(format!(
                        "Mismatch: got {} injection-language values but {} injection-content values",
                        languages.len(),
                        contents.len(),
                    ))
                }
            }
        }?;

        Ok(Self {
            scope: json.scope,
            injections,
        })
    }

    // Transform a tree path from the format expressed directly in the property sheet
    // (nested function calls), to a flat sequence of steps for transforming a list of
    // nodes. This way, we can evaluate these tree paths with no recursion and a single
    // vector of intermediate storage.
    fn flatten_tree_path(
        p: TreePathJSON,
        steps: &mut Vec<TreeStep>,
        language: Language,
    ) -> Result<(), String> {
        match p {
            TreePathJSON::This => {}
            TreePathJSON::Child { args } => {
                let (tree_path, index, kinds) = Self::parse_args("child", args, language)?;
                Self::flatten_tree_path(tree_path, steps, language)?;
                steps.push(TreeStep::Child {
                    index: index
                        .ok_or_else(|| "The `child` function requires an index".to_string())?,
                    kinds: kinds,
                });
            }
            TreePathJSON::Children { args } => {
                let (tree_path, _, kinds) = Self::parse_args("children", args, language)?;
                Self::flatten_tree_path(tree_path, steps, language)?;
                steps.push(TreeStep::Children { kinds });
            }
            TreePathJSON::Next { args } => {
                let (tree_path, _, kinds) = Self::parse_args("next", args, language)?;
                Self::flatten_tree_path(tree_path, steps, language)?;
                steps.push(TreeStep::Next { kinds });
            }
        }
        Ok(())
    }

    fn parse_args(
        name: &str,
        args: Vec<TreePathArgJSON>,
        language: Language,
    ) -> Result<(TreePathJSON, Option<isize>, Option<Vec<u16>>), String> {
        let tree_path;
        let mut index = None;
        let mut kinds = Vec::new();
        let mut iter = args.into_iter();

        match iter.next() {
            Some(TreePathArgJSON::TreePath(p)) => tree_path = p,
            _ => {
                return Err(format!(
                    "First argument to `{}()` must be a tree path",
                    name
                ));
            }
        }

        for arg in iter {
            match arg {
                TreePathArgJSON::TreePath(_) => {
                    return Err(format!(
                        "Other arguments to `{}()` must be strings or numbers",
                        name
                    ));
                }
                TreePathArgJSON::Number(i) => index = Some(i),
                TreePathArgJSON::String(s) => kinds.push(s),
            }
        }

        if kinds.len() > 0 {
            let mut kind_ids = Vec::new();
            for i in 0..(language.node_kind_count() as u16) {
                if kinds.iter().any(|s| s == language.node_kind_for_id(i))
                    && language.node_kind_is_named(i)
                {
                    kind_ids.push(i);
                }
            }
            if kind_ids.len() == 0 {
                return Err(format!("Non-existent node kinds: {:?}", kinds));
            }

            Ok((tree_path, index, Some(kind_ids)))
        } else {
            Ok((tree_path, index, None))
        }
    }
}

impl<'a, T: LanguageRegistry> Highlighter<'a, T> {
    fn new(
        language_registry: &'a T,
        source: &'a [u8],
        language: Language,
        property_sheet: &'a PropertySheet<Properties>,
    ) -> Result<Self, String> {
        let mut parser = Parser::new();
        parser.set_language(language)?;
        let tree = parser
            .parse(source, None)
            .ok_or_else(|| format!("Tree-sitter: failed to parse"))?;
        Ok(Self {
            language_registry,
            source,
            source_offset: 0,
            parser,
            layers: vec![Layer::new(
                source,
                tree,
                property_sheet,
                vec![Range {
                    start_byte: 0,
                    end_byte: usize::MAX,
                    start_point: Point::new(0, 0),
                    end_point: Point::new(usize::MAX, usize::MAX),
                }],
            )],
            utf8_error_len: None,
        })
    }

    fn emit_source(&mut self, next_offset: usize) -> Option<HighlightEvent<'a>> {
        let input = &self.source[self.source_offset..next_offset];
        match str::from_utf8(input) {
            Ok(valid) => {
                self.source_offset = next_offset;
                Some(HighlightEvent::Source(valid))
            }
            Err(error) => {
                if let Some(error_len) = error.error_len() {
                    if error.valid_up_to() > 0 {
                        let prefix = &input[0..error.valid_up_to()];
                        self.utf8_error_len = Some(error_len);
                        Some(HighlightEvent::Source(unsafe {
                            str::from_utf8_unchecked(prefix)
                        }))
                    } else {
                        self.source_offset += error_len;
                        Some(HighlightEvent::Source("\u{FFFD}"))
                    }
                } else {
                    None
                }
            }
        }
    }

    fn process_tree_step(&self, step: &TreeStep, nodes: &mut Vec<Node>) {
        let len = nodes.len();
        for i in 0..len {
            let node = nodes[i];
            match step {
                TreeStep::Child { index, kinds } => {
                    let index = if *index >= 0 {
                        *index as usize
                    } else {
                        (node.child_count() as isize + *index) as usize
                    };
                    if let Some(child) = node.child(index) {
                        if let Some(kinds) = kinds {
                            if kinds.contains(&child.kind_id()) {
                                nodes.push(child);
                            }
                        } else {
                            nodes.push(child);
                        }
                    }
                }
                TreeStep::Children { kinds } => {
                    for child in node.children() {
                        if let Some(kinds) = kinds {
                            if kinds.contains(&child.kind_id()) {
                                nodes.push(child);
                            }
                        } else {
                            nodes.push(child);
                        }
                    }
                }
                TreeStep::Next { .. } => unimplemented!(),
            }
        }
        nodes.drain(0..len);
    }

    fn nodes_for_tree_path(&self, node: Node<'a>, steps: &Vec<TreeStep>) -> Vec<Node<'a>> {
        let mut nodes = vec![node];
        for step in steps.iter() {
            self.process_tree_step(step, &mut nodes);
        }
        nodes
    }

    // An injected language name may either be specified as a fixed string, or based
    // on the text of some node in the syntax tree.
    fn injection_language_string(
        &self,
        node: &Node,
        language: &InjectionLanguage,
    ) -> Option<String> {
        match language {
            InjectionLanguage::Literal(s) => Some(s.to_string()),
            InjectionLanguage::TreePath(steps) => self
                .nodes_for_tree_path(*node, steps)
                .first()
                .and_then(|node| {
                    str::from_utf8(&self.source[node.start_byte()..node.end_byte()])
                        .map(|s| s.to_owned())
                        .ok()
                }),
        }
    }

    // Compute the ranges that should be included when parsing an injection.
    // This takes into account two things:
    // * `nodes` - Every injection takes place within a set of nodes. The injection ranges
    //   are the ranges of those nodes, *minus* the ranges of those nodes' children.
    // * `parent_ranges` - The new injection may be nested inside of *another* injection
    //   (e.g. JavaScript within HTML within ERB). The parent injection's ranges must
    //   be taken into account.
    fn intersect_ranges(parent_ranges: &Vec<Range>, nodes: &Vec<Node>) -> Vec<Range> {
        let mut result = Vec::new();
        let mut parent_range_iter = parent_ranges.iter();
        let mut parent_range = parent_range_iter
            .next()
            .expect("Layers should only be constructed with non-empty ranges vectors");
        for node in nodes.iter() {
            let range = node.range();
            let mut preceding_range = Range {
                start_byte: 0,
                start_point: Point::new(0, 0),
                end_byte: range.start_byte,
                end_point: range.start_point,
            };
            let following_range = Range {
                start_byte: node.end_byte(),
                start_point: node.end_position(),
                end_byte: usize::MAX,
                end_point: Point::new(usize::MAX, usize::MAX),
            };

            for child_range in node
                .children()
                .map(|c| c.range())
                .chain([following_range].iter().cloned())
            {
                let mut range = Range {
                    start_byte: preceding_range.end_byte,
                    start_point: preceding_range.end_point,
                    end_byte: child_range.start_byte,
                    end_point: child_range.start_point,
                };
                preceding_range = child_range;

                if range.end_byte < parent_range.start_byte {
                    continue;
                }

                while parent_range.start_byte <= range.end_byte {
                    if parent_range.end_byte > range.start_byte {
                        if range.start_byte < parent_range.start_byte {
                            range.start_byte = parent_range.start_byte;
                            range.start_point = parent_range.start_point;
                        }

                        if parent_range.end_byte < range.end_byte {
                            if range.start_byte < parent_range.end_byte {
                                result.push(Range {
                                    start_byte: range.start_byte,
                                    start_point: range.start_point,
                                    end_byte: parent_range.end_byte,
                                    end_point: parent_range.end_point,
                                });
                            }
                            range.start_byte = parent_range.end_byte;
                            range.start_point = parent_range.end_point;
                        } else {
                            if range.start_byte < range.end_byte {
                                result.push(range);
                            }
                            break;
                        }
                    }

                    if let Some(next_range) = parent_range_iter.next() {
                        parent_range = next_range;
                    } else {
                        return result;
                    }
                }
            }
        }
        result
    }

    fn add_layer(&mut self, language_string: &str, ranges: Vec<Range>) {
        if let Some((language, property_sheet)) = self
            .language_registry
            .language_for_injection_string(language_string)
        {
            self.parser
                .set_language(language)
                .expect("Failed to set language");
            self.parser.set_included_ranges(&ranges);
            let tree = self
                .parser
                .parse(self.source, None)
                .expect("Failed to parse");
            let layer = Layer::new(self.source, tree, property_sheet, ranges);
            match self
                .layers
                .binary_search_by_key(&(layer.offset(), 1), |l| (l.offset(), 0))
            {
                Ok(i) | Err(i) => self.layers.insert(i, layer),
            };
        }
    }
}

impl<'a, T: LanguageRegistry> Iterator for Highlighter<'a, T> {
    type Item = HighlightEvent<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if let Some(utf8_error_len) = self.utf8_error_len.take() {
            self.source_offset += utf8_error_len;
            return Some(HighlightEvent::Source("\u{FFFD}"));
        }

        while !self.layers.is_empty() {
            let first_layer = &self.layers[0];
            let properties = &first_layer.cursor.node_properties();

            // Add any injections for the current node.
            if !first_layer.at_node_end {
                let node = first_layer.cursor.node();
                let injections = properties
                    .injections
                    .iter()
                    .filter_map(|Injection { language, content }| {
                        if let Some(language) = self.injection_language_string(&node, language) {
                            let nodes = self.nodes_for_tree_path(node, content);
                            let ranges = Self::intersect_ranges(&first_layer.ranges, &nodes);
                            if ranges.len() > 0 {
                                return Some((language, ranges));
                            }
                        }
                        None
                    })
                    .collect::<Vec<_>>();

                for (language, ranges) in injections {
                    self.add_layer(&language, ranges);
                }
            }

            // Determine if any scopes start or end at the current position.
            let scope_event;
            if let Some(scope) = properties.scope {
                let next_offset = cmp::min(self.source.len(), self.layers[0].offset());

                // Before returning any scope boundaries, return any remaining slice of
                // the source code the precedes that scope boundary.
                if self.source_offset < next_offset {
                    return self.emit_source(next_offset);
                }

                scope_event = if self.layers[0].at_node_end {
                    Some(HighlightEvent::ScopeEnd(scope))
                } else {
                    Some(HighlightEvent::ScopeStart(scope))
                };
            } else {
                scope_event = None;
            };

            // Advance the current layer's tree cursor. This might cause that cursor to move
            // beyond one of the other layers' cursors for a different syntax tree, so we need
            // to re-sort the layers. If the cursor is already at the end of its syntax tree,
            // remove it.
            if self.layers[0].advance() {
                self.layers.sort_unstable_by_key(|layer| layer.offset());
            } else {
                self.layers.remove(0);
            }

            if scope_event.is_some() {
                return scope_event;
            }
        }

        if self.source_offset < self.source.len() {
            self.emit_source(self.source.len())
        } else {
            None
        }
    }
}

impl<'a> Layer<'a> {
    fn new(
        source: &'a [u8],
        tree: Tree,
        sheet: &'a PropertySheet<Properties>,
        ranges: Vec<Range>,
    ) -> Self {
        // The cursor's lifetime parameter indicates that the tree must outlive the cursor.
        // But because the tree is really a pointer to the heap, the cursor can remain
        // valid when the tree is moved. There's no way to express this with lifetimes
        // right now, so we have to `transmute` the cursor's lifetime.
        let cursor = unsafe { transmute(tree.walk_with_properties(sheet, source)) };
        Self {
            _tree: tree,
            cursor,
            ranges,
            at_node_end: false,
        }
    }

    fn offset(&self) -> usize {
        if self.at_node_end {
            self.cursor.node().end_byte()
        } else {
            self.cursor.node().start_byte()
        }
    }

    fn advance(&mut self) -> bool {
        if self.at_node_end {
            if self.cursor.goto_next_sibling() {
                self.at_node_end = false;
            } else if !self.cursor.goto_parent() {
                return false;
            }
        } else if !self.cursor.goto_first_child() {
            self.at_node_end = true;
        }
        true
    }
}

impl Scope {
    fn to_class_name(&self) -> &'static str {
        match self {
            Scope::Attribute => "pl-c1",
            Scope::Comment => "pl-c",
            Scope::Constant => "pl-c1",
            Scope::ConstantBuiltin => "pl-c1",
            Scope::Constructor => "pl-v",
            Scope::ConstructorBuiltin => "pl-v",
            Scope::Embedded => "pl-s1",
            Scope::Escape => "pl-cce",
            Scope::Function => "pl-en",
            Scope::FunctionBuiltin => "pl-en",
            Scope::Keyword => "pl-k",
            Scope::Number => "pl-c1",
            Scope::Operator => "pl-c1",
            Scope::Property => "pl-c1",
            Scope::PropertyBuiltin => "pl-c1",
            Scope::Punctuation => "pl-kos",
            Scope::PunctuationBracket => "pl-kos",
            Scope::PunctuationDelimiter => "pl-kos",
            Scope::PunctuationSpecial => "pl-kos",
            Scope::String => "pl-s",
            Scope::StringSpecial => "pl-pds",
            Scope::Tag => "pl-ent",
            Scope::Type => "pl-smi",
            Scope::TypeBuiltin => "pl-smi",
            Scope::Variable => "pl-s1",
            Scope::VariableBuiltin => "pl-smi",
            Scope::Unknown => "",
        }
    }
}

impl<'de> Deserialize<'de> for Scope {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        match s.as_str() {
            "attribute" => Ok(Scope::Attribute),
            "comment" => Ok(Scope::Comment),
            "constant" => Ok(Scope::Constant),
            "constant.builtin" => Ok(Scope::ConstantBuiltin),
            "constructor" => Ok(Scope::Constructor),
            "constructor.builtin" => Ok(Scope::ConstructorBuiltin),
            "embedded" => Ok(Scope::Embedded),
            "escape" => Ok(Scope::Escape),
            "function" => Ok(Scope::Function),
            "function.builtin" => Ok(Scope::FunctionBuiltin),
            "keyword" => Ok(Scope::Keyword),
            "number" => Ok(Scope::Number),
            "operator" => Ok(Scope::Operator),
            "property" => Ok(Scope::Property),
            "property.builtin" => Ok(Scope::PropertyBuiltin),
            "punctuation" => Ok(Scope::Punctuation),
            "punctuation.bracket" => Ok(Scope::PunctuationBracket),
            "punctuation.delimiter" => Ok(Scope::PunctuationDelimiter),
            "punctuation.special" => Ok(Scope::PunctuationSpecial),
            "string" => Ok(Scope::String),
            "string.special" => Ok(Scope::StringSpecial),
            "type" => Ok(Scope::Type),
            "type.builtin" => Ok(Scope::TypeBuiltin),
            "variable" => Ok(Scope::Variable),
            "variable.builtin" => Ok(Scope::VariableBuiltin),
            "tag" => Ok(Scope::Tag),
            _ => Ok(Scope::Unknown),
        }
    }
}

pub fn highlight<'a, T: LanguageRegistry>(
    language_registry: &'a T,
    source: &'a [u8],
    language: Language,
    property_sheet: &'a PropertySheet<Properties>,
) -> Result<impl Iterator<Item = HighlightEvent<'a>> + 'a, String> {
    Highlighter::new(language_registry, source, language, property_sheet)
}

// struct HtmlRenderer {
//     result: Vec<String>,
//     buffer: String,
// }
//
// impl HtmlRenderer {
//     fn new() -> Self {
//         HtmlRenderer {
//             result: Vec::new(),
//             buffer: String::new(),
//         }
//     }
//
//     fn start_scope(&mut self, s: &Scope) {
//         write!(&mut self.buffer, "<span class=\"{}\">", s.to_class_name()).unwrap();
//     }
//
//     fn end_scope(&mut self) {
//         write!(&mut self.buffer, "</span>").unwrap();
//     }
//
//     fn flush(&mut self) {
//         if !self.buffer.is_empty() {
//             self.buffer.push('\n');
//             self.result.push(self.buffer.clone());
//             self.buffer.clear();
//         }
//     }
//
//     fn render_line(&mut self, src: &str, scopes: &Vec<Scope>) {
//         let mut multiline = false;
//         for line in src.split('\n') {
//             let line = line.trim_end_matches('\r');
//             if multiline {
//                 scopes.iter().for_each(|_| self.end_scope());
//                 self.flush();
//                 scopes.iter().for_each(|scope| self.start_scope(scope));
//             }
//             write!(&mut self.buffer, "{}", escape::Escape(line)).unwrap();
//             multiline = true;
//         }
//     }
//
//     fn render(mut self, src: &[u8], language: &str) -> Option<Vec<String>> {
//         let mut scopes = Vec::new();
//         for event in Highlighter::new(src, language)? {
//             match event {
//                 HighlightEvent::ScopeStart(s) => {
//                     scopes.push(s);
//                     self.start_scope(&s);
//                 }
//                 HighlightEvent::ScopeEnd(s) => {
//                     assert_eq!(scopes.pop(), Some(s));
//                     self.end_scope();
//                 }
//                 HighlightEvent::Source(src) => {
//                     self.render_line(src, &scopes);
//                 }
//             };
//         }
//         self.flush();
//         Some(self.result)
//     }
// }
//
// pub fn to_html<T: AsRef<[u8]>>(src: T, language: &str) -> Option<Vec<String>> {
//     HtmlRenderer::new().render(src.as_ref(), language)
// }
//
// #[cfg(test)]
// mod tests {
//     use super::Scope::*;
//     use super::*;
//
//     pub fn to_token_vector<'a>(
//         src: &'a str,
//         language: &str,
//     ) -> Option<Vec<Vec<(&'a str, Vec<Scope>)>>> {
//         let mut lines = Vec::new();
//         let mut scopes = Vec::new();
//         let mut line = Vec::new();
//         for event in Highlighter::new(src.as_bytes(), language)? {
//             match event {
//                 HighlightEvent::ScopeStart(s) => scopes.push(s),
//                 HighlightEvent::ScopeEnd(s) => {
//                     assert_eq!(*scopes.last().unwrap(), s);
//                     scopes.pop();
//                 }
//                 HighlightEvent::Source(s) => {
//                     for (i, l) in s.lines().enumerate() {
//                         if i > 0 {
//                             lines.push(line);
//                             line = Vec::new();
//                         }
//                         if l.len() > 0 {
//                             line.push((l, scopes.clone()));
//                         }
//                     }
//                 }
//             }
//         }
//         lines.push(line);
//         Some(lines)
//     }
//
//     #[test]
//     fn test_to_html() {
//         let html = to_html(r#"a("b", 1)"#, "source.ruby").unwrap();
//         assert_eq!(html, vec![
//             "<span class=\"pl-en\">a</span><span class=\"pl-kos\">(</span><span class=\"pl-s\">&quot;b&quot;</span><span class=\"pl-kos\">,</span> <span class=\"pl-c1\">1</span><span class=\"pl-kos\">)</span>\n"
//         ]);
//     }
//
//     #[test]
//     fn test_comments_to_html() {
//         let html = to_html("# a comment", "source.ruby").unwrap();
//         assert_eq!(html, vec!["<span class=\"pl-c\"># a comment</span>\n"]);
//     }
//
//     #[test]
//     fn test_line_splitting() {
//         let lines = to_html("case foo\nwhen 0\n  render bar\nend", "source.ruby").unwrap();
//         assert_eq!(
//             lines,
//             vec![
//                 "<span class=\"pl-k\">case</span> <span class=\"pl-s1\">foo</span>\n",
//                 "<span class=\"pl-k\">when</span> <span class=\"pl-c1\">0</span>\n",
//                 "  <span class=\"pl-en\">render</span> <span class=\"pl-s1\">bar</span>\n",
//                 "<span class=\"pl-k\">end</span>\n"
//             ]
//         );
//     }
//
//     #[test]
//     fn test_injection_of_html_in_javascript() {
//         let source = vec!["const s = html `<div>${a < b}</div>`;"].join("\n");
//
//         assert_eq!(
//             &to_token_vector(&source, "source.js").unwrap(),
//             &[vec![
//                 ("const", vec![Keyword]),
//                 (" ", vec![]),
//                 ("s", vec![Variable]),
//                 (" ", vec![]),
//                 ("=", vec![Operator]),
//                 (" ", vec![]),
//                 ("html", vec![Function]),
//                 (" ", vec![]),
//                 ("`<", vec![String]),
//                 ("div", vec![String, Tag]),
//                 (">", vec![String]),
//                 ("${", vec![String, Embedded, PunctuationSpecial]),
//                 ("a", vec![String, Embedded, Variable]),
//                 (" ", vec![String, Embedded]),
//                 ("<", vec![String, Embedded, Operator]),
//                 (" ", vec![String, Embedded]),
//                 ("b", vec![String, Embedded, Variable]),
//                 ("}", vec![String, Embedded, PunctuationSpecial]),
//                 ("</", vec![String]),
//                 ("div", vec![String, Tag]),
//                 (">`", vec![String]),
//                 (";", vec![PunctuationDelimiter]),
//             ]]
//         );
//     }
//
//     #[test]
//     fn test_injection_of_javascript_in_html() {
//         let source = vec![
//             "<body>",
//             "  <script>",
//             "    const x = new Thing();",
//             "  </script>",
//             "</body>",
//         ]
//         .join("\n");
//
//         assert_eq!(
//             &to_token_vector(&source, "text.html.basic").unwrap(),
//             &[
//                 vec![("<", vec![]), ("body", vec![Tag]), (">", vec![]),],
//                 vec![("  <", vec![]), ("script", vec![Tag]), (">", vec![]),],
//                 vec![
//                     ("    ", vec![]),
//                     ("const", vec![Keyword]),
//                     (" ", vec![]),
//                     ("x", vec![Variable]),
//                     (" ", vec![]),
//                     ("=", vec![Operator]),
//                     (" ", vec![]),
//                     ("new", vec![Keyword]),
//                     (" ", vec![]),
//                     ("Thing", vec![Constructor]),
//                     ("(", vec![PunctuationBracket]),
//                     (")", vec![PunctuationBracket]),
//                     (";", vec![PunctuationDelimiter]),
//                 ],
//                 vec![("  </", vec![]), ("script", vec![Tag]), (">", vec![]),],
//                 vec![("</", vec![]), ("body", vec![Tag]), (">", vec![]),],
//             ]
//         );
//     }
//
//     #[test]
//     fn test_injection_of_html_and_ruby_in_erb() {
//         let source = vec![
//             "<ol>",
//             "  <% things.each do |thing| %>",
//             "    <li><%= thing.name %></li>",
//             "  <% end %>",
//             "</ol>",
//         ]
//         .join("\n");
//
//         assert_eq!(
//             &to_token_vector(&source, "text.html.erb").unwrap(),
//             &[
//                 vec![("<", vec![]), ("ol", vec![Tag]), (">", vec![]),],
//                 vec![
//                     ("  ", vec![]),
//                     ("<%", vec![Keyword]),
//                     (" ", vec![]),
//                     ("things", vec![Variable]),
//                     (".", vec![PunctuationDelimiter]),
//                     ("each", vec![Function]),
//                     (" ", vec![]),
//                     ("do", vec![Keyword]),
//                     (" |", vec![]),
//                     ("thing", vec![Variable]),
//                     ("| ", vec![]),
//                     ("%>", vec![Keyword]),
//                 ],
//                 vec![
//                     ("    <", vec![]),
//                     ("li", vec![Tag]),
//                     (">", vec![]),
//                     ("<%=", vec![Keyword]),
//                     (" ", vec![]),
//                     ("thing", vec![Variable]),
//                     (".", vec![PunctuationDelimiter]),
//                     ("name", vec![Function]),
//                     (" ", vec![]),
//                     ("%>", vec![Keyword]),
//                     ("</", vec![]),
//                     ("li", vec![Tag]),
//                     (">", vec![]),
//                 ],
//                 vec![
//                     ("  ", vec![]),
//                     ("<%", vec![Keyword]),
//                     (" ", vec![]),
//                     ("end", vec![Keyword]),
//                     (" ", vec![]),
//                     ("%>", vec![Keyword]),
//                 ],
//                 vec![("</", vec![]), ("ol", vec![Tag]), (">", vec![]),],
//             ]
//         );
//     }
//
//     #[test]
//     fn test_injection_of_javascript_in_erb() {
//         let source =
//             vec![" a <% ok %> <script>const x = y(<%= a(:b => nil) %>)</script>"].join("\n");
//
//         assert_eq!(
//             &to_token_vector(&source, "text.html.erb").unwrap(),
//             &[vec![
//                 (" a ", vec![]),
//                 ("<%", vec![Keyword]),
//                 (" ", vec![]),
//                 ("ok", vec![Variable]),
//                 (" ", vec![]),
//                 ("%>", vec![Keyword]),
//                 (" <", vec![]),
//                 ("script", vec![Tag]),
//                 (">", vec![]),
//                 ("const", vec![Keyword]),
//                 (" ", vec![]),
//                 ("x", vec![Variable]),
//                 (" ", vec![]),
//                 ("=", vec![Operator]),
//                 (" ", vec![]),
//                 ("y", vec![Function]),
//                 ("(", vec![PunctuationBracket]),
//                 ("<%=", vec![Keyword]),
//                 (" ", vec![]),
//                 ("a", vec![Function]),
//                 ("(", vec![PunctuationBracket]),
//                 (":b", vec![StringSpecial]),
//                 (" ", vec![]),
//                 ("=>", vec![Operator]),
//                 (" ", vec![]),
//                 ("nil", vec![ConstantBuiltin]),
//                 (")", vec![PunctuationBracket]),
//                 (" ", vec![]),
//                 ("%>", vec![Keyword]),
//                 (")", vec![PunctuationBracket]),
//                 ("</", vec![]),
//                 ("script", vec![Tag]),
//                 (">", vec![]),
//             ],]
//         );
//     }
// }
