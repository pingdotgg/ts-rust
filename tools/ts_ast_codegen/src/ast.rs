#![allow(dead_code)]

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fmt::Write as _;
use std::path::Path;

use crate::{
    FieldDef, ListKind, MemberDef, NodeAlias, NodeDef, Schema, StringOrList, parse_schema,
    rust_type_name, snake_case,
};

#[derive(Clone, Debug)]
#[allow(clippy::struct_excessive_bools)] // Retains the independent upstream field attributes.
struct ResolvedField {
    schema_name: String,
    ty: StringOrList,
    optional: bool,
    list: Option<ListKind>,
    go_only: bool,
    no_go: bool,
    no_factory: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ChildStorage {
    Node,
    OptionalNode,
    NodeList,
    OptionalNodeList,
    ModifierList,
    OptionalModifierList,
    RawNodes,
    OptionalRawNodes,
}

struct AstGenerator<'a> {
    schema: &'a Schema,
    node_names: HashSet<&'a str>,
    base_names: HashSet<&'a str>,
    node_alias_names: HashSet<&'a str>,
    list_alias_names: HashSet<&'a str>,
    instantiation_alias_names: HashSet<&'a str>,
    syntax_node_names: HashSet<&'a str>,
    syntax_kind_names: HashSet<&'a str>,
    kind_alias_names: HashSet<&'a str>,
}

/// Generate arena-oriented Rust AST foundations for every schema node.
///
/// # Errors
///
/// Returns an error when the input is invalid or contains unresolved base,
/// member, alias, or type references.
pub fn generate_ast(json: &str) -> Result<String, String> {
    let schema = parse_schema(json)?;
    AstGenerator::new(&schema)?.generate()
}

/// Read an `ast.json` file and generate arena-oriented Rust AST foundations.
///
/// # Errors
///
/// Returns an error when the file cannot be read or generated.
pub fn generate_ast_file(path: &Path) -> Result<String, String> {
    let json = std::fs::read_to_string(path)
        .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
    generate_ast(&json)
}

impl<'a> AstGenerator<'a> {
    fn new(schema: &'a Schema) -> Result<Self, String> {
        let generator = Self {
            schema,
            node_names: schema
                .nodes
                .definitions
                .keys()
                .map(String::as_str)
                .collect(),
            base_names: schema.bases.keys().map(String::as_str).collect(),
            node_alias_names: schema.nodes.aliases.keys().map(String::as_str).collect(),
            list_alias_names: schema
                .nodes
                .list_aliases
                .keys()
                .map(String::as_str)
                .collect(),
            instantiation_alias_names: schema
                .nodes
                .definitions
                .values()
                .flat_map(|node| node.instantiation_aliases.keys())
                .map(String::as_str)
                .collect(),
            syntax_node_names: schema
                .nodes
                .definitions
                .iter()
                .flat_map(|(name, node)| syntax_node_names(name, node))
                .collect(),
            syntax_kind_names: schema
                .kinds
                .elements
                .iter()
                .filter_map(|element| match element {
                    crate::KindElement::Name(name) => Some(name.as_str()),
                    crate::KindElement::Detailed { name, .. } => name.as_deref(),
                })
                .collect(),
            kind_alias_names: schema.kinds.aliases.keys().map(String::as_str).collect(),
        };
        generator.validate()?;
        Ok(generator)
    }

    fn validate(&self) -> Result<(), String> {
        for (name, base) in &self.schema.bases {
            self.validate_extends(name, &base.extends)?;
            self.collect_base_fields(name, &mut Vec::new())?;
            for (field_name, field) in &base.fields {
                self.rust_type(&field.r#type, field.list, field.optional)
                    .map_err(|error| format!("{name}.{field_name}: {error}"))?;
            }
        }
        for (name, node) in &self.schema.nodes.definitions {
            self.validate_extends(name, &node.extends)?;
            self.validate_node_kinds(name, node)?;
            let fields = self.node_fields(name, node)?;
            let mut rust_fields = HashSet::new();
            for field in &fields {
                let rust_name = rust_field_name(&field.schema_name);
                if !rust_fields.insert(rust_name.clone()) {
                    return Err(format!("{name} has duplicate Rust field {rust_name}"));
                }
                self.rust_type(&field.ty, field.list, field.optional)
                    .map_err(|error| format!("{name}.{}: {error}", field.schema_name))?;
            }
            if node.hand_written && name != "SourceFile" {
                return Err(format!("unsupported handwritten node {name}"));
            }
            if node.hand_written_visitor && name != "JSDocParameterOrPropertyTag" {
                return Err(format!("unsupported handwritten visitor for {name}"));
            }
            self.child_fields(name, node)?;
        }
        for (name, alias) in &self.schema.nodes.aliases {
            match alias {
                NodeAlias::Base { base } if !self.base_names.contains(base.as_str()) => {
                    return Err(format!("node alias {name} has unknown base {base}"));
                }
                NodeAlias::Members(members) => {
                    for member in members {
                        if !self.is_node_type(member)
                            && !self.kind_alias_names.contains(member.as_str())
                        {
                            return Err(format!("node alias {name} has unknown member {member}"));
                        }
                    }
                }
                NodeAlias::Base { .. } => {}
            }
        }
        for (name, element) in &self.schema.nodes.list_aliases {
            if !self.is_node_type(element) {
                return Err(format!("list alias {name} has unknown element {element}"));
            }
        }
        Ok(())
    }

    fn validate_node_kinds(&self, name: &str, node: &NodeDef) -> Result<(), String> {
        let kind_member = node
            .members
            .iter()
            .find(|member| matches!(member.name.as_str(), "Kind" | "kind"));
        if let Some(kind) = &node.kind {
            for kind_name in type_names(kind) {
                self.validate_kind_reference(name, kind_name)?;
            }
        } else if node.type_parameters.is_empty()
            && kind_member.is_none()
            && !self.syntax_kind_names.contains(name)
        {
            return Err(format!("node {name} has no matching SyntaxKind"));
        }
        for parameter in &node.type_parameters {
            self.validate_kind_reference(name, &parameter.constraint)?;
        }
        for kind in node.instantiation_aliases.values() {
            self.validate_kind_reference(name, kind)?;
        }
        if let Some(member) = kind_member
            && let Some(kind) = &member.r#type
        {
            for kind_name in type_names(kind) {
                if !node
                    .type_parameters
                    .iter()
                    .any(|parameter| parameter.name == kind_name)
                {
                    self.validate_kind_reference(name, kind_name)?;
                }
            }
        }
        Ok(())
    }

    fn validate_kind_reference(&self, owner: &str, name: &str) -> Result<(), String> {
        let name = strip_syntax_kind(name);
        if self.syntax_kind_names.contains(name) || self.kind_alias_names.contains(name) {
            Ok(())
        } else {
            Err(format!("{owner} references unknown SyntaxKind {name}"))
        }
    }

    fn validate_extends(&self, name: &str, extends: &[String]) -> Result<(), String> {
        for base in extends {
            if !self.base_names.contains(base.as_str()) {
                return Err(format!("{name} extends unknown base {base}"));
            }
        }
        Ok(())
    }

    fn collect_base_fields(
        &self,
        name: &str,
        stack: &mut Vec<String>,
    ) -> Result<BTreeMap<String, ResolvedField>, String> {
        if stack.iter().any(|item| item == name) {
            return Err(format!("cyclic base inheritance involving {name}"));
        }
        let base = self
            .schema
            .bases
            .get(name)
            .ok_or_else(|| format!("unknown base {name}"))?;
        stack.push(name.to_owned());
        let mut fields = BTreeMap::new();
        for parent in &base.extends {
            for (field_name, field) in self.collect_base_fields(parent, stack)? {
                Self::merge_field(name, &mut fields, field_name, field)?;
            }
        }
        stack.pop();
        for (field_name, field) in &base.fields {
            // `noGo` fields describe TypeScript-only overrides. They do not
            // replace an inherited runtime field with the same name.
            if field.no_go {
                continue;
            }
            let resolved = resolved_base_field(field_name, field);
            fields.insert(field_name.clone(), resolved);
        }
        Ok(fields)
    }

    fn merge_field(
        owner: &str,
        fields: &mut BTreeMap<String, ResolvedField>,
        name: String,
        field: ResolvedField,
    ) -> Result<(), String> {
        if let Some(previous) = fields.get(&name) {
            if format!("{:?}", previous.ty) != format!("{:?}", field.ty)
                || previous.list.map(|kind| kind as u8) != field.list.map(|kind| kind as u8)
            {
                return Err(format!("conflicting inherited field {owner}.{name}"));
            }
            return Ok(());
        }
        fields.insert(name, field);
        Ok(())
    }

    fn node_fields(&self, name: &str, node: &NodeDef) -> Result<Vec<ResolvedField>, String> {
        let mut fields = BTreeMap::new();
        for base in &node.extends {
            for (field_name, field) in self.collect_base_fields(base, &mut Vec::new())? {
                Self::merge_field(name, &mut fields, field_name, field)?;
            }
        }
        for member in &node.members {
            if is_header_field(&member.name) {
                continue;
            }
            if member.inherited {
                Self::apply_inherited_member(name, &mut fields, member)?;
            } else {
                let ty = member
                    .r#type
                    .clone()
                    .ok_or_else(|| format!("{name}.{} has no type", member.name))?;
                fields.insert(
                    member.name.clone(),
                    ResolvedField {
                        schema_name: member.name.clone(),
                        ty,
                        optional: member.optional.unwrap_or(false)
                            || rust_nullable_schema_field(name, &member.name),
                        list: member.list,
                        go_only: member.go_only,
                        no_go: member.no_go,
                        no_factory: member.no_factory,
                    },
                );
            }
        }
        fields.remove("Flags");
        // astshrink1: the `goOnly` fields are the binder fields (`Symbol`,
        // `LocalSymbol`, `Locals`, `NextContainer`, the flow nodes and
        // `facts`). The port keeps that data in the store records, the bind
        // extras and `subtree_facts`, so the node data has no room for it.
        Ok(fields
            .into_values()
            .filter(|field| !field.no_go && !field.go_only)
            .collect())
    }

    fn apply_inherited_member(
        node_name: &str,
        fields: &mut BTreeMap<String, ResolvedField>,
        member: &MemberDef,
    ) -> Result<(), String> {
        let Some(field) = fields.get_mut(&member.name) else {
            return Err(format!(
                "{node_name}.{} does not resolve to an inherited base field",
                member.name
            ));
        };
        if let Some(ty) = &member.r#type {
            field.ty.clone_from(ty);
        }
        if let Some(optional) = member.optional {
            field.optional = optional;
        }
        if let Some(list) = member.list {
            field.list = Some(list);
        }
        field.go_only |= member.go_only;
        field.no_go |= member.no_go;
        field.no_factory |= member.no_factory;
        Ok(())
    }

    fn child_fields(&self, name: &str, node: &NodeDef) -> Result<Vec<ResolvedField>, String> {
        let fields = self.node_fields(name, node)?;
        let mut children = Vec::new();
        for member in &node.members {
            if is_header_field(&member.name) {
                continue;
            }
            let Some(field) = fields.iter().find(|field| field.schema_name == member.name) else {
                if member.no_go || member.go_only {
                    continue;
                }
                return Err(format!(
                    "{name}.{} is missing from the generated Rust fields",
                    member.name
                ));
            };
            if field.go_only || field.no_factory {
                continue;
            }
            if self.child_storage(field)?.is_some() {
                children.push(field.clone());
            }
        }
        Ok(children)
    }

    fn child_storage(&self, field: &ResolvedField) -> Result<Option<ChildStorage>, String> {
        let scalar = self.scalar_rust_type(&field.ty)?;
        let is_node = matches!(scalar.as_str(), "NodeId" | "Option<NodeId>");
        if !is_node {
            return Ok(None);
        }
        let optional = field.optional || scalar == "Option<NodeId>";
        let storage = match (field.list, optional) {
            (None, false) => ChildStorage::Node,
            (None, true) => ChildStorage::OptionalNode,
            (Some(ListKind::NodeList), false) => ChildStorage::NodeList,
            (Some(ListKind::NodeList), true) => ChildStorage::OptionalNodeList,
            (Some(ListKind::ModifierList), false) => ChildStorage::ModifierList,
            (Some(ListKind::ModifierList), true) => ChildStorage::OptionalModifierList,
            (Some(ListKind::Raw), false) => ChildStorage::RawNodes,
            (Some(ListKind::Raw), true) => ChildStorage::OptionalRawNodes,
        };
        Ok(Some(storage))
    }

    fn rust_type(
        &self,
        ty: &StringOrList,
        list: Option<ListKind>,
        optional: bool,
    ) -> Result<String, String> {
        let base = if let Some(list) = list {
            match list {
                ListKind::NodeList => "NodeList".to_owned(),
                ListKind::ModifierList => "ModifierList".to_owned(),
                ListKind::Raw => format!("Vec<{}>", self.scalar_rust_type(ty)?),
            }
        } else {
            self.scalar_rust_type(ty)?
        };
        if optional && !base.starts_with("Option<") {
            Ok(format!("Option<{base}>"))
        } else {
            Ok(base)
        }
    }

    fn scalar_rust_type(&self, ty: &StringOrList) -> Result<String, String> {
        match ty {
            StringOrList::One(name) => self.named_rust_type(name),
            StringOrList::Many(names) => {
                let mapped: BTreeSet<_> = names
                    .iter()
                    .map(|name| self.named_rust_type(name))
                    .collect::<Result<_, _>>()?;
                if mapped.len() == 1 {
                    Ok(mapped.into_iter().next().unwrap())
                } else {
                    Err(format!("union has incompatible Rust types: {mapped:?}"))
                }
            }
        }
    }

    fn named_rust_type(&self, name: &str) -> Result<String, String> {
        let ty = match name {
            "bool" | "boolean" => "bool",
            "int" => "i32",
            "string" => "String",
            "NodeFlags" => "NodeFlags",
            "TokenFlags" => "TokenFlags",
            "ModifierFlags" => "ModifierFlags",
            "SymbolTable" => "SymbolTable",
            "atomic.Uint32" => "u32",
            "any" => "OpaqueValue",
            "*Node" => "Option<NodeId>",
            "*Symbol" => "Option<SymbolId>",
            "*FlowNode" => "Option<FlowNodeId>",
            "Kind" => "SyntaxKind",
            _ if name.starts_with("SyntaxKind.") => "SyntaxKind",
            _ if self.kind_alias_names.contains(name) => "SyntaxKind",
            _ if self.list_alias_names.contains(name) => "NodeList",
            _ if self.is_node_type(name) => "NodeId",
            _ => return Err(format!("unresolved schema type {name}")),
        };
        Ok(ty.to_owned())
    }

    fn is_node_type(&self, name: &str) -> bool {
        name == "Node"
            || self.node_names.contains(name)
            || self.base_names.contains(name)
            || self.node_alias_names.contains(name)
            || self.instantiation_alias_names.contains(name)
            || self.syntax_node_names.contains(name)
    }

    fn generate_child_traversal(&self, output: &mut String) -> Result<(), String> {
        writeln!(output).unwrap();
        writeln!(
            output,
            "    /// Visits direct AST children in TypeScript-Go `ForEachChild` order."
        )
        .unwrap();
        writeln!(output, "    ///").unwrap();
        writeln!(
            output,
            "    /// Returning [`std::ops::ControlFlow::Break`] stops traversal immediately."
        )
        .unwrap();
        writeln!(
            output,
            "    #[allow(clippy::match_same_arms, clippy::too_many_lines)]"
        )
        .unwrap();
        writeln!(output, "    pub fn try_for_each_child<B, F>(").unwrap();
        writeln!(output, "        self,").unwrap();
        writeln!(output, "        visitor: &mut F,").unwrap();
        writeln!(output, "    ) -> std::ops::ControlFlow<B>").unwrap();
        writeln!(output, "    where").unwrap();
        writeln!(
            output,
            "        F: FnMut(Child) -> std::ops::ControlFlow<B> + ?Sized,"
        )
        .unwrap();
        writeln!(output, "    {{").unwrap();
        writeln!(
            output,
            "        // BEGIN schema-exhaustive direct child visitor"
        )
        .unwrap();
        writeln!(output, "        match self {{").unwrap();
        for (name, node) in &self.schema.nodes.definitions {
            let rust_name = rust_type_name(name);
            if node.hand_written_visitor {
                Self::generate_hand_written_child_arm(output, name, &rust_name)?;
                continue;
            }
            let fields = self.child_fields(name, node)?;
            if fields.is_empty() {
                writeln!(
                    output,
                    "            Self::{rust_name}(..) => std::ops::ControlFlow::Continue(()),"
                )
                .unwrap();
                continue;
            }
            writeln!(output, "            Self::{rust_name}(node) => {{").unwrap();
            for (index, field) in fields.iter().enumerate() {
                let call = self.child_visit_call(field)?;
                if index + 1 == fields.len() {
                    writeln!(output, "                {call}").unwrap();
                } else {
                    writeln!(output, "                {call}?;").unwrap();
                }
            }
            writeln!(output, "            }}").unwrap();
        }
        writeln!(output, "        }}").unwrap();
        writeln!(
            output,
            "        // END schema-exhaustive direct child visitor"
        )
        .unwrap();
        writeln!(output, "    }}").unwrap();
        Ok(())
    }

    /// `NodeData::for_each_list`: every list field of the data, also the
    /// ones that `try_for_each_child` does not visit. A print pack copies
    /// them all (astshrink1).
    fn generate_list_walk(&self, output: &mut String) -> Result<(), String> {
        output.push_str(
            "
    /// Visits every list field of the data, also the ones that
    /// `try_for_each_child` skips, as Go `ForEachChild` does.
    #[allow(clippy::too_many_lines)]
    pub fn for_each_list(self, mut visitor: impl FnMut(NodeList)) {
        let mut visit = |list: Option<NodeList>| {
            if let Some(list) = list {
                visitor(list);
            }
        };
        match self {
",
        );
        for (name, node) in &self.schema.nodes.definitions {
            let mut visits = Vec::new();
            for field in self.node_fields(name, node)? {
                let f = rust_field_name(&field.schema_name);
                let ty = self.rust_type(&field.ty, field.list, field.optional)?;
                visits.push(match ty.as_str() {
                    "NodeList" => format!("visit(Some(node.{f}))"),
                    "Option<NodeList>" => format!("visit(node.{f})"),
                    "ModifierList" => format!("visit(Some(node.{f}.list))"),
                    "Option<ModifierList>" => format!("visit(node.{f}.map(|m| m.list))"),
                    _ => continue,
                });
            }
            let rust_name = rust_type_name(name);
            match visits.as_slice() {
                [] => {}
                [visit] => {
                    writeln!(output, "            Self::{rust_name}(node) => {visit},").unwrap();
                }
                _ => {
                    let visits = visits.join("; ");
                    writeln!(
                        output,
                        "            Self::{rust_name}(node) => {{ {visits}; }}"
                    )
                    .unwrap();
                }
            }
        }
        output.push_str("            _ => {}\n        }\n    }\n");
        Ok(())
    }

    /// `BoxedData`, the owning data (a box per payload), and `NodeData<'a>`,
    /// the borrowed view that readers match (Go's `nodeData` interface word:
    /// a tag and a pointer, 16 B). Both are `#[repr(u8)]` with one variant
    /// order, so `BoxedData::view` is a copy (astchunks2 probe).
    fn generate_data_enums(&self, output: &mut String) {
        output.push_str(
            "/// The owning node data: one box per payload. Owned file versions (LSP)
/// and synthetic nodes keep their data in it. Readers match its `view`.
#[derive(Clone, Debug)]
#[repr(u8)]
pub enum BoxedData {
",
        );
        for name in self.schema.nodes.definitions.keys() {
            let rust_name = rust_type_name(name);
            writeln!(output, "    {rust_name}(Box<{rust_name}Data>),").unwrap();
        }
        output.push_str(
            "}

/// The node data that readers match: a tag and a reference (16 B), as Go's
/// `nodeData` interface word. Same tags and order as `BoxedData`.
#[derive(Clone, Copy, Debug)]
#[repr(u8)]
pub enum NodeData<'a> {
",
        );
        for name in self.schema.nodes.definitions.keys() {
            let rust_name = rust_type_name(name);
            writeln!(output, "    {rust_name}(&'a {rust_name}Data),").unwrap();
        }
        output.push_str(
            "}

impl BoxedData {
    /// The view of the data. A 16 B copy: the two enums have one layout.
    #[inline]
    #[must_use]
    #[allow(clippy::too_many_lines)]
    pub fn view(&self) -> NodeData<'_> {
        match self {
",
        );
        for name in self.schema.nodes.definitions.keys() {
            let rust_name = rust_type_name(name);
            writeln!(
                output,
                "            Self::{rust_name}(d) => NodeData::{rust_name}(d),"
            )
            .unwrap();
        }
        output.push_str("        }\n    }\n}\n\n");
    }

    /// `NodeData::to_boxed`, `NodeData::addr` and `NodeData::same`.
    fn generate_view_methods(&self, output: &mut String) {
        output.push_str(
            "
    /// An owning copy of the data in a new box.
    #[must_use]
    #[allow(clippy::too_many_lines)]
    pub fn to_boxed(self) -> BoxedData {
        match self {
",
        );
        for name in self.schema.nodes.definitions.keys() {
            let rust_name = rust_type_name(name);
            writeln!(
                output,
                "            Self::{rust_name}(d) => BoxedData::{rust_name}(Box::new(d.clone())),"
            )
            .unwrap();
        }
        output.push_str(
            "        }
    }

    /// The address of the data. Data of a zero-size type can share an
    /// address with other data: `same` also compares the tag.
    #[must_use]
    #[allow(clippy::too_many_lines)]
    pub fn addr(self) -> *const () {
        match self {
",
        );
        for name in self.schema.nodes.definitions.keys() {
            let rust_name = rust_type_name(name);
            writeln!(
                output,
                "            Self::{rust_name}(d) => std::ptr::from_ref(d).cast(),"
            )
            .unwrap();
        }
        output.push_str(
            "        }
    }

    /// Whether `self` and `other` view the same data: same tag and address.
    #[must_use]
    pub fn same(self, other: Self) -> bool {
        std::mem::discriminant(&self) == std::mem::discriminant(&other)
            && std::ptr::eq(self.addr(), other.addr())
    }
",
        );
    }

    /// `AstData`: the view and the box of one data struct (the factory's
    /// `new_node<T: AstData>`).
    fn generate_ast_data_impls(&self, output: &mut String) {
        output.push_str(
            "/// One node data struct: its view and its box.
pub trait AstData: Sized + 'static {
    /// The view of this data.
    fn view(&self) -> NodeData<'_>;
    /// This data in a new box.
    fn into_boxed(self) -> BoxedData;
}

macro_rules! ast_data {
    ($($v:ident($t:ident)),+ $(,)?) => {$(
        impl AstData for $t {
            #[inline]
            fn view(&self) -> NodeData<'_> {
                NodeData::$v(self)
            }

            #[inline]
            fn into_boxed(self) -> BoxedData {
                BoxedData::$v(Box::new(self))
            }
        }
    )+};
}

ast_data! {
",
        );
        for name in self.schema.nodes.definitions.keys() {
            let rust_name = rust_type_name(name);
            writeln!(output, "    {rust_name}({rust_name}Data),").unwrap();
        }
        output.push_str("}\n\n");
    }

    fn generate_hand_written_child_arm(
        output: &mut String,
        schema_name: &str,
        rust_name: &str,
    ) -> Result<(), String> {
        if schema_name != "JSDocParameterOrPropertyTag" {
            return Err(format!("unsupported handwritten visitor for {schema_name}"));
        }
        writeln!(output, "            Self::{rust_name}(node) => {{").unwrap();
        writeln!(
            output,
            "                visitor(Child::Node(node.tag_name))?;"
        )
        .unwrap();
        writeln!(output, "                if node.is_name_first {{").unwrap();
        writeln!(
            output,
            "                    visitor(Child::Node(node.name))?;"
        )
        .unwrap();
        writeln!(
            output,
            "                    visit_optional_child(visitor, node.type_expression)?;"
        )
        .unwrap();
        writeln!(output, "                }} else {{").unwrap();
        writeln!(
            output,
            "                    visit_optional_child(visitor, node.type_expression)?;"
        )
        .unwrap();
        writeln!(
            output,
            "                    visitor(Child::Node(node.name))?;"
        )
        .unwrap();
        writeln!(output, "                }}").unwrap();
        writeln!(
            output,
            "                visit_optional_node_list(visitor, node.comment)"
        )
        .unwrap();
        writeln!(output, "            }}").unwrap();
        Ok(())
    }

    fn child_visit_call(&self, field: &ResolvedField) -> Result<String, String> {
        let field_name = rust_field_name(&field.schema_name);
        let call = match self
            .child_storage(field)?
            .ok_or_else(|| format!("{} is not an AST child", field.schema_name))?
        {
            ChildStorage::Node => format!("visitor(Child::Node(node.{field_name}))"),
            ChildStorage::OptionalNode => {
                format!("visit_optional_child(visitor, node.{field_name})")
            }
            ChildStorage::NodeList => format!("visitor(Child::List(node.{field_name}))"),
            ChildStorage::OptionalNodeList => {
                format!("visit_optional_node_list(visitor, node.{field_name})")
            }
            ChildStorage::ModifierList => {
                format!("visitor(Child::List(node.{field_name}.list))")
            }
            ChildStorage::OptionalModifierList => {
                format!("visit_optional_modifier_list(visitor, node.{field_name})")
            }
            ChildStorage::RawNodes => format!("visit_children(visitor, &node.{field_name})"),
            ChildStorage::OptionalRawNodes => {
                format!("visit_optional_children(visitor, node.{field_name}.as_deref())")
            }
        };
        Ok(call)
    }

    #[allow(clippy::too_many_lines)]
    fn generate(&self) -> Result<String, String> {
        let mut output = String::new();
        writeln!(
            output,
            "// Code generated by tools/ts_ast_codegen. DO NOT EDIT."
        )
        .unwrap();
        writeln!(output).unwrap();
        writeln!(output, "use super::SyntaxKind;").unwrap();
        writeln!(output, "use super::text::TextRange;").unwrap();
        writeln!(output).unwrap();
        write_foundations(&mut output);

        for (name, node) in &self.schema.nodes.definitions {
            let rust_name = rust_type_name(name);
            let fields = self.node_fields(name, node)?;
            writeln!(output, "#[derive(Clone, Debug)]").unwrap();
            if fields.is_empty() {
                writeln!(output, "pub struct {rust_name}Data;").unwrap();
            } else {
                writeln!(output, "pub struct {rust_name}Data {{").unwrap();
                for field in fields {
                    writeln!(
                        output,
                        "    pub {}: {},",
                        rust_field_name(&field.schema_name),
                        self.rust_type(&field.ty, field.list, field.optional)?
                    )
                    .unwrap();
                }
                writeln!(output, "}}").unwrap();
            }
            writeln!(output).unwrap();
        }

        self.generate_data_enums(&mut output);
        writeln!(output, "impl<'a> NodeData<'a> {{").unwrap();
        writeln!(
            output,
            "    pub const SCHEMA_NODE_COUNT: usize = {};",
            self.schema.nodes.definitions.len()
        )
        .unwrap();
        writeln!(
            output,
            "    pub const SCHEMA_BASE_COUNT: usize = {};",
            self.schema.bases.len()
        )
        .unwrap();
        writeln!(
            output,
            "    pub const SCHEMA_NODE_ALIAS_COUNT: usize = {};",
            self.schema.nodes.aliases.len()
        )
        .unwrap();
        writeln!(
            output,
            "    pub const SCHEMA_LIST_ALIAS_COUNT: usize = {};",
            self.schema.nodes.list_aliases.len()
        )
        .unwrap();
        writeln!(output).unwrap();
        writeln!(output, "    #[must_use]").unwrap();
        writeln!(output, "    #[allow(clippy::too_many_lines)]").unwrap();
        writeln!(
            output,
            "    pub const fn schema_name(self) -> &'static str {{"
        )
        .unwrap();
        writeln!(output, "        match self {{").unwrap();
        for name in self.schema.nodes.definitions.keys() {
            let rust_name = rust_type_name(name);
            writeln!(output, "            Self::{rust_name}(..) => \"{name}\",").unwrap();
        }
        writeln!(output, "        }}").unwrap();
        writeln!(output, "    }}").unwrap();
        self.generate_child_traversal(&mut output)?;
        self.generate_list_walk(&mut output)?;
        self.generate_view_methods(&mut output);
        writeln!(output, "}}").unwrap();
        writeln!(output).unwrap();
        self.generate_ast_data_impls(&mut output);
        writeln!(output).unwrap();
        write_child_traversal_helpers(&mut output);

        for name in self.schema.nodes.definitions.keys() {
            writeln!(output, "pub type {}Node = NodeId;", rust_type_name(name)).unwrap();
        }
        for name in self.schema.nodes.aliases.keys() {
            writeln!(output, "pub type {} = NodeId;", rust_type_name(name)).unwrap();
        }
        for name in self.schema.nodes.list_aliases.keys() {
            writeln!(output, "pub type {} = NodeList;", rust_type_name(name)).unwrap();
        }
        Ok(output)
    }
}

fn syntax_node_names<'a>(node_name: &'a str, node: &'a NodeDef) -> Vec<&'a str> {
    let mut names = Vec::new();
    if let Some(kind) = &node.kind {
        names.extend(type_names(kind).map(strip_syntax_kind));
    } else {
        names.push(node_name);
    }
    if let Some(kind_member) = node
        .members
        .iter()
        .find(|member| matches!(member.name.as_str(), "Kind" | "kind"))
        && let Some(kind) = &kind_member.r#type
    {
        names.extend(type_names(kind).map(strip_syntax_kind));
    }
    names
}

fn write_child_traversal_helpers(output: &mut String) {
    output.push_str(
        r"fn visit_optional_child<B, F>(
    visitor: &mut F,
    child: Option<NodeId>,
) -> std::ops::ControlFlow<B>
where
    F: FnMut(Child) -> std::ops::ControlFlow<B> + ?Sized,
{
    match child {
        Some(child) => visitor(Child::Node(child)),
        None => std::ops::ControlFlow::Continue(()),
    }
}

fn visit_children<B, F>(visitor: &mut F, children: &[NodeId]) -> std::ops::ControlFlow<B>
where
    F: FnMut(Child) -> std::ops::ControlFlow<B> + ?Sized,
{
    for &child in children {
        visitor(Child::Node(child))?;
    }
    std::ops::ControlFlow::Continue(())
}

fn visit_optional_children<B, F>(
    visitor: &mut F,
    children: Option<&[NodeId]>,
) -> std::ops::ControlFlow<B>
where
    F: FnMut(Child) -> std::ops::ControlFlow<B> + ?Sized,
{
    match children {
        Some(children) => visit_children(visitor, children),
        None => std::ops::ControlFlow::Continue(()),
    }
}

fn visit_optional_node_list<B, F>(
    visitor: &mut F,
    list: Option<NodeList>,
) -> std::ops::ControlFlow<B>
where
    F: FnMut(Child) -> std::ops::ControlFlow<B> + ?Sized,
{
    match list {
        Some(list) => visitor(Child::List(list)),
        None => std::ops::ControlFlow::Continue(()),
    }
}

fn visit_optional_modifier_list<B, F>(
    visitor: &mut F,
    list: Option<ModifierList>,
) -> std::ops::ControlFlow<B>
where
    F: FnMut(Child) -> std::ops::ControlFlow<B> + ?Sized,
{
    match list {
        Some(list) => visitor(Child::List(list.list)),
        None => std::ops::ControlFlow::Continue(()),
    }
}

impl Node {
    /// Visits every direct AST child in TypeScript-Go `ForEachChild` order.
    pub fn for_each_child(&self, mut visitor: impl FnMut(Child)) {
        let _ = self.data.view().try_for_each_child(&mut |child| {
            visitor(child);
            std::ops::ControlFlow::<()>::Continue(())
        });
    }

    /// Tries to visit direct AST children in TypeScript-Go `ForEachChild` order.
    ///
    /// Returning [`std::ops::ControlFlow::Break`] stops traversal immediately.
    pub fn try_for_each_child<B>(
        &self,
        mut visitor: impl FnMut(Child) -> std::ops::ControlFlow<B>,
    ) -> std::ops::ControlFlow<B> {
        self.data.view().try_for_each_child(&mut visitor)
    }
}

",
    );
}

fn type_names(ty: &StringOrList) -> impl Iterator<Item = &str> {
    match ty {
        StringOrList::One(name) => std::slice::from_ref(name).iter(),
        StringOrList::Many(names) => names.iter(),
    }
    .map(String::as_str)
}

fn strip_syntax_kind(name: &str) -> &str {
    name.strip_prefix("SyntaxKind.").unwrap_or(name)
}

fn resolved_base_field(name: &str, field: &FieldDef) -> ResolvedField {
    ResolvedField {
        schema_name: name.to_owned(),
        ty: field.r#type.clone(),
        optional: field.optional,
        list: field.list,
        go_only: field.go_only,
        no_go: field.no_go,
        no_factory: field.no_factory,
    }
}

/// Upstream's pointer-backed Go model permits a small number of parser-owned
/// nil slots that are not marked optional in `ast.json`. Preserve the exact
/// runtime contract when choosing a Rust storage type without modifying the
/// vendored schema snapshot.
fn rust_nullable_schema_field(node: &str, field: &str) -> bool {
    matches!((node, field), ("ExportAssignment", "Type"))
}

fn is_header_field(name: &str) -> bool {
    matches!(name, "Kind" | "kind" | "Flags")
}

fn rust_field_name(name: &str) -> String {
    let name = snake_case(name);
    if matches!(
        name.as_str(),
        "as" | "async"
            | "await"
            | "break"
            | "const"
            | "continue"
            | "crate"
            | "dyn"
            | "else"
            | "enum"
            | "extern"
            | "false"
            | "fn"
            | "for"
            | "if"
            | "impl"
            | "in"
            | "let"
            | "loop"
            | "match"
            | "mod"
            | "move"
            | "mut"
            | "pub"
            | "ref"
            | "return"
            | "self"
            | "static"
            | "struct"
            | "super"
            | "trait"
            | "true"
            | "type"
            | "union"
            | "unsafe"
            | "use"
            | "where"
            | "while"
            | "yield"
    ) {
        format!("{name}_")
    } else {
        name
    }
}

#[allow(clippy::too_many_lines)] // One generated foundation block.
fn write_foundations(output: &mut String) {
    output.push_str(
        r#"#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct NodeId(u32);

impl NodeId {
    #[must_use]
    pub const fn new(index: u32) -> Self { Self(index) }

    #[must_use]
    pub const fn index(self) -> usize { self.0 as usize }
}

/// Opaque identity for one node arena allocation.
///
/// Moving an arena preserves its identity. Cloning creates an independent
/// arena with a fresh identity so binding provenance cannot cross clones.
#[derive(Clone, Copy, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct NodeArenaId(std::num::NonZeroU64);

impl std::fmt::Debug for NodeArenaId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("NodeArenaId")
    }
}

static LAST_NODE_ARENA_ID: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

pub(crate) fn allocate_node_arena_id_from(
    counter: &std::sync::atomic::AtomicU64,
) -> NodeArenaId {
    let previous = counter
        .fetch_update(
            std::sync::atomic::Ordering::Relaxed,
            std::sync::atomic::Ordering::Relaxed,
            |current| current.checked_add(1),
        )
        .unwrap_or_else(|_| panic!("AST node arena identity space exhausted"));
    NodeArenaId(
        std::num::NonZeroU64::new(previous + 1)
            .expect("allocated AST node arena identities are nonzero"),
    )
}

fn allocate_node_arena_id() -> NodeArenaId {
    allocate_node_arena_id_from(&LAST_NODE_ARENA_ID)
}

/// Opaque mutation revision for one node arena identity.
///
/// Revisions are comparable only for snapshots of the same [`NodeArenaId`].
/// A new or cloned arena starts at revision zero.
#[derive(Clone, Copy, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct NodeArenaRevision(u64);

impl std::fmt::Debug for NodeArenaRevision {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("NodeArenaRevision")
    }
}

impl NodeArenaRevision {
    const INITIAL: Self = Self(0);

    fn checked_next(self) -> Option<Self> {
        self.0.checked_add(1).map(Self)
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct SymbolId(pub u32);

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct FlowNodeId(pub u32);

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct NodeFlags(pub u32);

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct TokenFlags(pub u32);

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct ModifierFlags(pub u32);

#[derive(Clone, Debug, Default)]
pub struct SymbolTable;

#[derive(Clone, Debug, Default)]
pub struct OpaqueValue;

/// A list field of node data: entry `index()` of the list table of the owner
/// of the data (a file store, or the synthetic arena of a thread). The entry
/// holds the Go `Loc`, the node ids, the missing-list bit and, for a modifier
/// list, Go `ModifiersToFlags`. Go stores a `*NodeList`.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct NodeList(std::num::NonZeroU32);

impl NodeList {
    /// The list field of table entry `index`.
    #[must_use]
    pub const fn new(index: u32) -> Self {
        match std::num::NonZeroU32::new(index.wrapping_add(1)) {
            Some(n) => Self(n),
            None => panic!("list table index out of range"),
        }
    }

    /// The table entry.
    #[must_use]
    pub const fn index(self) -> usize {
        (self.0.get() - 1) as usize
    }
}

/// A modifier list field: the list of its table entry (Go `ModifierList`
/// embeds its `NodeList`).
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ModifierList {
    pub list: NodeList,
}

/// A direct child that `NodeData::try_for_each_child` visits: a child node
/// id, or a list field (whose ids are in the list table of the data's owner).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Child {
    Node(NodeId),
    List(NodeList),
}

#[derive(Clone, Debug)]
pub struct Node {
    pub kind: SyntaxKind,
    pub flags: NodeFlags,
    pub range: TextRange,
    pub parent: Option<NodeId>,
    pub data: BoxedData,
}

#[derive(Debug)]
pub struct NodeArena {
    id: NodeArenaId,
    revision: NodeArenaRevision,
    nodes: Vec<Node>,
    source_text: Option<String>,
}

impl Clone for NodeArena {
    fn clone(&self) -> Self {
        Self {
            id: allocate_node_arena_id(),
            revision: NodeArenaRevision::INITIAL,
            nodes: self.nodes.clone(),
            source_text: self.source_text.clone(),
        }
    }

    fn clone_from(&mut self, source: &Self) {
        self.id = allocate_node_arena_id();
        self.revision = NodeArenaRevision::INITIAL;
        self.nodes.clone_from(&source.nodes);
        self.source_text.clone_from(&source.source_text);
    }
}

impl Default for NodeArena {
    fn default() -> Self { Self::new() }
}

impl NodeArena {
    /// Creates an empty arena with a fresh, process-local identity.
    ///
    /// # Panics
    ///
    /// Panics if the process has exhausted the arena identity space.
    #[must_use]
    pub fn new() -> Self {
        Self {
            id: allocate_node_arena_id(),
            revision: NodeArenaRevision::INITIAL,
            nodes: Vec::new(),
            source_text: None,
        }
    }

    #[must_use]
    pub const fn id(&self) -> NodeArenaId { self.id }

    #[must_use]
    pub const fn revision(&self) -> NodeArenaRevision { self.revision }

    /// Replaces the retained source text and advances this arena's revision.
    ///
    /// # Panics
    ///
    /// Panics before changing the source text if the revision is exhausted.
    pub fn set_source_text(&mut self, source_text: impl Into<String>) {
        let source_text = source_text.into();
        self.bump_revision();
        self.source_text = Some(source_text);
    }

    #[must_use]
    pub fn source_text(&self) -> Option<&str> { self.source_text.as_deref() }

    /// Allocate a node and return its stable arena identifier.
    ///
    /// # Panics
    ///
    /// Panics before adding the node if the arena contains more than
    /// `u32::MAX` nodes or if the revision is exhausted.
    pub fn alloc(&mut self, node: Node) -> NodeId {
        let index = u32::try_from(self.nodes.len()).expect("AST node arena exceeds u32::MAX nodes");
        self.bump_revision();
        self.nodes.push(node);
        NodeId::new(index)
    }

    #[must_use]
    pub fn get(&self, id: NodeId) -> Option<&Node> { self.nodes.get(id.index()) }

    /// Returns mutable access to an existing node and advances the revision.
    ///
    /// # Panics
    ///
    /// Panics before returning mutable access if the revision is exhausted.
    #[must_use]
    pub fn get_mut(&mut self, id: NodeId) -> Option<&mut Node> {
        if id.index() >= self.nodes.len() {
            return None;
        }
        self.bump_revision();
        self.nodes.get_mut(id.index())
    }

    #[must_use]
    pub fn len(&self) -> usize { self.nodes.len() }

    #[must_use]
    pub fn is_empty(&self) -> bool { self.nodes.is_empty() }

    /// Iterate over nodes in allocation order with their identifiers.
    ///
    /// # Panics
    ///
    /// Panics if the arena invariant limiting it to `u32::MAX` nodes is broken.
    #[must_use]
    pub fn iter(&self) -> impl ExactSizeIterator<Item = (NodeId, &Node)> {
        self.nodes.iter().enumerate().map(|(index, node)| {
            let index = u32::try_from(index).expect("AST node arena exceeds u32::MAX nodes");
            (NodeId::new(index), node)
        })
    }

    fn bump_revision(&mut self) {
        self.revision = self
            .revision
            .checked_next()
            .unwrap_or_else(|| panic!("AST node arena revision space exhausted"));
    }

    #[cfg(test)]
    pub(crate) fn exhaust_revision_for_test(&mut self) {
        self.revision = NodeArenaRevision(u64::MAX);
    }
}

"#,
    );
}

#[cfg(test)]
mod tests {
    use crate::{parse_schema, rust_type_name};

    use super::generate_ast;

    const UPSTREAM_AST: &str = include_str!("../spec/ast.json");

    #[test]
    fn current_schema_emits_every_node_definition() {
        let output = generate_ast(UPSTREAM_AST).unwrap();
        assert!(output.contains("pub struct NodeArenaId(std::num::NonZeroU64);"));
        assert!(output.contains("static LAST_NODE_ARENA_ID:"));
        assert!(output.contains("pub(crate) fn allocate_node_arena_id_from("));
        assert!(output.contains("pub struct NodeArenaRevision(u64);"));
        assert!(output.contains("revision: NodeArenaRevision,"));
        assert!(output.contains("impl Clone for NodeArena"));
        assert!(output.contains("fn clone_from(&mut self, source: &Self)"));
        assert!(output.contains("id: allocate_node_arena_id(),"));
        assert!(output.contains("revision: NodeArenaRevision::INITIAL,"));
        assert!(output.contains("pub const SCHEMA_NODE_COUNT: usize = 192;"));
        assert!(output.contains("pub const SCHEMA_BASE_COUNT: usize = 35;"));
        assert!(output.contains("pub const SCHEMA_NODE_ALIAS_COUNT: usize = 72;"));
        assert!(output.contains("pub const SCHEMA_LIST_ALIAS_COUNT: usize = 23;"));
        assert_eq!(output.matches("(Box<").count(), 192);
        assert!(output.contains("#[repr(u8)]\npub enum BoxedData {"));
        assert!(output.contains("#[repr(u8)]\npub enum NodeData<'a> {"));
        assert_eq!(output.matches("(&'a ").count(), 192);
        assert!(output.contains("pub fn view(&self) -> NodeData<'_>"));
        assert!(output.contains("pub fn to_boxed(self) -> BoxedData"));
        assert!(output.contains("pub trait AstData: Sized"));
        assert!(output.contains("pub struct SourceFileData"));
        assert!(output.contains("pub struct JsDocParameterOrPropertyTagData"));
    }

    #[test]
    fn hard_schema_cases_have_resolved_rust_fields() {
        let output = generate_ast(UPSTREAM_AST).unwrap();
        let for_in_or_of = output
            .split("pub struct ForInOrOfStatementData")
            .nth(1)
            .unwrap()
            .split('}')
            .next()
            .unwrap();
        assert!(!for_in_or_of.contains("pub kind:"));
        assert!(output.contains("pub else_statement: Option<NodeId>"));
        assert!(output.contains("pub statements: NodeList"));
        assert!(output.contains("pub modifiers: Option<ModifierList>"));
        assert!(!output.contains("pub flow_node:"));
        let export_assignment = output
            .split("pub struct ExportAssignmentData")
            .nth(1)
            .unwrap()
            .split('}')
            .next()
            .unwrap();
        assert!(export_assignment.contains("pub type_: Option<NodeId>"));
        assert!(output.contains("pub type Expression = NodeId;"));
        assert!(output.contains("pub type StatementList = NodeList;"));
    }

    #[test]
    fn current_schema_emits_one_direct_child_arm_per_payload() {
        let output = generate_ast(UPSTREAM_AST).unwrap();
        let visitor = output
            .split("// BEGIN schema-exhaustive direct child visitor")
            .nth(1)
            .unwrap()
            .split("// END schema-exhaustive direct child visitor")
            .next()
            .unwrap();
        let schema = parse_schema(UPSTREAM_AST).unwrap();

        assert_eq!(
            visitor.matches("            Self::").count(),
            schema.nodes.definitions.len()
        );
        for name in schema.nodes.definitions.keys() {
            let variant = rust_type_name(name);
            assert!(
                visitor.contains(&format!("            Self::{variant}(")),
                "missing direct-child arm for {name}"
            );
        }
    }

    #[test]
    fn generated_child_order_covers_upstream_special_cases() {
        let output = generate_ast(UPSTREAM_AST).unwrap();

        assert!(output.contains(
            "Self::DoStatement(node) => {\n                visitor(Child::Node(node.statement))?;\n                visitor(Child::Node(node.expression))"
        ));
        assert!(output.contains(
            "Self::FunctionDeclaration(node) => {\n                visit_optional_modifier_list(visitor, node.modifiers)?;\n                visit_optional_child(visitor, node.asterisk_token)?;\n                visit_optional_child(visitor, node.name)?;\n                visit_optional_node_list(visitor, node.type_parameters)?;\n                visitor(Child::List(node.parameters))?;\n                visit_optional_child(visitor, node.type_)?;\n                visit_optional_child(visitor, node.full_signature)?;\n                visit_optional_child(visitor, node.body)"
        ));
        assert!(output.contains(
            "Self::ImportDeclaration(node) => {\n                visit_optional_modifier_list(visitor, node.modifiers)?;\n                visit_optional_child(visitor, node.import_clause)?;\n                visitor(Child::Node(node.module_specifier))?;\n                visit_optional_child(visitor, node.attributes)"
        ));
        assert!(output.contains(
            "Self::ExportAssignment(node) => {\n                visit_optional_modifier_list(visitor, node.modifiers)?;\n                visit_optional_child(visitor, node.type_)?;\n                visitor(Child::Node(node.expression))"
        ));
        assert!(output.contains(
            "Self::SourceFile(node) => {\n                visitor(Child::List(node.statements))?;\n                visitor(Child::Node(node.end_of_file_token))"
        ));
        assert!(output.contains(
            "Self::JsDocParameterOrPropertyTag(node) => {\n                visitor(Child::Node(node.tag_name))?;\n                if node.is_name_first"
        ));
    }
}
