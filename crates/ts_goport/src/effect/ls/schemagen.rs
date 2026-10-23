//! Port of Effect-TS/tsgo `internal/schemagen` (effect-tsgo 0.51.1,
//! `47cb1ed7`): `schemagen.go` converts type nodes into Effect Schema API
//! calls (the typeToEffectSchema and typeToEffectSchemaClass refactors), and
//! `structural.go` converts checker types into them (structuralTypeToSchema).
//!
//! Go keeps the rewriter tracker in both generators and calls only its node
//! factory (`g.Tracker.New*`). The port keeps that factory.

use crate::effect::typeparser::{EffectMajorVersion, TypeParser, find_module_identifier};
use crate::prelude::*;

// Go: schemagen/schemagen.go:18
// Errors returned by schema generation when a type cannot be converted.
const ERR_TYPE_PARAMETERS_NOT_SUPPORTED: &str = "could not process types with type parameters";
const ERR_ONLY_LITERAL_PROPERTIES_SUPPORTED: &str = "only literal properties are supported";
const ERR_REQUIRED_EXPLICIT_TYPES: &str = "only explicit types are supported";
const ERR_INDEX_SIGNATURE_MULTIPLE_PARAMETERS: &str =
    "only index signatures with one parameter are supported";

/// `<schema_id>.<name>` (e.g., `Schema.String`).
// PORT: Go repeats this body as `createApiPropertyAccess` in both
// generators.
fn api_property_access(f: &NodeFactory, schema_id: &str, name: &str) -> Node {
    f.new_property_access_expression(
        f.new_identifier(schema_id),
        Node::NIL,
        f.new_identifier(name),
        NodeFlags::NONE,
    )
}

/// `<schema_id>.<name>(...args)` (e.g., `Schema.Array(Schema.String)`).
// PORT: Go repeats this body as `createApiCall` in both generators.
fn api_call(f: &NodeFactory, schema_id: &str, name: &str, args: &[Node]) -> Node {
    f.new_call_expression(
        f.new_property_access_expression(
            f.new_identifier(schema_id),
            Node::NIL,
            f.new_identifier(name),
            NodeFlags::NONE,
        ),
        Node::NIL,     // questionDotToken
        NodeList::NIL, // typeArguments
        f.new_node_list(args),
        NodeFlags::NONE,
    )
}

/// `first.pipe(<schema_id>.extend(second), <schema_id>.extend(third), ...)`.
// PORT: Go repeats this tail for intersection types in both generators.
fn extend_pipe(f: &NodeFactory, schema_id: &str, members: &[Node]) -> Node {
    let first = members[0];
    let extend_args: Vec<Node> = members[1..]
        .iter()
        .map(|&member| api_call(f, schema_id, "extend", &[member]))
        .collect();
    f.new_call_expression(
        f.new_property_access_expression(
            first,
            Node::NIL,
            f.new_identifier("pipe"),
            NodeFlags::NONE,
        ),
        Node::NIL,
        NodeList::NIL,
        f.new_node_list(&extend_args),
        NodeFlags::NONE,
    )
}

/// `{ <key_name>: key, <value_name>: value }` on one line.
fn two_property_object(
    f: &NodeFactory,
    key_name: &str,
    key: Node,
    value_name: &str,
    value: Node,
) -> Node {
    f.new_object_literal_expression(
        f.new_node_list(&[
            f.new_property_assignment(
                ModifierList::NIL,
                f.new_identifier(key_name),
                Node::NIL,
                Node::NIL,
                key,
            ),
            f.new_property_assignment(
                ModifierList::NIL,
                f.new_identifier(value_name),
                Node::NIL,
                Node::NIL,
                value,
            ),
        ]),
        false,
    )
}

/// `export` as a new modifier list.
fn export_modifiers(f: &NodeFactory) -> ModifierList {
    f.new_modifier_list(&[f.new_modifier(SyntaxKind::ExportKeyword)])
}

/// `class <name> extends <schema_id>.Class<name>("name")(fields) {}` with
/// `modifiers`.
// PORT: Go builds this class in `createExportSchemaClassDeclaration` (with
// `export`) and in `processObjectType` (no modifiers).
fn schema_class_declaration(
    f: &NodeFactory,
    schema_id: &str,
    modifiers: impl FnOnce(&NodeFactory) -> ModifierList,
    name: &str,
    fields: Node,
) -> Node {
    // Schema.Class
    let class_access = api_property_access(f, schema_id, "Class");

    // Schema.Class<Name>("Name")
    let inner_call = f.new_call_expression(
        class_access,
        Node::NIL, // questionDotToken
        f.new_node_list(&[f.new_type_reference_node(f.new_identifier(name), NodeList::NIL)]),
        f.new_node_list(&[f.new_string_literal(name, TokenFlags::NONE)]),
        NodeFlags::NONE,
    );

    // Schema.Class<Name>("Name")({...})
    let outer_call = f.new_call_expression(
        inner_call,
        Node::NIL,
        NodeList::NIL,
        f.new_node_list(&[fields]),
        NodeFlags::NONE,
    );

    // extends ...
    let expr_with_type_args =
        f.new_expression_with_type_arguments(outer_call, f.new_node_list(&[]));
    let heritage_clause = f.new_heritage_clause(
        SyntaxKind::ExtendsKeyword,
        f.new_node_list(&[expr_with_type_args]),
    );

    f.new_class_declaration(
        modifiers(f),
        f.new_identifier(name),
        NodeList::NIL, // typeParameters
        f.new_node_list(&[heritage_clause]),
        f.new_node_list(&[]), // empty members
    )
}

// Go: schemagen/schemagen.go:26 SchemaGen
/// SchemaGen holds the context needed to convert AST type nodes into Schema expressions.
pub struct SchemaGen<'f> {
    pub f: &'f NodeFactory,
    pub source_file: Node,
    pub schema_identifier: String,
    pub version: EffectMajorVersion,
}

impl<'f> SchemaGen<'f> {
    // Go: schemagen/schemagen.go:35 New
    /// New creates a SchemaGen from the given tracker and source file, resolving the
    /// Schema import identifier automatically.
    #[must_use]
    pub fn new(f: &'f NodeFactory, sf: Node, version: EffectMajorVersion) -> Self {
        SchemaGen {
            f,
            source_file: sf,
            schema_identifier: find_module_identifier(sf, "Schema"),
            version,
        }
    }

    // Go: schemagen/schemagen.go:45 createApiPropertyAccess
    /// createApiPropertyAccess creates Schema.<name> (e.g., Schema.String).
    fn create_api_property_access(&self, name: &str) -> Node {
        api_property_access(self.f, &self.schema_identifier, name)
    }

    // Go: schemagen/schemagen.go:55 createApiCall
    /// createApiCall creates Schema.<name>(...args) (e.g., Schema.Array(Schema.String)).
    fn create_api_call(&self, name: &str, args: &[Node]) -> Node {
        api_call(self.f, &self.schema_identifier, name, args)
    }

    // Go: schemagen/schemagen.go:72 unsupportedComment
    /// unsupportedComment creates a placeholder identifier for unsupported conversions.
    /// This produces /* Not supported conversion: <text> */ in the emitted output.
    fn unsupported_comment(&self, node: Node) -> Node {
        let text = get_text_of_node(node);
        self.f
            .new_identifier(format!("undefined /* Not supported conversion: {text} */"))
    }

    // Go: schemagen/schemagen.go:78 processNode
    /// processNode recursively converts a TypeScript type AST node into a Schema expression.
    fn process_node(&self, node: Node) -> Result<Node, &'static str> {
        if node.is_nil() {
            return Ok(self.create_api_property_access("Unknown"));
        }

        // Primitive keywords
        let keyword = match node.kind() {
            SyntaxKind::AnyKeyword => Some("Any"),
            SyntaxKind::NeverKeyword => Some("Never"),
            SyntaxKind::UnknownKeyword => Some("Unknown"),
            SyntaxKind::VoidKeyword => Some("Void"),
            SyntaxKind::NullKeyword => Some("Null"),
            SyntaxKind::UndefinedKeyword => Some("Undefined"),
            SyntaxKind::StringKeyword => Some("String"),
            SyntaxKind::NumberKeyword => Some("Number"),
            SyntaxKind::BooleanKeyword => Some("Boolean"),
            SyntaxKind::BigIntKeyword => Some("BigInt"),
            _ => None,
        };
        if let Some(name) = keyword {
            return Ok(self.create_api_property_access(name));
        }

        // Literal types: string/number/boolean literals and null
        if node.kind() == SyntaxKind::LiteralType {
            if node.literal().kind() == SyntaxKind::NullKeyword {
                return Ok(self.create_api_property_access("Null"));
            }
            if let Some(literals) = self.parse_all_literals(node)
                && !literals.is_empty()
            {
                return Ok(self.create_api_call("Literal", &literals));
            }
        }

        // Union types: A | B
        if node.kind() == SyntaxKind::UnionType {
            let types = node.types();
            if types.is_nil() {
                return Ok(self.unsupported_comment(node));
            }

            // Optimization: try to collapse all-literal unions into Schema.Literal(...)
            if self.version != EffectMajorVersion::V4
                && let Some(all_literals) = self.parse_all_literals(node)
                && !all_literals.is_empty()
            {
                return Ok(self.create_api_call("Literal", &all_literals));
            }

            let members = self.process_node_list(types.nodes())?;

            let args = if self.version == EffectMajorVersion::V4 {
                vec![
                    self.f
                        .new_array_literal_expression(self.f.new_node_list(&members), false),
                ]
            } else {
                members
            };
            return Ok(self.create_api_call("Union", &args));
        }

        // Intersection types: A & B
        if node.kind() == SyntaxKind::IntersectionType {
            let types = node.types();
            if types.is_nil() || types.nodes().is_empty() {
                return Ok(self.unsupported_comment(node));
            }

            let members = self.process_node_list(types.nodes())?;

            if members.len() == 1 {
                return Ok(members[0]);
            }

            // firstSchema.pipe(Schema.extend(second), Schema.extend(third), ...)
            return Ok(extend_pipe(self.f, &self.schema_identifier, &members));
        }

        // Type operator: keyof T, readonly T
        if node.kind() == SyntaxKind::TypeOperator {
            if node.operator() == SyntaxKind::KeyOfKeyword {
                let inner = self.process_node(node.type_())?;
                return Ok(self.create_api_call("keyof", &[inner]));
            }
            if node.operator() == SyntaxKind::ReadonlyKeyword {
                return self.process_node(node.type_());
            }
        }

        // Array type: T[]
        if node.kind() == SyntaxKind::ArrayType {
            let elem_schema = self.process_node(node.element_type())?;
            return Ok(self.create_api_call("Array", &[elem_schema]));
        }

        // Tuple type: [A, B, C]
        if node.kind() == SyntaxKind::TupleType {
            // PORT: Go processes `tt.Elements` only when the list is not nil;
            // a nil list gives no elements, as `elements()` does.
            let elems = self.process_node_list(node.elements())?;
            let args = if self.version == EffectMajorVersion::V4 {
                vec![
                    self.f
                        .new_array_literal_expression(self.f.new_node_list(&elems), false),
                ]
            } else {
                elems
            };
            return Ok(self.create_api_call("Tuple", &args));
        }

        // Type literal (object shape): { a: string, b: number }
        if node.kind() == SyntaxKind::TypeLiteral {
            let (properties, records) = self.process_members(node.members())?;
            let mut args = vec![
                self.f
                    .new_object_literal_expression(self.f.new_node_list(&properties), true),
            ];
            args.extend(records);
            return Ok(self.create_api_call("Struct", &args));
        }

        // Parenthesized type: (A)
        if node.kind() == SyntaxKind::ParenthesizedType {
            return self.process_node(node.type_());
        }

        // Type reference: resolve known Effect types and data types
        if node.kind() == SyntaxKind::TypeReference {
            let data_type_name = self.entity_name_to_data_type_name(node.type_name());
            if !data_type_name.is_empty() {
                return self.process_type_reference(node, data_type_name);
            }

            // Simple type reference without type arguments → pass through as identifier
            if node.type_arguments().is_empty() {
                return Ok(self.type_entity_name_to_node(node.type_name()));
            }
        }

        // Unsupported
        Ok(self.unsupported_comment(node))
    }

    // Go: schemagen/schemagen.go:268 processTypeReference
    /// processTypeReference handles known type references like Array, Record, Option, etc.
    fn process_type_reference(
        &self,
        node: Node,
        data_type_name: &'static str,
    ) -> Result<Node, &'static str> {
        let type_args: Vec<Node> = node.type_arguments().into_iter().collect();

        match data_type_name {
            "Duration" | "Date" => Ok(self.create_api_property_access(data_type_name)),

            "Option" | "Chunk" | "Array" => {
                let elements = self.process_node_list(type_args)?;
                Ok(self.create_api_call(data_type_name, &elements))
            }

            "Record" => {
                let elements = self.process_node_list(type_args)?;
                if elements.len() >= 2 {
                    if self.version == EffectMajorVersion::V4 {
                        return Ok(self.create_api_call("Record", &[elements[0], elements[1]]));
                    }
                    return Ok(self.create_api_call(
                        data_type_name,
                        &[two_property_object(
                            self.f,
                            "key",
                            elements[0],
                            "value",
                            elements[1],
                        )],
                    ));
                }
                Ok(self.unsupported_comment(node))
            }

            "Either" => {
                let elements = self.process_node_list(type_args)?;
                if elements.len() >= 2 {
                    return Ok(self.create_api_call(
                        data_type_name,
                        &[two_property_object(
                            self.f,
                            "right",
                            elements[0],
                            "left",
                            elements[1],
                        )],
                    ));
                }
                Ok(self.unsupported_comment(node))
            }

            "Pick" | "Omit" => {
                if type_args.len() != 2 {
                    return Ok(self.unsupported_comment(node));
                }
                let base_type = self.process_node(type_args[0])?;
                let Some(string_literals) = self
                    .parse_all_literals(type_args[1])
                    .filter(|literals| !literals.is_empty())
                else {
                    return Ok(self.unsupported_comment(node));
                };
                // Use lowercase: "pick" or "omit"
                let method_name = if data_type_name == "Omit" {
                    "omit"
                } else {
                    "pick"
                };
                let f = self.f;
                Ok(f.new_call_expression(
                    f.new_property_access_expression(
                        base_type,
                        Node::NIL,
                        f.new_identifier("pipe"),
                        NodeFlags::NONE,
                    ),
                    Node::NIL,
                    NodeList::NIL,
                    f.new_node_list(&[self.create_api_call(method_name, &string_literals)]),
                    NodeFlags::NONE,
                ))
            }

            _ => Ok(self.unsupported_comment(node)),
        }
    }

    // Go: schemagen/schemagen.go:352 entityNameToDataTypeName
    /// entityNameToDataTypeName maps a type name to a recognized data type name.
    fn entity_name_to_data_type_name(&self, type_name: Node) -> &'static str {
        if type_name.is_nil() {
            return "";
        }
        if type_name.kind() == SyntaxKind::Identifier {
            return match type_name.text() {
                "Date" => "Date",
                "Pick" => "Pick",
                "Omit" => "Omit",
                "Record" => "Record",
                "ReadonlyArray" | "Array" => "Array",
                _ => "",
            };
        }
        // Qualified name: Module.Name (e.g., Option.Option)
        if type_name.kind() == SyntaxKind::QualifiedName {
            let left = type_name.left();
            let right = type_name.right();
            if left.is_nil() || right.is_nil() {
                return "";
            }
            if left.kind() != SyntaxKind::Identifier {
                return "";
            }
            let right_name = get_text_of_node(right);
            let left_name = get_text_of_node(left);
            // Check if leftName is an imported module identifier for a known module
            for module_name in ["Option", "Either", "Chunk", "Duration"] {
                let imported = find_module_identifier(self.source_file, module_name);
                if left_name == imported && right_name == module_name {
                    return module_name;
                }
            }
        }
        ""
    }

    // Go: schemagen/schemagen.go:389 typeEntityNameToNode
    /// typeEntityNameToNode converts a type entity name to an expression node (identifier or property access).
    fn type_entity_name_to_node(&self, entity_name: Node) -> Node {
        if entity_name.is_nil() {
            return self.f.new_identifier("unknown");
        }
        if entity_name.kind() == SyntaxKind::Identifier {
            return self.f.new_identifier(entity_name.text());
        }
        if entity_name.kind() == SyntaxKind::QualifiedName {
            let left = self.type_entity_name_to_node(entity_name.left());
            return self.f.new_property_access_expression(
                left,
                Node::NIL,
                self.f.new_identifier(get_text_of_node(entity_name.right())),
                NodeFlags::NONE,
            );
        }
        self.f.new_identifier(get_text_of_node(entity_name))
    }

    // Go: schemagen/schemagen.go:410 parseAllLiterals
    /// parseAllLiterals tries to parse a type node as a collection of literal values.
    /// Returns Some(literals) on success, None if the node contains non-literal types.
    // PORT: Go returns `(literals, ok)`.
    fn parse_all_literals(&self, node: Node) -> Option<Vec<Node>> {
        if node.is_nil() {
            return None;
        }

        if node.kind() == SyntaxKind::LiteralType {
            let literal = node.literal();
            return match literal.kind() {
                SyntaxKind::StringLiteral => Some(vec![
                    self.f.new_string_literal(literal.text(), TokenFlags::NONE),
                ]),
                SyntaxKind::NumericLiteral => Some(vec![
                    self.f.new_numeric_literal(literal.text(), TokenFlags::NONE),
                ]),
                SyntaxKind::TrueKeyword => {
                    Some(vec![self.f.new_keyword_expression(SyntaxKind::TrueKeyword)])
                }
                SyntaxKind::FalseKeyword => Some(vec![
                    self.f.new_keyword_expression(SyntaxKind::FalseKeyword),
                ]),
                _ => None,
            };
        }

        if node.kind() == SyntaxKind::UnionType {
            let types = node.types();
            if types.is_nil() {
                return None;
            }
            let mut result = Vec::new();
            for t in types.nodes() {
                result.extend(self.parse_all_literals(t)?);
            }
            return Some(result);
        }

        if node.kind() == SyntaxKind::ParenthesizedType {
            return self.parse_all_literals(node.type_());
        }

        None
    }

    // Go: schemagen/schemagen.go:455 processNodeList
    /// processNodeList processes a list of type nodes, converting each to a Schema expression.
    fn process_node_list(
        &self,
        nodes: impl IntoIterator<Item = Node>,
    ) -> Result<Vec<Node>, &'static str> {
        nodes.into_iter().map(|n| self.process_node(n)).collect()
    }

    // Go: schemagen/schemagen.go:474 processMembers
    /// processMembers handles property signatures and index signatures from type members.
    /// Returns (properties, records).
    // PORT: Go's unused `membersResult` type is not ported.
    fn process_members(&self, members: NodeSlice) -> Result<(Vec<Node>, Vec<Node>), &'static str> {
        let f = self.f;
        let mut properties = Vec::new();
        let mut records = Vec::new();
        for member in members {
            if member.kind() == SyntaxKind::PropertySignature {
                let name = member.name();
                if name.is_nil() {
                    return Err(ERR_ONLY_LITERAL_PROPERTIES_SUPPORTED);
                }
                if name.kind() != SyntaxKind::Identifier && name.kind() != SyntaxKind::StringLiteral
                {
                    return Err(ERR_ONLY_LITERAL_PROPERTIES_SUPPORTED);
                }
                let type_node = member.type_();
                if type_node.is_nil() {
                    return Err(ERR_REQUIRED_EXPLICIT_TYPES);
                }

                let mut prop_schema = self.process_node(type_node)?;

                // Wrap in Schema.optional(...) if question token present
                if member.question_token().is_some() {
                    prop_schema = self.create_api_call("optional", &[prop_schema]);
                }

                // Create property name node
                let prop_name = if name.kind() == SyntaxKind::Identifier {
                    f.new_identifier(name.text())
                } else {
                    f.new_string_literal(name.text(), TokenFlags::NONE)
                };

                let assignment = f.new_property_assignment(
                    ModifierList::NIL,
                    prop_name,
                    Node::NIL,
                    Node::NIL,
                    prop_schema,
                );
                properties.push(assignment);
            }

            if member.kind() == SyntaxKind::IndexSignature {
                let parameters = member.parameter_list();
                if parameters.is_nil() || parameters.nodes().len() != 1 {
                    return Err(ERR_INDEX_SIGNATURE_MULTIPLE_PARAMETERS);
                }
                let param = parameters.nodes().get(0);
                let param_type = param.type_();
                if param_type.is_nil() {
                    return Err(ERR_REQUIRED_EXPLICIT_TYPES);
                }

                let key = self.process_node(param_type)?;
                let value = self.process_node(member.type_())?;

                records.push(two_property_object(f, "key", key, "value", value));
            }
        }
        Ok((properties, records))
    }

    // Go: schemagen/schemagen.go:545 ProcessInterfaceDeclaration
    /// ProcessInterfaceDeclaration generates schema statements for an interface declaration.
    /// When preferClass is true and there are no index signatures, generates a Schema.Class declaration.
    /// Otherwise generates a Schema.Struct variable declaration.
    /// Returns the generated statement node.
    pub fn process_interface_declaration(
        &self,
        node: Node,
        prefer_class: bool,
    ) -> Result<Node, &'static str> {
        if !node.type_parameters().is_empty() {
            return Err(ERR_TYPE_PARAMETERS_NOT_SUPPORTED);
        }

        let name = get_text_of_node(node.name());

        let (properties, records) = self.process_members(node.members())?;

        if prefer_class && records.is_empty() {
            return Ok(self.create_export_schema_class_declaration(&name, &properties));
        }

        let mut args = vec![
            self.f
                .new_object_literal_expression(self.f.new_node_list(&properties), true),
        ];
        args.extend(records);
        let schema_struct = self.create_api_call("Struct", &args);
        Ok(self.create_export_variable_declaration(&name, schema_struct))
    }

    // Go: schemagen/schemagen.go:579 ProcessTypeAliasDeclaration
    /// ProcessTypeAliasDeclaration generates schema statements for a type alias declaration.
    /// When preferClass is true and the type is an object literal without index signatures,
    /// generates a Schema.Class declaration. Otherwise generates a variable declaration.
    /// Returns the generated statement node.
    pub fn process_type_alias_declaration(
        &self,
        node: Node,
        prefer_class: bool,
    ) -> Result<Node, &'static str> {
        if !node.type_parameters().is_empty() {
            return Err(ERR_TYPE_PARAMETERS_NOT_SUPPORTED);
        }

        let name = get_text_of_node(node.name());

        // For preferClass with object literal types, generate class declaration
        let type_node = node.type_();
        if prefer_class && type_node.is_some() && type_node.kind() == SyntaxKind::TypeLiteral {
            let (properties, records) = self.process_members(type_node.members())?;
            if records.is_empty() {
                return Ok(self.create_export_schema_class_declaration(&name, &properties));
            }
        }

        let effect_schema = self.process_node(type_node)?;

        Ok(self.create_export_variable_declaration(&name, effect_schema))
    }

    // Go: schemagen/schemagen.go:612 createExportVariableDeclaration
    /// createExportVariableDeclaration creates: export const <name> = <initializer>
    fn create_export_variable_declaration(&self, name: &str, initializer: Node) -> Node {
        let f = self.f;
        let var_decl = f.new_variable_declaration(
            f.new_identifier(name),
            Node::NIL, // exclamationToken
            Node::NIL, // type
            initializer,
        );
        let var_decl_list =
            f.new_variable_declaration_list(f.new_node_list(&[var_decl]), NodeFlags::CONST);
        f.new_variable_statement(export_modifiers(f), var_decl_list)
    }

    // Go: schemagen/schemagen.go:632 createExportSchemaClassDeclaration
    /// createExportSchemaClassDeclaration creates:
    ///
    ///	export class <Name> extends Schema.Class<Name>("Name")({...}) { }
    fn create_export_schema_class_declaration(&self, name: &str, properties: &[Node]) -> Node {
        let fields = self
            .f
            .new_object_literal_expression(self.f.new_node_list(properties), true);
        schema_class_declaration(
            self.f,
            &self.schema_identifier,
            export_modifiers,
            name,
            fields,
        )
    }

    // Go: schemagen/schemagen.go:684 Process
    /// Process converts an interface or type alias declaration into a schema statement.
    /// On error, returns a comment node describing the error.
    #[must_use]
    pub fn process(&self, node: Node, prefer_class: bool) -> Node {
        let result = match node.kind() {
            SyntaxKind::InterfaceDeclaration => {
                self.process_interface_declaration(node, prefer_class)
            }
            SyntaxKind::TypeAliasDeclaration => {
                self.process_type_alias_declaration(node, prefer_class)
            }
            _ => return Node::NIL,
        };

        match result {
            Err(err) => self.f.new_identifier(format!("undefined /* {err} */")),
            Ok(result) => {
                set_parent_in_children(result);
                result
            }
        }
    }
}

// Go: schemagen/schemagen.go:707 HasIndexSignatures
/// HasIndexSignatures checks whether an interface or type alias has index signatures.
/// Used by typeToEffectSchemaClass to determine applicability.
#[must_use]
pub fn has_index_signatures(node: Node) -> bool {
    let members = match node.kind() {
        SyntaxKind::InterfaceDeclaration => node.members(),
        SyntaxKind::TypeAliasDeclaration => {
            let type_node = node.type_();
            if type_node.is_some() && type_node.kind() == SyntaxKind::TypeLiteral {
                type_node.members()
            } else {
                NodeSlice::NIL
            }
        }
        _ => NodeSlice::NIL,
    };
    members
        .into_iter()
        .any(|m| m.kind() == SyntaxKind::IndexSignature)
}

// Go: schemagen/structural.go:41 hoistedEntry
// PORT: Go keeps a `createRef` closure; every closure gives a new
// identifier of one name, so the entry keeps that name (`ref_name`) and
// `StructuralSchemaGen::create_ref` makes the identifier.
#[derive(Clone)]
struct HoistedEntry {
    t: TypeId,
    ref_name: String,
}

// Go: schemagen/structural.go:47 processingContext
/// processingContext tracks state during recursive type traversal.
#[derive(Clone)]
struct ProcessingContext {
    depth: i32,
    max_depth: i32,
    hoist_name: String,
    version: EffectMajorVersion,
}

// Go: schemagen/structural.go:21 StructuralSchemaGen
/// StructuralSchemaGen holds the context for converting resolved types to Schema expressions.
// PORT: Go's `Checker` field is `tp.checker`. The Go maps that are ranged
// over (`hoistedSchemas`, `nameToType`) have random order in Go; the
// IndexMaps keep insertion order. The order only shows when two entries
// match the same type.
pub struct StructuralSchemaGen<'f, 'a, 'c> {
    pub f: &'f NodeFactory,
    pub source_file: Node,
    pub tp: &'a mut TypeParser<'c>,
    pub schema_identifier: String,
    pub version: EffectMajorVersion,

    /// hoistedSchemas maps types to expression generators for reuse
    hoisted_schemas: IndexMap<TypeId, HoistedEntry>,
    /// typeToStatementIndex tracks which types have been hoisted to which statement index
    type_to_statement_index: FxHashMap<TypeId, usize>,
    /// nameToType maps requested names to their types
    name_to_type: IndexMap<String, TypeId>,
    /// schemaStatements accumulates generated top-level statements
    schema_statements: Vec<Node>,
    /// usedGlobalIdentifiers tracks used identifier names and their count
    used_global_identifiers: FxHashMap<String, i32>,
}

// Go: schemagen/structural.go:434 validIdentifierRegex
/// validIdentifierRegex matches valid JS identifiers: `^[a-zA-Z_$][a-zA-Z0-9_$]*$`.
fn is_valid_identifier(s: &str) -> bool {
    let mut bytes = s.bytes();
    let Some(first) = bytes.next() else {
        return false;
    };
    (first.is_ascii_alphabetic() || first == b'_' || first == b'$')
        && bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'$')
}

impl<'f, 'a, 'c> StructuralSchemaGen<'f, 'a, 'c> {
    // Go: schemagen/structural.go:55 NewStructuralSchemaGen
    /// NewStructuralSchemaGen creates a StructuralSchemaGen for type-checker-based schema generation.
    pub fn new(
        f: &'f NodeFactory,
        tp: &'a mut TypeParser<'c>,
        sf: Node,
        version: EffectMajorVersion,
    ) -> Self {
        StructuralSchemaGen {
            f,
            source_file: sf,
            tp,
            schema_identifier: find_module_identifier(sf, "Schema"),
            version,
            hoisted_schemas: IndexMap::new(),
            type_to_statement_index: FxHashMap::default(),
            name_to_type: IndexMap::new(),
            schema_statements: Vec::new(),
            used_global_identifiers: FxHashMap::default(),
        }
    }

    // Go: schemagen/structural.go:72 createApiPropertyAccess
    /// createApiPropertyAccess creates Schema.<name>.
    fn create_api_property_access(&self, name: &str) -> Node {
        api_property_access(self.f, &self.schema_identifier, name)
    }

    // Go: schemagen/structural.go:82 createApiCall
    /// createApiCall creates Schema.<name>(...args).
    fn create_api_call(&self, name: &str, args: &[Node]) -> Node {
        api_call(self.f, &self.schema_identifier, name, args)
    }

    /// The reference expression of a hoisted entry (Go `entry.createRef()`).
    fn create_ref(&self, ref_name: &str) -> Node {
        self.f.new_identifier(ref_name)
    }

    // Go: schemagen/structural.go:97 pushHoistedStatement
    /// pushHoistedStatement records a generated statement and its reference generator.
    fn push_hoisted_statement(&mut self, name: &str, t: TypeId, statement: Node, ref_name: &str) {
        *self
            .used_global_identifiers
            .entry(name.to_string())
            .or_insert(0) += 1;
        self.schema_statements.push(statement);
        self.type_to_statement_index
            .insert(t, self.schema_statements.len() - 1);
        self.hoisted_schemas.insert(
            t,
            HoistedEntry {
                t,
                ref_name: ref_name.to_string(),
            },
        );
    }

    // Go: schemagen/structural.go:105 pushHoistedVariableStatement
    /// pushHoistedVariableStatement creates a const variable statement and hoists it.
    fn push_hoisted_variable_statement(&mut self, name: &str, t: TypeId, result: Node) {
        let f = self.f;
        let var_decl =
            f.new_variable_declaration(f.new_identifier(name), Node::NIL, Node::NIL, result);
        let var_decl_list =
            f.new_variable_declaration_list(f.new_node_list(&[var_decl]), NodeFlags::CONST);
        let stmt = f.new_variable_statement(ModifierList::NIL, var_decl_list);
        self.push_hoisted_statement(name, t, stmt, name);
    }

    /// The reference of the hoisted entry of `t`, if any.
    fn hoisted_ref(&self, t: TypeId) -> Option<Node> {
        self.hoisted_schemas
            .get(&t)
            .map(|entry| self.create_ref(&entry.ref_name))
    }

    // Go: schemagen/structural.go:122 processType
    /// processType converts a TypeScript type to a Schema expression, with hoisting support.
    fn process_type(&mut self, t: TypeId, ctx: &ProcessingContext) -> Result<Node, String> {
        if ctx.depth >= ctx.max_depth {
            return Err("maximum depth exceeded".to_string());
        }

        // Try to resolve a hoist name from the type
        let hoist_name = self.resolve_hoist_name(t);

        let nested_ctx = ProcessingContext {
            depth: ctx.depth + 1,
            max_depth: ctx.max_depth,
            hoist_name: hoist_name.clone(),
            version: ctx.version,
        };

        // Check if we can reuse a hoisted schema
        for i in 0..self.hoisted_schemas.len() {
            let entry_t = self.hoisted_schemas[i].t;
            if entry_t == t || self.types_equal(t, entry_t) {
                let ref_name = self.hoisted_schemas[i].ref_name.clone();
                return Ok(self.create_ref(&ref_name));
            }
        }

        // Process the type
        let (expr, skip_hoisting) = self.process_type_impl(t, &nested_ctx)?;

        // Hoist if we have a name and hoisting isn't skipped
        if !skip_hoisting && !hoist_name.is_empty() {
            self.push_hoisted_variable_statement(&hoist_name, t, expr);
            if let Some(reference) = self.hoisted_ref(t) {
                return Ok(reference);
            }
        }

        Ok(expr)
    }

    // Go: schemagen/structural.go:162 resolveHoistName
    /// resolveHoistName tries to find a name for the type for hoisting purposes.
    fn resolve_hoist_name(&mut self, t: TypeId) -> String {
        // Check nameToType map first
        for (name, &existing_type) in &self.name_to_type {
            if existing_type == t {
                return name.clone();
            }
        }

        // Try to get name from the type's symbol
        let symbol = self.tp.checker.ty(t).symbol();
        if symbol.is_some() && self.tp.checker.sym(symbol).declarations.len() == 1 {
            let decl = self.tp.checker.sym(symbol).declarations[0];
            let hoist_name = if decl.kind() == SyntaxKind::InterfaceDeclaration {
                decl.name().text().to_string()
            } else if decl.parent().is_some()
                && decl.parent().kind() == SyntaxKind::TypeAliasDeclaration
            {
                decl.parent().name().text().to_string()
            } else {
                String::new()
            };
            if !hoist_name.is_empty() {
                if let Some(&existing_type) = self.name_to_type.get(&hoist_name)
                    && existing_type.is_some()
                    && self.types_equal(t, existing_type)
                {
                    return hoist_name;
                }
                // Avoid name conflicts
                let used_count = self
                    .used_global_identifiers
                    .get(&hoist_name)
                    .copied()
                    .unwrap_or(0);
                self.used_global_identifiers
                    .insert(hoist_name.clone(), used_count + 1);
                if used_count > 0 {
                    return format!("{hoist_name}_{used_count}");
                }
                return hoist_name;
            }
        }

        String::new()
    }

    // Go: schemagen/structural.go:198 typesEqual
    /// typesEqual checks if two types are bidirectionally assignable.
    fn types_equal(&mut self, a: TypeId, b: TypeId) -> bool {
        self.tp.checker.is_type_assignable_to(a, b) && self.tp.checker.is_type_assignable_to(b, a)
    }

    // Go: schemagen/structural.go:204 processTypeImpl
    /// processTypeImpl dispatches type processing based on type flags.
    /// Returns (expr, skipHoisting).
    fn process_type_impl(
        &mut self,
        t: TypeId,
        ctx: &ProcessingContext,
    ) -> Result<(Node, bool), String> {
        let flags = self.tp.checker.ty(t).flags();

        // Primitive types
        let primitive = [
            (TypeFlags::STRING, "String"),
            (TypeFlags::NUMBER, "Number"),
            (TypeFlags::BOOLEAN, "Boolean"),
            (TypeFlags::BIG_INT, "BigInt"),
            (TypeFlags::VOID, "Void"),
            (TypeFlags::UNDEFINED, "Undefined"),
            (TypeFlags::NULL, "Null"),
            (TypeFlags::NEVER, "Never"),
            (TypeFlags::ANY, "Any"),
            (TypeFlags::UNKNOWN, "Unknown"),
        ];
        for (flag, name) in primitive {
            if flags.intersects(flag) {
                return Ok((self.create_api_property_access(name), true));
            }
        }

        // Literal types
        if flags.intersects(TypeFlags::STRING_LITERAL)
            && let Some(LiteralValue::String(val)) = self.tp.checker.ty(t).as_literal_type().value()
        {
            let literal = self.f.new_string_literal(val.as_str(), TokenFlags::NONE);
            return Ok((self.create_api_call("Literal", &[literal]), true));
        }
        if flags.intersects(TypeFlags::NUMBER_LITERAL) {
            let text = self.tp.checker.ty(t).as_literal_type().string();
            let literal = self.f.new_numeric_literal(text, TokenFlags::NONE);
            return Ok((self.create_api_call("Literal", &[literal]), true));
        }
        if flags.intersects(TypeFlags::BOOLEAN_LITERAL) {
            let is_true = matches!(
                self.tp.checker.ty(t).as_literal_type().value(),
                Some(LiteralValue::Bool(true))
            );
            let keyword = if is_true {
                SyntaxKind::TrueKeyword
            } else {
                SyntaxKind::FalseKeyword
            };
            let literal = self.f.new_keyword_expression(keyword);
            return Ok((self.create_api_call("Literal", &[literal]), true));
        }

        // Union types
        if flags.intersects(TypeFlags::UNION) {
            let types = self.tp.checker.ty(t).types().to_vec();
            return self.process_union_type(&types, ctx);
        }

        // Intersection types
        if flags.intersects(TypeFlags::INTERSECTION) {
            let types = self.tp.checker.ty(t).types().to_vec();
            return self.process_intersection_type(&types, ctx);
        }

        // Array types (check before generic object)
        if self.tp.checker.is_array_type(t) {
            return self.process_array_type(t, ctx);
        }

        // Tuple types
        if self.tp.checker.is_tuple_type(t) {
            return self.process_tuple_type(t, ctx);
        }

        // Object types
        if flags.intersects(TypeFlags::OBJECT) {
            let symbol = self.tp.checker.ty(t).symbol();
            if symbol.is_some() {
                let type_name = self.tp.checker.symbol_to_string(symbol);
                if type_name == "Date" {
                    return Ok((self.create_api_property_access("Date"), false));
                }
                if type_name == "ReadonlyArray" || type_name == "Array" {
                    return self.process_array_type(t, ctx);
                }
            }
            return self.process_object_type(t, ctx);
        }

        Err(format!("type with flags {} is not supported", flags.bits()))
    }

    // Go: schemagen/structural.go:306 processUnionType
    /// processUnionType converts union types to Schema.Union or optimized Schema.Literal.
    fn process_union_type(
        &mut self,
        types: &[TypeId],
        ctx: &ProcessingContext,
    ) -> Result<(Node, bool), String> {
        // Check if all members are literals (V3 optimization)
        let all_literals = types.iter().all(|&t| {
            self.tp.checker.ty(t).flags().intersects(
                TypeFlags::STRING_LITERAL | TypeFlags::NUMBER_LITERAL | TypeFlags::BOOLEAN_LITERAL,
            )
        });

        if all_literals && ctx.version != EffectMajorVersion::V4 {
            let mut literals = Vec::with_capacity(types.len());
            for &t in types {
                let expr = self.process_type(t, ctx)?;
                // Extract literal value from Schema.Literal(value) call
                if expr.kind() == SyntaxKind::CallExpression {
                    let args = expr.arguments();
                    if !args.is_empty() {
                        literals.push(args.get(0));
                        continue;
                    }
                }
                literals.push(expr);
            }
            return Ok((self.create_api_call("Literal", &literals), false));
        }

        // Process each union member
        let mut members = Vec::with_capacity(types.len());
        for &t in types {
            members.push(self.process_type(t, ctx)?);
        }

        if members.len() == 1 {
            return Ok((members[0], false));
        }

        let args = if ctx.version == EffectMajorVersion::V4 {
            vec![
                self.f
                    .new_array_literal_expression(self.f.new_node_list(&members), false),
            ]
        } else {
            members
        };
        Ok((self.create_api_call("Union", &args), false))
    }

    // Go: schemagen/structural.go:361 processIntersectionType
    /// processIntersectionType converts intersection types to Schema.extend chains.
    fn process_intersection_type(
        &mut self,
        types: &[TypeId],
        ctx: &ProcessingContext,
    ) -> Result<(Node, bool), String> {
        let mut members = Vec::with_capacity(types.len());
        for &t in types {
            members.push(self.process_type(t, ctx)?);
        }

        if members.is_empty() {
            return Err("empty intersection type".to_string());
        }
        if members.len() == 1 {
            return Ok((members[0], false));
        }

        // firstSchema.pipe(Schema.extend(second), Schema.extend(third), ...)
        Ok((
            extend_pipe(self.f, &self.schema_identifier, &members),
            false,
        ))
    }

    // Go: schemagen/structural.go:395 processArrayType
    /// processArrayType converts array types to Schema.Array (wrapping in Schema.mutable for non-readonly).
    fn process_array_type(
        &mut self,
        t: TypeId,
        ctx: &ProcessingContext,
    ) -> Result<(Node, bool), String> {
        let type_args = self.tp.checker.get_type_arguments(t);
        if type_args.is_empty() {
            return Err("array type has no type arguments".to_string());
        }

        let elem_schema = self.process_type(type_args[0], ctx)?;
        let expr = self.create_api_call("Array", &[elem_schema]);
        if self.tp.checker.is_readonly_array_type(t) {
            return Ok((expr, false));
        }
        Ok((self.create_api_call("mutable", &[expr]), false))
    }

    // Go: schemagen/structural.go:413 processTupleType
    /// processTupleType converts tuple types to Schema.Tuple.
    fn process_tuple_type(
        &mut self,
        t: TypeId,
        ctx: &ProcessingContext,
    ) -> Result<(Node, bool), String> {
        let type_args = self.tp.checker.get_type_arguments(t);
        let mut elems = Vec::with_capacity(type_args.len());
        for &ta in type_args.iter() {
            elems.push(self.process_type(ta, ctx)?);
        }

        let args = if ctx.version == EffectMajorVersion::V4 {
            vec![
                self.f
                    .new_array_literal_expression(self.f.new_node_list(&elems), false),
            ]
        } else {
            elems
        };
        Ok((self.create_api_call("Tuple", &args), false))
    }

    // Go: schemagen/structural.go:437 processObjectType
    /// processObjectType converts object types to Schema.Struct or Schema.Class.
    fn process_object_type(
        &mut self,
        t: TypeId,
        ctx: &ProcessingContext,
    ) -> Result<(Node, bool), String> {
        let properties = self.tp.checker.get_properties_of_type_exported(t);
        let mut property_assignments = Vec::with_capacity(properties.len());

        for prop in properties {
            let prop_name = self.tp.checker.symbol_to_string(prop);
            let prop_type = self.tp.checker.get_type_of_symbol_exported(prop);

            let is_optional = self
                .tp
                .checker
                .sym_flags(prop)
                .intersects(SymbolFlags::OPTIONAL);

            let schema_expr = if is_optional {
                let inner_expr = self.process_type(prop_type, ctx)?;
                self.create_api_call("optional", &[inner_expr])
            } else {
                self.process_type(prop_type, ctx)?
            };

            // Create property name node
            let prop_name_node = if is_valid_identifier(&prop_name) {
                self.f.new_identifier(prop_name)
            } else {
                self.f.new_string_literal(prop_name, TokenFlags::NONE)
            };

            let assignment = self.f.new_property_assignment(
                ModifierList::NIL,
                prop_name_node,
                Node::NIL,
                Node::NIL,
                schema_expr,
            );
            property_assignments.push(assignment);
        }

        // Handle index signatures
        let index_infos = self.tp.checker.get_index_infos_of_type(t);
        let mut args = vec![self.f.new_object_literal_expression(
            self.f.new_node_list(&property_assignments),
            !property_assignments.is_empty(),
        )];

        // PORT: Go `recordEntry{key, value}` is a pair.
        let mut records: Vec<(Node, Node)> = Vec::new();
        for &info in index_infos.iter() {
            let (key_type, value_type) = {
                let info = self.tp.checker.index_info(info);
                (info.key_type, info.value_type)
            };

            let key_schema = self.process_type(key_type, ctx)?;
            let value_schema = self.process_type(value_type, ctx)?;
            records.push((key_schema, value_schema));
        }

        // V4: use StructWithRest for structs with index signatures
        if ctx.version == EffectMajorVersion::V4 && !records.is_empty() {
            let record_exprs: Vec<Node> = records
                .iter()
                .map(|&(key, value)| self.create_api_call("Record", &[key, value]))
                .collect();
            let expr = self.create_api_call(
                "StructWithRest",
                &[
                    self.create_api_call("Struct", &args),
                    self.f
                        .new_array_literal_expression(self.f.new_node_list(&record_exprs), false),
                ],
            );
            return Ok((expr, property_assignments.is_empty()));
        }

        // When no records and we have a hoistName, generate Schema.Class
        if records.is_empty() && !ctx.hoist_name.is_empty() {
            let fields = args[0];
            let class_decl = schema_class_declaration(
                self.f,
                &self.schema_identifier,
                |_| ModifierList::NIL, // modifiers (export added later)
                &ctx.hoist_name,
                fields,
            );

            self.push_hoisted_statement(&ctx.hoist_name, t, class_decl, &ctx.hoist_name);
            if let Some(reference) = self.hoisted_ref(t) {
                return Ok((reference, true));
            }
            return Ok((self.f.new_identifier(ctx.hoist_name.as_str()), true));
        }

        // V3: records go as additional arguments to Schema.Struct
        for (key, value) in records {
            args.push(two_property_object(self.f, "key", key, "value", value));
        }

        Ok((
            self.create_api_call("Struct", &args),
            property_assignments.is_empty(),
        ))
    }

    // Go: schemagen/structural.go:570 ScanExistingSchemas
    /// ScanExistingSchemas looks for existing schema variables in scope and records them
    /// in the hoisted map, so they can be reused instead of regenerated.
    // PORT: Go's symbol list comes from a Go map (random order); the port
    // gives the symbols in its own order (`get_symbols_in_scope`).
    pub fn scan_existing_schemas(&mut self, scope: Node) {
        let symbols = self
            .tp
            .checker
            .get_symbols_in_scope_exported(scope, SymbolFlags::VALUE);
        for sym in symbols {
            let name = self.tp.checker.symbol_to_string(sym);
            self.used_global_identifiers.insert(name.clone(), 1);
            let t = self
                .tp
                .checker
                .get_type_of_symbol_at_location(sym, self.source_file);
            if t.is_nil() {
                continue;
            }
            let Some(schema_types) = self.tp.effect_schema_types(t) else {
                continue;
            };
            let a_type = schema_types.a;
            self.hoisted_schemas.insert(
                a_type,
                HoistedEntry {
                    t: a_type,
                    ref_name: name,
                },
            );
        }
    }

    // Go: schemagen/structural.go:596 Process
    /// Process generates schema statements for one or more named types.
    /// It populates internal state and returns accumulated statements.
    // PORT: Go's `typeMap` is a Go map (random order); the refactor passes
    // one entry, so the port takes a slice of (name, type) pairs.
    pub fn process(
        &mut self,
        type_map: &[(String, TypeId)],
        scope: Node,
        is_exported: bool,
    ) -> Vec<Node> {
        for (name, t) in type_map {
            self.name_to_type.insert(name.clone(), *t);
        }

        // Scan existing schemas in scope for reuse
        self.scan_existing_schemas(scope);

        // Process each type
        // PORT: Go `processResult{requestedName, t, result}` is a tuple.
        let mut results: Vec<(String, TypeId, Node)> = Vec::new();
        for (name, t) in type_map {
            let ctx = ProcessingContext {
                depth: 0,
                max_depth: 200,
                hoist_name: String::new(),
                version: self.version,
            };
            let expr = match self.process_type(*t, &ctx) {
                Ok(expr) => expr,
                // On error, emit an error comment
                Err(err) => self.f.new_identifier(format!("undefined /* {err} */")),
            };
            results.push((name.clone(), *t, expr));
        }

        let f = self.f;

        // Add variable statements for types that were not hoisted
        for (requested_name, t, result) in results {
            if self.type_to_statement_index.contains_key(&t) {
                continue;
            }
            let var_decl = f.new_variable_declaration(
                f.new_identifier(requested_name),
                Node::NIL,
                Node::NIL,
                result,
            );
            let var_decl_list =
                f.new_variable_declaration_list(f.new_node_list(&[var_decl]), NodeFlags::CONST);
            let stmt = f.new_variable_statement(export_modifiers(f), var_decl_list);
            self.schema_statements.push(stmt);
            self.type_to_statement_index
                .insert(t, self.schema_statements.len() - 1);
        }

        // Add export modifiers to requested type statements
        if is_exported {
            let export_indices: FxHashSet<usize> = self
                .name_to_type
                .values()
                .filter_map(|t| self.type_to_statement_index.get(t).copied())
                .collect();
            for i in 0..self.schema_statements.len() {
                if !export_indices.contains(&i) {
                    continue;
                }
                let stmt = self.schema_statements[i];
                if stmt.kind() == SyntaxKind::VariableStatement {
                    // Already has export modifier from non-hoisted path; skip if so
                    if !stmt.modifiers().nodes().is_empty() {
                        continue;
                    }
                    self.schema_statements[i] =
                        f.new_variable_statement(export_modifiers(f), stmt.declaration_list());
                } else if stmt.kind() == SyntaxKind::ClassDeclaration {
                    if !stmt.modifiers().nodes().is_empty() {
                        continue;
                    }
                    self.schema_statements[i] = f.new_class_declaration(
                        export_modifiers(f),
                        stmt.name(),
                        stmt.type_parameter_list(),
                        stmt.heritage_clauses(),
                        stmt.member_list(),
                    );
                }
            }
        }

        // Set parent pointers
        for &stmt in &self.schema_statements {
            set_parent_in_children(stmt);
        }

        self.schema_statements.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::is_valid_identifier;

    // Go: schemagen/structural.go:434 validIdentifierRegex
    #[test]
    fn valid_identifier_matches_the_go_regex() {
        for name in ["a", "_a", "$", "a1", "A_b$2"] {
            assert!(is_valid_identifier(name), "{name}");
        }
        for name in ["", "1a", "a-b", "\"a b\"", "é", "a\n"] {
            assert!(!is_valid_identifier(name), "{name:?}");
        }
    }
}
