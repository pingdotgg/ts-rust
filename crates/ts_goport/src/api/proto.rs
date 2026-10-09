//! Port of internal/api/proto.go.
//!
//! PORT: Go marshals and unmarshals the tagged structs by reflection (JSON
//! v2 default rules). Here each struct gets a hand-written `MarshalerTo`
//! and, when the server decodes it, an `UnmarshalerFrom`. `proto_json!`
//! writes both from a field list in Go declaration order with the Go tag
//! options (`plain`, `omitzero`, `omitempty`). Structs whose fields have no
//! JSON impls of their own (`TypeResponse`, `DiagnosticResponse`,
//! `ProjectResponse`, `ConfigFileResponse`, `SnapshotChanges`,
//! `ProjectFileChanges`) have their impls written out below the struct.
//!
//! Go `*T` fields are `Option<T>`; Go `[]*T` fields are `Vec<T>` (no Go
//! code stores a nil element). Go `any` in `TypeResponse.Value` only holds
//! JSON primitives, so it is `LspAny`.
//!
//! PORT: tsgo#4915 generates the TS API from this Go source
//! (`_tools/gen-proto`). Its `nonnil`, `deprecated` and `internal` struct
//! tags and the `@gen-proto-*` comments on the session handlers only steer
//! that generator. They do not change the JSON, so the port leaves them out.

use crate::api::prelude::*;

use crate::api::requestfilesystem;
use crate::execute::tsc::diagnostics as diagnosticwriter;
use crate::frontend::core_ext::ProjectReference;
use crate::frontend::json::{
    JsonDecoder, JsonError, JsonToken, MarshalerTo, UnmarshalerFrom, json_unmarshal_decode,
};
use crate::frontend::json_ext::{
    AnyValue, ErrorPos, IsZero, JsonValue, LspAny, SemanticError, go_type_name, marshal_field,
    marshal_field_omitzero, marshal_opt_field, unmarshal_root, unmarshal_struct_fields,
    wrap_method_error, write_object_end, write_object_start,
};
use crate::frontend::tspath;
use crate::gostd::{GoError, errors, strconv};
use crate::ls::lsconv;
use crate::lsp::lsproto;
use crate::project;
use std::borrow::Cow;
use std::sync::LazyLock;

/// Go JSON v2 default struct arshalers for the tagged proto structs.
///
/// `marshal` writes the members in Go declaration order. Each field names
/// its Go tag option: `plain` (always written), `omitzero` (skipped when
/// `IsZero`), `omitempty` (skipped when the value marshals as `null`, `""`,
/// `{}` or `[]`). `both` adds the v2 default unmarshal: names match
/// exactly, unknown names are skipped and `null` sets the zero value.
macro_rules! proto_json {
    (marshal $ty:ident { $($field:ident : $name:literal $mode:ident),* $(,)? }) => {
        impl MarshalerTo for $ty {
            fn marshal_json_to(&self, enc: &mut String) -> Result<(), JsonError> {
                write_object_start(enc);
                let mut first = true;
                $( proto_json!(@field $mode, enc, first, $name, &self.$field); )*
                write_object_end(enc);
                Ok(())
            }
        }
    };
    (both $ty:ident { $($field:ident : $name:literal $mode:ident),* $(,)? }) => {
        proto_json!(marshal $ty { $($field : $name $mode),* });

        impl UnmarshalerFrom for $ty {
            fn unmarshal_json_from(&mut self, dec: &mut JsonDecoder<'_>) -> Result<(), JsonError> {
                let is_object = unmarshal_struct_fields(
                    dec,
                    concat!("api.", stringify!($ty)),
                    |name, dec| {
                        match name {
                            $($name => json_unmarshal_decode(dec, &mut self.$field)?,)*
                            _ => return Ok(false),
                        }
                        Ok(true)
                    },
                )?;
                if !is_object {
                    *self = $ty::default();
                }
                Ok(())
            }
        }
    };
    (@field plain, $enc:ident, $first:ident, $name:literal, $value:expr) => {
        marshal_field($enc, &mut $first, $name, $value)?;
    };
    (@field omitzero, $enc:ident, $first:ident, $name:literal, $value:expr) => {
        marshal_field_omitzero($enc, &mut $first, $name, $value)?;
    };
    (@field omitempty, $enc:ident, $first:ident, $name:literal, $value:expr) => {
        marshal_field_omitempty($enc, &mut $first, $name, $value)?;
    };
}

// Go: proto.go:20
pub static ERR_INVALID_REQUEST: LazyLock<GoError> =
    LazyLock::new(|| errors::new("api: invalid request"));
pub static ERR_CLIENT_ERROR: LazyLock<GoError> = LazyLock::new(|| errors::new("api: client error"));

// Go: proto.go:36 Method
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Method(pub Cow<'static, str>);

impl std::fmt::Display for Method {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

// PORT: `Method` is a Go string type; `BatchRequest.Method` and
// `BatchResponse.Method` (ts#63937) use the v2 string arshaler.
impl MarshalerTo for Method {
    fn marshal_json_to(&self, enc: &mut String) -> Result<(), JsonError> {
        self.0.as_ref().marshal_json_to(enc)
    }
}

impl UnmarshalerFrom for Method {
    fn unmarshal_json_from(&mut self, dec: &mut JsonDecoder<'_>) -> Result<(), JsonError> {
        let mut s = String::new();
        json_ext::unmarshal_string_as(dec, &mut s, &go_type_name::<Self>())?;
        self.0 = Cow::Owned(s);
        Ok(())
    }
}

impl IsZero for Method {
    fn is_zero(&self) -> bool {
        self.0.is_empty()
    }
}

// Go: proto.go:27
// PORT: Go named integer and string types are newtypes. Their JSON form,
// zero test and `%v` text are those of the underlying Go type.
// ts#64319: Go `project::ID` is gone; the api uses `project::ID` (JSON below).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SnapshotID(pub u64);
// ts#64299
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ModuleResolverID(pub u64);
// ts#64434
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SourceFileLeaseID(pub u64);
// ts#64158
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BuildOrchestratorID(pub u64);
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SymbolID(pub u64);
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TypeID(pub u32);
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SignatureID(pub u64);
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeHandle(pub String);

// `uint` handles use the v2 uint arshaler, `string` handles the string
// arshaler. Unmarshal errors name the Go type (`api.SnapshotID`), not the
// underlying one.
macro_rules! handle_json {
    (@unmarshal uint, $self:ident, $dec:ident) => {{
        $self.0 = json_ext::unmarshal_uint_as($dec, &go_type_name::<Self>())?;
        Ok(())
    }};
    (@unmarshal string, $self:ident, $dec:ident) => {
        json_ext::unmarshal_string_as($dec, &mut $self.0, &go_type_name::<Self>())
    };
    ($kind:ident: $($name:ident),*) => {$(
        impl MarshalerTo for $name {
            fn marshal_json_to(&self, enc: &mut String) -> Result<(), JsonError> {
                self.0.marshal_json_to(enc)
            }
        }

        impl UnmarshalerFrom for $name {
            fn unmarshal_json_from(&mut self, dec: &mut JsonDecoder<'_>) -> Result<(), JsonError> {
                handle_json!(@unmarshal $kind, self, dec)
            }
        }

        impl IsZero for $name {
            fn is_zero(&self) -> bool {
                self.0.is_zero()
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                std::fmt::Display::fmt(&self.0, f)
            }
        }
    )*};
}

handle_json!(
    uint: SnapshotID,
    ModuleResolverID,
    SourceFileLeaseID,
    BuildOrchestratorID,
    SymbolID,
    TypeID,
    SignatureID
);

// Go: proto.go nextBuildOrchestratorId (ts#64158)
static NEXT_BUILD_ORCHESTRATOR_ID: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

// Go: proto.go NewBuildOrchestratorID (ts#64158)
pub fn new_build_orchestrator_id() -> BuildOrchestratorID {
    BuildOrchestratorID(
        NEXT_BUILD_ORCHESTRATOR_ID.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1,
    )
}
handle_json!(string: NodeHandle);

// PORT: Go `project.ID` and `project.SyntheticProjectID` (ts#64319) are Go
// string types: JSON uses the v2 string arshaler, except the Go
// `SyntheticProjectID.UnmarshalJSONFrom`, ported here. The project package
// has no JSON code for them, so their trait impls live with the protocol.
impl MarshalerTo for project::ID {
    fn marshal_json_to(&self, enc: &mut String) -> Result<(), JsonError> {
        self.0.marshal_json_to(enc)
    }
}

impl UnmarshalerFrom for project::ID {
    fn unmarshal_json_from(&mut self, dec: &mut JsonDecoder<'_>) -> Result<(), JsonError> {
        json_ext::unmarshal_string_as(dec, &mut self.0, "project.ID")
    }
}

impl IsZero for project::ID {
    fn is_zero(&self) -> bool {
        self.0.is_empty()
    }
}

impl MarshalerTo for project::SyntheticProjectID {
    fn marshal_json_to(&self, enc: &mut String) -> Result<(), JsonError> {
        self.0.marshal_json_to(enc)
    }
}

// Go: project/project.go SyntheticProjectID.UnmarshalJSONFrom (ts#64319)
impl UnmarshalerFrom for project::SyntheticProjectID {
    fn unmarshal_json_from(&mut self, dec: &mut JsonDecoder<'_>) -> Result<(), JsonError> {
        let mut value = String::new();
        json_unmarshal_decode(dec, &mut value)?;
        let (parsed, ok) = project::parse_synthetic_project_id(&value);
        if !ok {
            return Err(SemanticError::method(
                ErrorPos::After,
                format!("invalid synthetic project ID: {value}"),
            ));
        }
        *self = parsed;
        Ok(())
    }
}

impl IsZero for project::SyntheticProjectID {
    fn is_zero(&self) -> bool {
        self.0.is_empty()
    }
}

// Go: proto.go:55 SymbolHandle
// PORT: `symbols` is the arena that holds `symbol` (the rule for `ast`
// functions that take a symbol). Go dereferences a nil symbol and panics.
pub fn symbol_handle(symbols: &SymbolArena, symbol: SymbolId) -> SymbolID {
    if symbol.is_nil() {
        panic!("runtime error: invalid memory address or nil pointer dereference");
    }
    SymbolID(get_symbol_id(symbols, symbol))
}

// Go: proto.go:59 TypeHandle
// PORT: Go reads `t.Id()`. A `TypeId` is the checker arena index, which
// equals the Go type id, so no checker is needed.
pub fn type_handle(t: TypeId) -> TypeID {
    if t.is_nil() {
        panic!("runtime error: invalid memory address or nil pointer dereference");
    }
    TypeID(t.0)
}

// Go: proto.go:63 SignatureHandle
// PORT: Go reads `sig.Id()`. A `SignatureId` is the checker arena index,
// which equals the Go signature id (`newSignature` numbers them from 1).
pub fn signature_handle(sig: SignatureId) -> SignatureID {
    if sig.is_nil() {
        panic!("runtime error: invalid memory address or nil pointer dereference");
    }
    SignatureID(u64::from(sig.0))
}

// ts#64319: Go `parseProjectHandle` is gone.

// Go: proto.go:56
impl Method {
    pub const RELEASE: Method = Method(Cow::Borrowed("release"));
    // ts#64434
    pub const RELEASE_SOURCE_FILE: Method = Method(Cow::Borrowed("releaseSourceFile"));
    // ts#64518
    pub const RETAIN_SOURCE_FILE: Method = Method(Cow::Borrowed("retainSourceFile"));
    pub const GET_CACHED_SOURCE_FILE: Method = Method(Cow::Borrowed("getCachedSourceFile"));

    // ts#63937
    pub const BATCH_REQUESTS: Method = Method(Cow::Borrowed("batchRequests"));
    // ts#64158
    pub const CREATE_BUILD_ORCHESTRATOR: Method = Method(Cow::Borrowed("createBuildOrchestrator"));
    pub const DISPOSE_BUILD_ORCHESTRATOR: Method =
        Method(Cow::Borrowed("disposeBuildOrchestrator"));
    pub const BUILD: Method = Method(Cow::Borrowed("build"));
    pub const BUILD_REFERENCES: Method = Method(Cow::Borrowed("buildReferences"));
    pub const CLEAN_BUILD: Method = Method(Cow::Borrowed("cleanBuild"));
    pub const CLEAN_REFERENCES: Method = Method(Cow::Borrowed("cleanReferences"));

    // tsgo#4915: MethodGetServerTiming and MethodResetServerTiming are gone;
    // the connection answers them (`ipc::timing`).
    pub const INITIALIZE: Method = Method(Cow::Borrowed("initialize"));
    // ts#64204: createSnapshot, updateSnapshot (new params) and
    // getCurrentLanguageServerSnapshot replace updateSnapshot and
    // updateTemporarySnapshot.
    pub const CREATE_SNAPSHOT: Method = Method(Cow::Borrowed("createSnapshot"));
    pub const UPDATE_SNAPSHOT: Method = Method(Cow::Borrowed("updateSnapshot"));
    pub const GET_CURRENT_LANGUAGE_SERVER_SNAPSHOT: Method =
        Method(Cow::Borrowed("getCurrentLanguageServerSnapshot"));
    // ts#64299
    pub const CREATE_MODULE_RESOLVER: Method = Method(Cow::Borrowed("createModuleResolver"));
    pub const RELEASE_MODULE_RESOLVER: Method = Method(Cow::Borrowed("releaseModuleResolver"));
    pub const RESOLVE_MODULE_NAME: Method = Method(Cow::Borrowed("resolveModuleName"));
    pub const PARSE_COMMAND_LINE: Method = Method(Cow::Borrowed("parseCommandLine"));
    pub const READ_CONFIG_FILE: Method = Method(Cow::Borrowed("readConfigFile"));
    pub const PARSE_JSON_CONFIG_FILE: Method = Method(Cow::Borrowed("parseJsonConfigFileContent"));
    pub const PARSE_CONFIG_FILE: Method = Method(Cow::Borrowed("parseConfigFile"));
    // ts#64216
    pub const CREATE_SOURCE_FILE: Method = Method(Cow::Borrowed("createSourceFile"));
    pub const CREATE_SOURCE_FILE_FROM_FILE: Method =
        Method(Cow::Borrowed("createSourceFileFromFile"));
    // tsgo#4849
    pub const TRANSPILE_MODULE: Method = Method(Cow::Borrowed("transpileModule"));
    pub const TRANSPILE_MODULE_FROM_FILE: Method = Method(Cow::Borrowed("transpileModuleFromFile"));
    pub const TRANSPILE_DECLARATION: Method = Method(Cow::Borrowed("transpileDeclaration"));
    pub const TRANSPILE_DECLARATION_FROM_FILE: Method =
        Method(Cow::Borrowed("transpileDeclarationFromFile"));
    pub const GET_DEFAULT_PROJECT_FOR_FILE: Method =
        Method(Cow::Borrowed("getDefaultProjectForFile"));
    pub const GET_SYMBOL_AT_POSITION: Method = Method(Cow::Borrowed("getSymbolAtPosition"));
    pub const GET_SYMBOLS_AT_POSITIONS: Method = Method(Cow::Borrowed("getSymbolsAtPositions"));
    pub const GET_SYMBOL_AT_LOCATION: Method = Method(Cow::Borrowed("getSymbolAtLocation"));
    pub const GET_SYMBOLS_AT_LOCATIONS: Method = Method(Cow::Borrowed("getSymbolsAtLocations"));
    pub const GET_SYMBOL_OF_SOURCE_FILE: Method = Method(Cow::Borrowed("getSymbolOfSourceFile"));
    pub const GET_SYMBOLS_OF_SOURCE_FILES: Method =
        Method(Cow::Borrowed("getSymbolsOfSourceFiles"));
    pub const GET_TYPE_OF_SYMBOL: Method = Method(Cow::Borrowed("getTypeOfSymbol"));
    pub const GET_TYPES_OF_SYMBOLS: Method = Method(Cow::Borrowed("getTypesOfSymbols"));
    pub const GET_DECLARED_TYPE_OF_SYMBOL: Method =
        Method(Cow::Borrowed("getDeclaredTypeOfSymbol"));
    // ts#63956
    pub const GET_NON_MISSING_TYPE_OF_SYMBOL: Method =
        Method(Cow::Borrowed("getNonMissingTypeOfSymbol"));
    pub const GET_SOURCE_FILE: Method = Method(Cow::Borrowed("getSourceFile"));
    pub const GET_SOURCE_FILE_NAMES: Method = Method(Cow::Borrowed("getSourceFileNames"));
    pub const GET_SOURCE_FILE_METADATA: Method = Method(Cow::Borrowed("getSourceFileMetadata"));
    // ts#64292
    pub const GET_MODE_FOR_USAGE_LOCATION: Method =
        Method(Cow::Borrowed("getModeForUsageLocation"));
    pub const GET_MODE_FOR_RESOLUTION_AT_INDEX: Method =
        Method(Cow::Borrowed("getModeForResolutionAtIndex"));
    // ts#64247
    pub const GET_RESOLVED_MODULE: Method = Method(Cow::Borrowed("getResolvedModule"));
    pub const GET_RESOLVED_MODULE_FROM_MODULE_SPECIFIER: Method =
        Method(Cow::Borrowed("getResolvedModuleFromModuleSpecifier"));
    pub const GET_RESOLVED_TYPE_REFERENCE_DIRECTIVE: Method =
        Method(Cow::Borrowed("getResolvedTypeReferenceDirective"));
    pub const GET_RESOLVED_TYPE_REFERENCE_DIRECTIVE_FROM_REFERENCE: Method = Method(Cow::Borrowed(
        "getResolvedTypeReferenceDirectiveFromTypeReferenceDirective",
    ));
    pub const GET_CONFIG_FILE_NAMES: Method = Method(Cow::Borrowed("getConfigFileNames"));
    pub const GET_CONFIG_SOURCE_FILE: Method = Method(Cow::Borrowed("getConfigSourceFile"));
    pub const RESOLVE_NAME: Method = Method(Cow::Borrowed("resolveName"));
    pub const GET_SYMBOLS_IN_SCOPE: Method = Method(Cow::Borrowed("getSymbolsInScope"));
    pub const GET_SIGNATURES_OF_TYPE: Method = Method(Cow::Borrowed("getSignaturesOfType"));
    pub const GET_RESOLVED_SIGNATURE: Method = Method(Cow::Borrowed("getResolvedSignature"));
    pub const GET_TYPE_AT_LOCATION: Method = Method(Cow::Borrowed("getTypeAtLocation"));
    pub const GET_TYPE_AT_LOCATIONS: Method = Method(Cow::Borrowed("getTypeAtLocations"));
    pub const GET_TYPE_AT_POSITION: Method = Method(Cow::Borrowed("getTypeAtPosition"));
    pub const GET_TYPES_AT_POSITIONS: Method = Method(Cow::Borrowed("getTypesAtPositions"));

    // Symbol sub-property methods
    pub const GET_PARENT_OF_SYMBOL: Method = Method(Cow::Borrowed("getParentOfSymbol"));
    pub const GET_MEMBERS_OF_SYMBOL: Method = Method(Cow::Borrowed("getMembersOfSymbol"));
    pub const GET_EXPORTS_OF_SYMBOL: Method = Method(Cow::Borrowed("getExportsOfSymbol"));
    pub const GET_EXPORT_SYMBOL_OF_SYMBOL: Method =
        Method(Cow::Borrowed("getExportSymbolOfSymbol"));

    // Type sub-property methods
    pub const GET_SYMBOL_OF_TYPE: Method = Method(Cow::Borrowed("getSymbolOfType"));
    pub const GET_TARGET_OF_TYPE: Method = Method(Cow::Borrowed("getTargetOfType"));
    pub const GET_FRESH_TYPE_OF_TYPE: Method = Method(Cow::Borrowed("getFreshTypeOfType"));
    pub const GET_REGULAR_TYPE_OF_TYPE: Method = Method(Cow::Borrowed("getRegularTypeOfType"));
    pub const GET_TYPES_OF_TYPE: Method = Method(Cow::Borrowed("getTypesOfType"));
    pub const GET_TYPE_PARAMETERS_OF_TYPE: Method =
        Method(Cow::Borrowed("getTypeParametersOfType"));
    pub const GET_OUTER_TYPE_PARAMETERS_OF_TYPE: Method =
        Method(Cow::Borrowed("getOuterTypeParametersOfType"));
    pub const GET_LOCAL_TYPE_PARAMETERS_OF_TYPE: Method =
        Method(Cow::Borrowed("getLocalTypeParametersOfType"));
    // ts#64264
    pub const GET_THIS_TYPE_OF_TYPE: Method = Method(Cow::Borrowed("getThisTypeOfType"));
    pub const GET_ALIAS_TYPE_ARGUMENTS_OF_TYPE: Method =
        Method(Cow::Borrowed("getAliasTypeArgumentsOfType"));
    pub const GET_ALIAS_SYMBOL_OF_TYPE: Method = Method(Cow::Borrowed("getAliasSymbolOfType"));
    pub const GET_OBJECT_TYPE_OF_TYPE: Method = Method(Cow::Borrowed("getObjectTypeOfType"));
    pub const GET_INDEX_TYPE_OF_TYPE: Method = Method(Cow::Borrowed("getIndexTypeOfType"));
    pub const GET_CHECK_TYPE_OF_TYPE: Method = Method(Cow::Borrowed("getCheckTypeOfType"));
    pub const GET_EXTENDS_TYPE_OF_TYPE: Method = Method(Cow::Borrowed("getExtendsTypeOfType"));
    pub const GET_BASE_TYPE_OF_TYPE: Method = Method(Cow::Borrowed("getBaseTypeOfType"));
    pub const GET_CONSTRAINT_OF_TYPE: Method = Method(Cow::Borrowed("getConstraintOfType"));
    // ts#64397
    pub const GET_TYPE_PARAMETER_OF_MAPPED_TYPE: Method =
        Method(Cow::Borrowed("getTypeParameterOfMappedType"));
    pub const GET_CONSTRAINT_TYPE_OF_MAPPED_TYPE: Method =
        Method(Cow::Borrowed("getConstraintTypeOfMappedType"));
    pub const GET_NAME_TYPE_OF_MAPPED_TYPE: Method =
        Method(Cow::Borrowed("getNameTypeOfMappedType"));
    pub const GET_TEMPLATE_TYPE_OF_MAPPED_TYPE: Method =
        Method(Cow::Borrowed("getTemplateTypeOfMappedType"));

    // Signature sub-property methods
    pub const GET_TYPE_PARAMETERS_OF_SIGNATURE: Method =
        Method(Cow::Borrowed("getTypeParametersOfSignature"));
    pub const GET_PARAMETERS_OF_SIGNATURE: Method =
        Method(Cow::Borrowed("getParametersOfSignature"));
    pub const GET_THIS_PARAMETER_OF_SIGNATURE: Method =
        Method(Cow::Borrowed("getThisParameterOfSignature"));
    pub const GET_TARGET_OF_SIGNATURE: Method = Method(Cow::Borrowed("getTargetOfSignature"));

    // Checker methods
    pub const GET_CONTEXTUAL_TYPE: Method = Method(Cow::Borrowed("getContextualType"));
    // ts#64264
    pub const GET_CONTEXTUAL_TYPE_FOR_ARGUMENT: Method =
        Method(Cow::Borrowed("getContextualTypeForArgument"));
    pub const GET_AWAITED_TYPE: Method = Method(Cow::Borrowed("getAwaitedType"));
    pub const GET_BASE_TYPE_OF_LITERAL_TYPE: Method =
        Method(Cow::Borrowed("getBaseTypeOfLiteralType"));
    pub const GET_NON_NULLABLE_TYPE: Method = Method(Cow::Borrowed("getNonNullableType"));
    pub const GET_TYPE_FROM_TYPE_NODE: Method = Method(Cow::Borrowed("getTypeFromTypeNode"));
    pub const GET_WIDENED_TYPE: Method = Method(Cow::Borrowed("getWidenedType"));
    pub const GET_PARAMETER_TYPE: Method = Method(Cow::Borrowed("getParameterType"));
    pub const GET_TYPE_PARAMETER_AT_POSITION: Method =
        Method(Cow::Borrowed("getTypeParameterAtPosition"));
    pub const IS_ARRAY_LIKE_TYPE: Method = Method(Cow::Borrowed("isArrayLikeType"));
    pub const IS_TYPE_ASSIGNABLE_TO: Method = Method(Cow::Borrowed("isTypeAssignableTo"));
    pub const GET_SHORTHAND_ASSIGNMENT_VALUE_SYMBOL: Method =
        Method(Cow::Borrowed("getShorthandAssignmentValueSymbol"));
    pub const GET_TYPE_OF_SYMBOL_AT_LOCATION: Method =
        Method(Cow::Borrowed("getTypeOfSymbolAtLocation"));
    pub const TYPE_TO_TYPE_NODE: Method = Method(Cow::Borrowed("typeToTypeNode"));
    pub const SIGNATURE_TO_SIGNATURE_DECLARATION: Method =
        Method(Cow::Borrowed("signatureToSignatureDeclaration"));
    pub const TYPE_TO_STRING: Method = Method(Cow::Borrowed("typeToString"));
    pub const IS_CONTEXT_SENSITIVE: Method = Method(Cow::Borrowed("isContextSensitive"));
    pub const GET_RETURN_TYPE_OF_SIGNATURE: Method =
        Method(Cow::Borrowed("getReturnTypeOfSignature"));
    pub const GET_REST_TYPE_OF_SIGNATURE: Method = Method(Cow::Borrowed("getRestTypeOfSignature"));
    pub const GET_TYPE_PREDICATE_OF_SIGNATURE: Method =
        Method(Cow::Borrowed("getTypePredicateOfSignature"));
    pub const GET_BASE_TYPES: Method = Method(Cow::Borrowed("getBaseTypes"));
    pub const GET_PROPERTIES_OF_TYPE: Method = Method(Cow::Borrowed("getPropertiesOfType"));
    pub const GET_APPARENT_PROPERTIES_OF_TYPE: Method =
        Method(Cow::Borrowed("getApparentPropertiesOfType"));
    pub const GET_APPARENT_TYPE: Method = Method(Cow::Borrowed("getApparentType"));
    // ts#63899
    pub const GET_REDUCED_TYPE: Method = Method(Cow::Borrowed("getReducedType"));
    pub const GET_PROPERTY_OF_TYPE: Method = Method(Cow::Borrowed("getPropertyOfType"));
    // ts#64264 (MethodGetIndexTypeOfTypeByKind is not ported: ts#64408
    // removes it)
    pub const GET_TYPE_OF_PROPERTY_OF_TYPE: Method =
        Method(Cow::Borrowed("getTypeOfPropertyOfType"));
    pub const GET_INDEX_INFO_OF_TYPE: Method = Method(Cow::Borrowed("getIndexInfoOfType"));
    pub const GET_INDEX_INFOS_OF_TYPE: Method = Method(Cow::Borrowed("getIndexInfosOfType"));
    pub const GET_CONSTRAINT_OF_TYPE_PARAMETER: Method =
        Method(Cow::Borrowed("getConstraintOfTypeParameter"));
    pub const GET_DEFAULT_FROM_TYPE_PARAMETER: Method =
        Method(Cow::Borrowed("getDefaultFromTypeParameter"));
    pub const GET_BASE_CONSTRAINT_OF_TYPE: Method =
        Method(Cow::Borrowed("getBaseConstraintOfType"));
    pub const GET_TYPE_ARGUMENTS: Method = Method(Cow::Borrowed("getTypeArguments"));
    // tsgo#3881
    pub const GET_IMPORT_ADDER_EDITS: Method = Method(Cow::Borrowed("getImportAdderEdits"));
    pub const GET_TRUE_TYPE_OF_CONDITIONAL_TYPE: Method =
        Method(Cow::Borrowed("getTrueTypeOfConditionalType"));
    pub const GET_FALSE_TYPE_OF_CONDITIONAL_TYPE: Method =
        Method(Cow::Borrowed("getFalseTypeOfConditionalType"));
    pub const GET_CONSTANT_VALUE: Method = Method(Cow::Borrowed("getConstantValue"));
    pub const GET_SIGNATURE_FROM_DECLARATION: Method =
        Method(Cow::Borrowed("getSignatureFromDeclaration"));
    pub const GET_EXPORT_SPECIFIER_LOCAL_TARGET: Method =
        Method(Cow::Borrowed("getExportSpecifierLocalTargetSymbol"));
    pub const GET_ALIASED_SYMBOL: Method = Method(Cow::Borrowed("getAliasedSymbol"));
    pub const GET_IMMEDIATE_ALIASED_SYMBOL: Method =
        Method(Cow::Borrowed("getImmediateAliasedSymbol"));
    // ts#63945
    pub const GET_TARGET_SYMBOL: Method = Method(Cow::Borrowed("getTargetSymbol"));
    // ts#64264
    pub const GET_EXPORT_SYMBOL_OF_SYMBOL_FOR_CHECKER: Method =
        Method(Cow::Borrowed("getExportSymbolOfSymbolForChecker"));
    pub const GET_FULLY_QUALIFIED_NAME: Method = Method(Cow::Borrowed("getFullyQualifiedName"));
    pub const GET_EXPORTS_OF_MODULE: Method = Method(Cow::Borrowed("getExportsOfModule"));
    pub const GET_MEMBER_IN_MODULE_EXPORTS: Method =
        Method(Cow::Borrowed("getMemberInModuleExports"));
    pub const GET_JS_DOC_TAGS: Method = Method(Cow::Borrowed("getJsDocTags"));
    pub const GET_DOCUMENTATION_COMMENT: Method = Method(Cow::Borrowed("getDocumentationComment"));
    pub const IS_ARRAY_TYPE: Method = Method(Cow::Borrowed("isArrayType"));
    // ts#64080: MethodIsTupleType is gone; TypeResponse.IsTupleType replaces it.
    // ts#63943
    pub const IS_READONLY_SYMBOL: Method = Method(Cow::Borrowed("isReadonlySymbol"));

    // Reference methods
    pub const GET_REFERENCES_TO_SYMBOL_IN_FILE: Method =
        Method(Cow::Borrowed("getReferencesToSymbolInFile"));
    pub const GET_REFERENCED_SYMBOLS_FOR_NODE: Method =
        Method(Cow::Borrowed("getReferencedSymbolsForNode"));
    pub const GET_SIGNATURE_USAGES: Method = Method(Cow::Borrowed("getSignatureUsages"));

    // Language service methods
    pub const GET_COMPLETIONS_AT_POSITION: Method =
        Method(Cow::Borrowed("getCompletionsAtPosition"));

    // Diagnostic methods
    pub const GET_SYNTACTIC_DIAGNOSTICS: Method = Method(Cow::Borrowed("getSyntacticDiagnostics"));
    pub const GET_BIND_DIAGNOSTICS: Method = Method(Cow::Borrowed("getBindDiagnostics"));
    pub const GET_SEMANTIC_DIAGNOSTICS: Method = Method(Cow::Borrowed("getSemanticDiagnostics"));
    pub const GET_SUGGESTION_DIAGNOSTICS: Method =
        Method(Cow::Borrowed("getSuggestionDiagnostics"));
    pub const GET_DECLARATION_DIAGNOSTICS: Method =
        Method(Cow::Borrowed("getDeclarationDiagnostics"));
    pub const GET_PROGRAM_DIAGNOSTICS: Method = Method(Cow::Borrowed("getProgramDiagnostics"));
    pub const GET_GLOBAL_DIAGNOSTICS: Method = Method(Cow::Borrowed("getGlobalDiagnostics"));
    pub const GET_CONFIG_FILE_PARSING_DIAGNOSTICS: Method =
        Method(Cow::Borrowed("getConfigFileParsingDiagnostics"));

    // Printer methods
    pub const PRINT_NODE: Method = Method(Cow::Borrowed("printNode"));
    pub const FORMAT_NODE_FOR_INSERTION: Method = Method(Cow::Borrowed("formatNodeForInsertion"));
    // tsgo#4699
    pub const EMIT: Method = Method(Cow::Borrowed("emit"));
    pub const EMIT_TO_STRING: Method = Method(Cow::Borrowed("emitToString"));
    pub const GET_JAVA_SCRIPT_EMIT: Method = Method(Cow::Borrowed("getJavaScriptEmit"));
    pub const GET_DECLARATION_EMIT: Method = Method(Cow::Borrowed("getDeclarationEmit"));

    // Intrinsic type getters
    pub const GET_ANY_TYPE: Method = Method(Cow::Borrowed("getAnyType"));
    pub const GET_STRING_TYPE: Method = Method(Cow::Borrowed("getStringType"));
    pub const GET_NUMBER_TYPE: Method = Method(Cow::Borrowed("getNumberType"));
    pub const GET_BOOLEAN_TYPE: Method = Method(Cow::Borrowed("getBooleanType"));
    pub const GET_VOID_TYPE: Method = Method(Cow::Borrowed("getVoidType"));
    pub const GET_UNDEFINED_TYPE: Method = Method(Cow::Borrowed("getUndefinedType"));
    pub const GET_NULL_TYPE: Method = Method(Cow::Borrowed("getNullType"));
    pub const GET_NEVER_TYPE: Method = Method(Cow::Borrowed("getNeverType"));
    pub const GET_UNKNOWN_TYPE: Method = Method(Cow::Borrowed("getUnknownType"));
    pub const GET_BIG_INT_TYPE: Method = Method(Cow::Borrowed("getBigIntType"));
    pub const GET_ES_SYMBOL_TYPE: Method = Method(Cow::Borrowed("getESSymbolType"));
    pub const GET_NON_PRIMITIVE_TYPE: Method = Method(Cow::Borrowed("getNonPrimitiveType"));

    // Well-known per-checker symbols
    pub const GET_WELL_KNOWN_SYMBOLS: Method = Method(Cow::Borrowed("getWellKnownSymbols"));

    // Well-known per-checker signatures
    pub const GET_WELL_KNOWN_SIGNATURES: Method = Method(Cow::Borrowed("getWellKnownSignatures"));

    // Profiling methods
    pub const START_CPU_PROFILE: Method = Method(Cow::Borrowed("startCPUProfile"));
    pub const STOP_CPU_PROFILE: Method = Method(Cow::Borrowed("stopCPUProfile"));
    pub const SAVE_HEAP_PROFILE: Method = Method(Cow::Borrowed("saveHeapProfile"));
}

// InitializeResponse is returned by the initialize method.
// Go: proto.go:265 InitializeResponse
#[derive(Clone, Debug, Default, PartialEq)]
pub struct InitializeResponse {
    // UseCaseSensitiveFileNames indicates whether the host file system is case-sensitive.
    pub use_case_sensitive_file_names: bool,
    // CurrentDirectory is the server's current working directory.
    pub current_directory: String,
}

proto_json!(marshal InitializeResponse {
    use_case_sensitive_file_names: "useCaseSensitiveFileNames" plain,
    current_directory: "currentDirectory" plain,
});

// DocumentIdentifier identifies a document by either a file name (plain string) or a URI object.
// On the wire it is string | { uri: string }.
//
// @example
//
// Using a file name:
//
//	project.program.getSourceFile("/path/to/file.ts");
//
// Using a URI:
//
//	project.program.getSourceFile({ uri: "file:///path/to/file.ts" });
// Go: proto.go:284 DocumentIdentifier
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DocumentIdentifier {
    pub file_name: String,
    pub uri: lsproto::DocumentUri,
}

proto_json!(marshal DocumentIdentifier {
    file_name: "fileName" omitempty,
    uri: "uri" omitempty,
});

// Go: proto.go:187 UnmarshalJSONFrom
impl UnmarshalerFrom for DocumentIdentifier {
    fn unmarshal_json_from(&mut self, dec: &mut JsonDecoder<'_>) -> Result<(), JsonError> {
        // Try reading as a plain string first
        let tok = dec.read_token()?;
        match tok.kind() {
            b'"' => {
                self.file_name = token_string(&tok);
                Ok(())
            }
            b'{' => {
                // Read the object fields
                while dec.peek_kind() != b'}' {
                    let key = dec.read_token()?;
                    let is_uri = token_string(&key) == "uri";
                    let val = dec.read_token()?;
                    if is_uri {
                        self.uri = lsproto::DocumentUri(token_string(&val));
                    }
                }
                // Consume the closing brace
                dec.read_token()?;
                Ok(())
            }
            // Go wraps the error of the method with the type (one token
            // was read).
            _ => Err(wrap_method_error::<Self>(SemanticError::method(
                ErrorPos::After,
                format!(
                    "DocumentIdentifier: expected string or object, got {}",
                    kind_string(tok.kind())
                ),
            ))),
        }
    }
}

// Go jsontext `Token.String`: the unquoted text of a string token, else the
// raw JSON text of the token.
fn token_string(tok: &JsonToken) -> String {
    match tok {
        JsonToken::Null => "null".to_string(),
        JsonToken::False => "false".to_string(),
        JsonToken::True => "true".to_string(),
        JsonToken::String(s) => s.clone(),
        JsonToken::Number(raw) => raw.clone(),
        JsonToken::BeginObject => "{".to_string(),
        JsonToken::EndObject => "}".to_string(),
        JsonToken::BeginArray => "[".to_string(),
        JsonToken::EndArray => "]".to_string(),
    }
}

// Go jsontext `Kind.String` (the `%v` text of a token kind).
fn kind_string(k: u8) -> String {
    match k {
        b'n' => "null".to_string(),
        b'f' => "false".to_string(),
        b't' => "true".to_string(),
        b'"' => "string".to_string(),
        b'0' => "number".to_string(),
        b'{' => "{".to_string(),
        b'}' => "}".to_string(),
        b'[' => "[".to_string(),
        b']' => "]".to_string(),
        _ => format!(
            "<invalid jsontext.Kind: {}>",
            crate::frontend::json_ext::quote_rune(&[k])
        ),
    }
}

impl DocumentIdentifier {
    // Go: proto.go:327 ToFileName
    pub fn to_file_name(&self) -> String {
        if !self.uri.0.is_empty() {
            return self.uri.file_name();
        }
        self.file_name.clone()
    }

    // Go: proto.go:337 ToURI
    // ToURI returns the document URI for this identifier. An explicitly provided URI
    // is returned as-is; a file name is first normalized to an absolute path against
    // cwd before being converted to a URI.
    pub fn to_uri(&self, cwd: &str) -> lsproto::DocumentUri {
        if !self.uri.0.is_empty() {
            return self.uri.clone();
        }
        lsconv::file_name_to_document_uri(&tspath::get_normalized_absolute_path(
            &self.file_name,
            cwd,
        ))
    }

    // Go: proto.go:344 ToAbsoluteFileName
    pub fn to_absolute_file_name(&self, cwd: &str) -> String {
        if !self.uri.0.is_empty() {
            return self.uri.file_name();
        }
        tspath::get_normalized_absolute_path(&self.file_name, cwd)
    }

    // Go: proto.go:351 String
    pub fn string(&self) -> String {
        if !self.uri.0.is_empty() {
            return self.uri.0.clone();
        }
        self.file_name.clone()
    }
}

// Go `%v` of a DocumentIdentifier calls its String method.
impl std::fmt::Display for DocumentIdentifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.string())
    }
}

// FileNotifications describes changes to files that have occurred on the host
// file system, used to notify the session to reload cached files and reevaluate
// tsconfig.json `include` globs. Either InvalidateAll is true (discard all caches)
// or Changed/Created/Deleted list individual documents.
// Go: proto.go FileNotifications (ts#64204; was APIFileChanges)
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FileNotifications {
    pub invalidate_all: bool,
    pub changed: Vec<DocumentIdentifier>,
    pub created: Vec<DocumentIdentifier>,
    pub deleted: Vec<DocumentIdentifier>,
}

proto_json!(both FileNotifications {
    invalidate_all: "invalidateAll" omitempty,
    changed: "changed" omitempty,
    created: "created" omitempty,
    deleted: "deleted" omitempty,
});

// SnapshotRequestChangesParams describes project, file, and program changes to apply
// while creating or updating a snapshot.
// Go: proto.go SnapshotRequestChangesParams (ts#64204, ts#64319, ts#64374)
// PORT: Go `[]*CreateSnapshotProgramParams` can hold nil elements (a JSON
// `null`), which the session rejects, so they are `Vec<Option<..>>`. Go
// `CreatePrograms != nil` (an empty array included) is `Some`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SnapshotRequestChangesParams {
    // OpenProjects lists tsconfig.json files to open/load in the new snapshot.
    pub open_projects: Vec<DocumentIdentifier>,
    // CloseProjects lists tsconfig.json files to release in the new snapshot.
    // A project is only unloaded once every API client that opened it closes it.
    pub close_projects: Vec<DocumentIdentifier>,
    // OpenFiles lists files to open in the new snapshot, mirroring LSP's
    // textDocument/didOpen. For each file, ancestor directories are searched for a
    // tsconfig that contains it; if found, that configured project is loaded and
    // becomes the file's default project. Otherwise the file is loaded into the
    // inferred project (e.g. a node_modules d.ts not in any project's import graph).
    // If a file cannot be loaded into any project, the request fails.
    pub open_files: Option<Vec<DocumentIdentifier>>,
    // CloseFiles lists files to release in the new snapshot. A file is only fully
    // closed once every API client that opened it closes it.
    pub close_files: Vec<DocumentIdentifier>,
    // CreatePrograms describes synthetic programs to create in the snapshot.
    pub create_programs: Option<Vec<Option<CreateSnapshotProgramParams>>>,
    // ReconfigurePrograms replaces the configuration of existing synthetic programs.
    pub reconfigure_programs: Vec<Option<ReconfigureSnapshotProgramParams>>,
    // RemovePrograms lists synthetic project handles to remove from the snapshot.
    pub remove_programs: Vec<project::SyntheticProjectID>,
    // EnsurePrograms identifies projects whose programs should be updated if dirty,
    // or all contained projects when true.
    pub ensure_programs: Option<EnsurePrograms>,
}

impl SnapshotRequestChangesParams {
    /// The members of Go `SnapshotRequestChangesParams`, which the structs
    /// that embed it decode inline (Go embedded struct fields).
    fn unmarshal_member(
        &mut self,
        name: &str,
        dec: &mut JsonDecoder<'_>,
    ) -> Result<bool, JsonError> {
        match name {
            "openProjects" => json_unmarshal_decode(dec, &mut self.open_projects)?,
            "closeProjects" => json_unmarshal_decode(dec, &mut self.close_projects)?,
            "openFiles" => json_unmarshal_decode(dec, &mut self.open_files)?,
            "closeFiles" => json_unmarshal_decode(dec, &mut self.close_files)?,
            "createPrograms" => json_unmarshal_decode(dec, &mut self.create_programs)?,
            "reconfigurePrograms" => json_unmarshal_decode(dec, &mut self.reconfigure_programs)?,
            "removePrograms" => json_unmarshal_decode(dec, &mut self.remove_programs)?,
            "ensurePrograms" => json_unmarshal_decode(dec, &mut self.ensure_programs)?,
            _ => return Ok(false),
        }
        Ok(true)
    }

    /// The members of Go `SnapshotRequestChangesParams`, which the structs
    /// that embed it marshal inline (Go embedded struct fields, every tag
    /// `omitempty`).
    fn marshal_members(&self, enc: &mut String, first: &mut bool) -> Result<(), JsonError> {
        marshal_field_omitempty(enc, first, "openProjects", &self.open_projects)?;
        marshal_field_omitempty(enc, first, "closeProjects", &self.close_projects)?;
        marshal_field_omitempty(enc, first, "openFiles", &self.open_files)?;
        marshal_field_omitempty(enc, first, "closeFiles", &self.close_files)?;
        marshal_field_omitempty(enc, first, "createPrograms", &self.create_programs)?;
        marshal_field_omitempty(
            enc,
            first,
            "reconfigurePrograms",
            &self.reconfigure_programs,
        )?;
        marshal_field_omitempty(enc, first, "removePrograms", &self.remove_programs)?;
        marshal_field_omitempty(enc, first, "ensurePrograms", &self.ensure_programs)
    }
}

impl UnmarshalerFrom for SnapshotRequestChangesParams {
    fn unmarshal_json_from(&mut self, dec: &mut JsonDecoder<'_>) -> Result<(), JsonError> {
        let is_object =
            unmarshal_struct_fields(dec, "api.SnapshotRequestChangesParams", |name, dec| {
                self.unmarshal_member(name, dec)
            })?;
        if !is_object {
            *self = SnapshotRequestChangesParams::default();
        }
        Ok(())
    }
}

// Go: proto.go EnsurePrograms (ts#64204, ts#64319)
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EnsurePrograms {
    pub all: bool,
    pub projects: Vec<project::ID>,
}

// Go: proto.go EnsurePrograms.UnmarshalJSONFrom (ts#64204)
impl UnmarshalerFrom for EnsurePrograms {
    fn unmarshal_json_from(&mut self, dec: &mut JsonDecoder<'_>) -> Result<(), JsonError> {
        let value = dec.read_value()?.to_vec();
        if value == b"true" {
            self.all = true;
            return Ok(());
        }
        if value.first() != Some(&b'[') {
            return Err(SemanticError::method(
                ErrorPos::After,
                "ensurePrograms must be true or an array of project IDs",
            ));
        }
        crate::frontend::json::json_unmarshal(&value, &mut self.projects, &[])
    }
}

// PORT: Go has no marshaler for `EnsurePrograms`, so it marshals by the v2
// default struct rule: the untagged fields keep their Go names.
impl MarshalerTo for EnsurePrograms {
    fn marshal_json_to(&self, enc: &mut String) -> Result<(), JsonError> {
        write_object_start(enc);
        let mut first = true;
        marshal_field(enc, &mut first, "All", &self.all)?;
        marshal_field(enc, &mut first, "Projects", &self.projects)?;
        write_object_end(enc);
        Ok(())
    }
}

// CreateSnapshotParams are the parameters for creating a new independent snapshot.
// Go: proto.go CreateSnapshotParams (ts#64204, ts#64115)
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CreateSnapshotParams {
    pub snapshot_request_changes_params: SnapshotRequestChangesParams,
    // FileNotifications describes host file system changes to invalidate while creating the snapshot.
    pub file_notifications: Option<FileNotifications>,
    // FileSystem supplies file contents and directory listings for the new snapshot.
    // A full filesystem is canonical and total. A filesystem layer is checked
    // before falling back to the base snapshot or host filesystem.
    pub file_system: Option<requestfilesystem::RequestFileSystem>,
}

impl UnmarshalerFrom for CreateSnapshotParams {
    fn unmarshal_json_from(&mut self, dec: &mut JsonDecoder<'_>) -> Result<(), JsonError> {
        let is_object = unmarshal_struct_fields(dec, "api.CreateSnapshotParams", |name, dec| {
            match name {
                "fileNotifications" => json_unmarshal_decode(dec, &mut self.file_notifications)?,
                "fileSystem" => json_unmarshal_decode(dec, &mut self.file_system)?,
                _ => {
                    return self
                        .snapshot_request_changes_params
                        .unmarshal_member(name, dec);
                }
            }
            Ok(true)
        })?;
        if !is_object {
            *self = CreateSnapshotParams::default();
        }
        Ok(())
    }
}

// PORT: the server only decodes the snapshot params, but a decoded payload
// is a Go `any` (`AnyValue`), which marshals. The marshalers below follow
// the v2 default struct rule, so they match Go reflection.
impl MarshalerTo for CreateSnapshotParams {
    fn marshal_json_to(&self, enc: &mut String) -> Result<(), JsonError> {
        write_object_start(enc);
        let mut first = true;
        self.snapshot_request_changes_params
            .marshal_members(enc, &mut first)?;
        marshal_field_omitempty(
            enc,
            &mut first,
            "fileNotifications",
            &self.file_notifications,
        )?;
        marshal_field_omitempty(enc, &mut first, "fileSystem", &self.file_system)?;
        write_object_end(enc);
        Ok(())
    }
}

// PORT: Go v2 default marshal of the `requestfilesystem` request structs
// (`CreateSnapshotParams.FileSystem`). They live here with the other
// params marshalers. Go map members come in random order; the port writes
// them in sorted key order.
impl MarshalerTo for requestfilesystem::Kind {
    fn marshal_json_to(&self, enc: &mut String) -> Result<(), JsonError> {
        self.0.as_ref().marshal_json_to(enc)
    }
}

impl MarshalerTo for requestfilesystem::RequestDirectoryEntries {
    fn marshal_json_to(&self, enc: &mut String) -> Result<(), JsonError> {
        write_object_start(enc);
        let mut first = true;
        marshal_field(enc, &mut first, "files", &self.files)?;
        marshal_field(enc, &mut first, "directories", &self.directories)?;
        write_object_end(enc);
        Ok(())
    }
}

impl MarshalerTo for requestfilesystem::RequestSymlink {
    fn marshal_json_to(&self, enc: &mut String) -> Result<(), JsonError> {
        write_object_start(enc);
        let mut first = true;
        marshal_field(enc, &mut first, "target", &self.target)?;
        marshal_field_omitempty(enc, &mut first, "host", &self.host)?;
        write_object_end(enc);
        Ok(())
    }
}

impl MarshalerTo for requestfilesystem::RequestFileSystem {
    fn marshal_json_to(&self, enc: &mut String) -> Result<(), JsonError> {
        write_object_start(enc);
        let mut first = true;
        marshal_field(enc, &mut first, "kind", &self.kind)?;
        marshal_field(enc, &mut first, "files", &SortedMapJSON(&self.files))?;
        marshal_field_omitempty(
            enc,
            &mut first,
            "directories",
            &SortedMapJSON(&self.directories),
        )?;
        marshal_field_omitempty(enc, &mut first, "symlinks", &SortedMapJSON(&self.symlinks))?;
        marshal_field_omitempty(enc, &mut first, "removedPaths", &self.removed_paths)?;
        write_object_end(enc);
        Ok(())
    }
}

/// Go v2 marshal of a `map[string]V` (a nil map writes `{}`), with the
/// members in sorted key order.
struct SortedMapJSON<'a, V>(&'a FxHashMap<String, V>);

impl<V: MarshalerTo> MarshalerTo for SortedMapJSON<'_, V> {
    fn marshal_json_to(&self, enc: &mut String) -> Result<(), JsonError> {
        let mut entries: Vec<(&String, &V)> = self.0.iter().collect();
        entries.sort_by(|a, b| a.0.cmp(b.0));
        enc.push('{');
        for (i, (k, v)) in entries.into_iter().enumerate() {
            if i > 0 {
                enc.push(',');
            }
            k.marshal_json_to(enc)?;
            enc.push(':');
            v.marshal_json_to(enc)?;
        }
        enc.push('}');
        Ok(())
    }
}

// Go: proto.go CreateSnapshotProgramParams (ts#64204, ts#64324)
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CreateSnapshotProgramParams {
    pub root_files: Vec<DocumentIdentifier>,
    pub compiler_options: CompilerOptions,
    pub options: Option<CreateProgramOptions>,
}

// PORT: the params are only decoded (`core.CompilerOptions` has no plain
// marshaler in the port), so only the decode is written.
impl UnmarshalerFrom for CreateSnapshotProgramParams {
    fn unmarshal_json_from(&mut self, dec: &mut JsonDecoder<'_>) -> Result<(), JsonError> {
        let is_object =
            unmarshal_struct_fields(dec, "api.CreateSnapshotProgramParams", |name, dec| {
                match name {
                    "rootFiles" => json_unmarshal_decode(dec, &mut self.root_files)?,
                    "compilerOptions" => json_unmarshal_decode(dec, &mut self.compiler_options)?,
                    "options" => json_unmarshal_decode(dec, &mut self.options)?,
                    _ => return Ok(false),
                }
                Ok(true)
            })?;
        if !is_object {
            *self = CreateSnapshotProgramParams::default();
        }
        Ok(())
    }
}

impl MarshalerTo for CreateSnapshotProgramParams {
    fn marshal_json_to(&self, enc: &mut String) -> Result<(), JsonError> {
        write_object_start(enc);
        let mut first = true;
        marshal_field(enc, &mut first, "rootFiles", &self.root_files)?;
        marshal_field(
            enc,
            &mut first,
            "compilerOptions",
            &CompilerOptionsJSON(&self.compiler_options),
        )?;
        marshal_field_omitempty(enc, &mut first, "options", &self.options)?;
        write_object_end(enc);
        Ok(())
    }
}

// Go: proto.go ReconfigureSnapshotProgramParams (ts#64204, ts#64319, ts#64324)
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ReconfigureSnapshotProgramParams {
    pub id: project::SyntheticProjectID,
    pub root_files: Vec<DocumentIdentifier>,
    pub compiler_options: CompilerOptions,
    pub options: Option<CreateProgramOptions>,
}

impl UnmarshalerFrom for ReconfigureSnapshotProgramParams {
    fn unmarshal_json_from(&mut self, dec: &mut JsonDecoder<'_>) -> Result<(), JsonError> {
        let is_object =
            unmarshal_struct_fields(dec, "api.ReconfigureSnapshotProgramParams", |name, dec| {
                match name {
                    "id" => json_unmarshal_decode(dec, &mut self.id)?,
                    "rootFiles" => json_unmarshal_decode(dec, &mut self.root_files)?,
                    "compilerOptions" => json_unmarshal_decode(dec, &mut self.compiler_options)?,
                    "options" => json_unmarshal_decode(dec, &mut self.options)?,
                    _ => return Ok(false),
                }
                Ok(true)
            })?;
        if !is_object {
            *self = ReconfigureSnapshotProgramParams::default();
        }
        Ok(())
    }
}

impl MarshalerTo for ReconfigureSnapshotProgramParams {
    fn marshal_json_to(&self, enc: &mut String) -> Result<(), JsonError> {
        write_object_start(enc);
        let mut first = true;
        marshal_field(enc, &mut first, "id", &self.id)?;
        marshal_field(enc, &mut first, "rootFiles", &self.root_files)?;
        marshal_field(
            enc,
            &mut first,
            "compilerOptions",
            &CompilerOptionsJSON(&self.compiler_options),
        )?;
        marshal_field_omitempty(enc, &mut first, "options", &self.options)?;
        write_object_end(enc);
        Ok(())
    }
}

// UpdateSnapshotParams are the parameters for deriving a snapshot from an existing one.
// Go: proto.go UpdateSnapshotParams (ts#64204)
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UpdateSnapshotParams {
    pub snapshot: SnapshotID,
    pub changes: Option<CreateSnapshotParams>,
}

proto_json!(both UpdateSnapshotParams {
    snapshot: "snapshot" plain,
    changes: "changes" omitempty,
});

// Go: proto.go GetCurrentLanguageServerSnapshotParams (ts#64204)
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GetCurrentLanguageServerSnapshotParams {
    pub base_snapshot: SnapshotID,
    pub changes: Option<LanguageServerSnapshotChanges>,
}

proto_json!(both GetCurrentLanguageServerSnapshotParams {
    base_snapshot: "baseSnapshot" omitempty,
    changes: "changes" omitempty,
});

// LanguageServerSnapshotChanges describes API-driven changes to adopt into the
// language server's canonical state.
// Go: proto.go LanguageServerSnapshotChanges (ts#64204)
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LanguageServerSnapshotChanges {
    pub snapshot_request_changes_params: SnapshotRequestChangesParams,
}

impl UnmarshalerFrom for LanguageServerSnapshotChanges {
    fn unmarshal_json_from(&mut self, dec: &mut JsonDecoder<'_>) -> Result<(), JsonError> {
        let is_object =
            unmarshal_struct_fields(dec, "api.LanguageServerSnapshotChanges", |name, dec| {
                self.snapshot_request_changes_params
                    .unmarshal_member(name, dec)
            })?;
        if !is_object {
            *self = LanguageServerSnapshotChanges::default();
        }
        Ok(())
    }
}

impl MarshalerTo for LanguageServerSnapshotChanges {
    fn marshal_json_to(&self, enc: &mut String) -> Result<(), JsonError> {
        write_object_start(enc);
        let mut first = true;
        self.snapshot_request_changes_params
            .marshal_members(enc, &mut first)?;
        write_object_end(enc);
        Ok(())
    }
}

// PORT: Go decodes `core.ProjectReference` by reflection (the fields
// `path`, `originalPath` and `circular`); only the api decodes it.
impl UnmarshalerFrom for ProjectReference {
    fn unmarshal_json_from(&mut self, dec: &mut JsonDecoder<'_>) -> Result<(), JsonError> {
        let is_object = unmarshal_struct_fields(dec, "core.ProjectReference", |name, dec| {
            match name {
                "path" => json_unmarshal_decode(dec, &mut self.path)?,
                "originalPath" => json_unmarshal_decode(dec, &mut self.original_path)?,
                "circular" => json_unmarshal_decode(dec, &mut self.circular)?,
                _ => return Ok(false),
            }
            Ok(true)
        })?;
        if !is_object {
            *self = ProjectReference::default();
        }
        Ok(())
    }
}

// Go: proto.go CreateProgramOptions (ts#63950, ts#64324)
// PORT: Go `[]*core.ProjectReference` elements are never nil here.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CreateProgramOptions {
    pub project_references: Vec<ProjectReference>,
    pub config_file_parsing_diagnostics: Vec<DiagnosticResponse>,
    // ts#64299
    pub module_resolver: ModuleResolverID,
}

impl UnmarshalerFrom for CreateProgramOptions {
    fn unmarshal_json_from(&mut self, dec: &mut JsonDecoder<'_>) -> Result<(), JsonError> {
        let is_object = unmarshal_struct_fields(dec, "api.CreateProgramOptions", |name, dec| {
            match name {
                "projectReferences" => json_unmarshal_decode(dec, &mut self.project_references)?,
                "configFileParsingDiagnostics" => {
                    json_unmarshal_decode(dec, &mut self.config_file_parsing_diagnostics)?
                }
                "moduleResolver" => json_unmarshal_decode(dec, &mut self.module_resolver)?,
                _ => return Ok(false),
            }
            Ok(true)
        })?;
        if !is_object {
            *self = CreateProgramOptions::default();
        }
        Ok(())
    }
}

proto_json!(marshal CreateProgramOptions {
    project_references: "projectReferences" omitempty,
    config_file_parsing_diagnostics: "configFileParsingDiagnostics" omitempty,
    module_resolver: "moduleResolver" omitempty,
});

// Go: proto.go ModuleResolutionFallback (ts#64299)
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ModuleResolutionFallback(pub Cow<'static, str>);

impl ModuleResolutionFallback {
    pub const RESOLVE: ModuleResolutionFallback =
        ModuleResolutionFallback(Cow::Borrowed("resolve"));
    pub const UNRESOLVED: ModuleResolutionFallback =
        ModuleResolutionFallback(Cow::Borrowed("unresolved"));
}

impl UnmarshalerFrom for ModuleResolutionFallback {
    fn unmarshal_json_from(&mut self, dec: &mut JsonDecoder<'_>) -> Result<(), JsonError> {
        let mut s = String::new();
        json_ext::unmarshal_string_as(dec, &mut s, "api.ModuleResolutionFallback")?;
        self.0 = Cow::Owned(s);
        Ok(())
    }
}

impl MarshalerTo for ModuleResolutionFallback {
    fn marshal_json_to(&self, enc: &mut String) -> Result<(), JsonError> {
        self.0.as_ref().marshal_json_to(enc)
    }
}

// Go: proto.go ResolutionMode (ts#64299): `type ResolutionMode core.ModuleKind`.
// PORT: named `ResolutionMode` as in Go; it is not the core alias
// (`crate::options::ResolutionMode`), which api code outside this file names
// `ModuleKind`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct ResolutionMode(pub i32);

impl MarshalerTo for ResolutionMode {
    fn marshal_json_to(&self, enc: &mut String) -> Result<(), JsonError> {
        self.0.marshal_json_to(enc)
    }
}

impl UnmarshalerFrom for ResolutionMode {
    fn unmarshal_json_from(&mut self, dec: &mut JsonDecoder<'_>) -> Result<(), JsonError> {
        self.0
            .unmarshal_json_from(dec)
            .map_err(|err| match SemanticError::of(&err) {
                Some(mut s) => {
                    s.go_type = "api.ResolutionMode".to_string();
                    s.into_json_error()
                }
                None => err,
            })
    }
}

// Go: proto.go ModuleResolutionSpec (ts#64299)
// PORT: Go `[]*ModuleResolutionEntry` elements can be nil (JSON `null`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ModuleResolutionSpec {
    pub fallback: ModuleResolutionFallback,
    pub entries: Vec<Option<ModuleResolutionEntry>>,
}

impl UnmarshalerFrom for ModuleResolutionSpec {
    fn unmarshal_json_from(&mut self, dec: &mut JsonDecoder<'_>) -> Result<(), JsonError> {
        let is_object = unmarshal_struct_fields(dec, "api.ModuleResolutionSpec", |name, dec| {
            match name {
                "fallback" => json_unmarshal_decode(dec, &mut self.fallback)?,
                "entries" => json_unmarshal_decode(dec, &mut self.entries)?,
                _ => return Ok(false),
            }
            Ok(true)
        })?;
        if !is_object {
            *self = ModuleResolutionSpec::default();
        }
        Ok(())
    }
}

proto_json!(marshal ModuleResolutionSpec {
    fallback: "fallback" plain,
    entries: "entries" plain,
});

// Go: proto.go ModuleResolutionEntry (ts#64299)
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ModuleResolutionEntry {
    pub module_name: String,
    pub containing_directory: Option<DocumentIdentifier>,
    pub resolution_mode: Option<ResolutionMode>,
    pub result: Option<StaticModuleResolution>,
}

impl UnmarshalerFrom for ModuleResolutionEntry {
    fn unmarshal_json_from(&mut self, dec: &mut JsonDecoder<'_>) -> Result<(), JsonError> {
        let is_object = unmarshal_struct_fields(dec, "api.ModuleResolutionEntry", |name, dec| {
            match name {
                "moduleName" => json_unmarshal_decode(dec, &mut self.module_name)?,
                "containingDirectory" => {
                    json_unmarshal_decode(dec, &mut self.containing_directory)?
                }
                "resolutionMode" => json_unmarshal_decode(dec, &mut self.resolution_mode)?,
                "result" => json_unmarshal_decode(dec, &mut self.result)?,
                _ => return Ok(false),
            }
            Ok(true)
        })?;
        if !is_object {
            *self = ModuleResolutionEntry::default();
        }
        Ok(())
    }
}

proto_json!(marshal ModuleResolutionEntry {
    module_name: "moduleName" plain,
    containing_directory: "containingDirectory" omitempty,
    resolution_mode: "resolutionMode" omitempty,
    result: "result" plain,
});

// Go: proto.go StaticModuleResolution (ts#64299)
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StaticModuleResolution {
    pub resolved_file_name: Option<DocumentIdentifier>,
    pub original_path: Option<DocumentIdentifier>,
    pub package_id: Option<PackageId>,
}

impl UnmarshalerFrom for StaticModuleResolution {
    fn unmarshal_json_from(&mut self, dec: &mut JsonDecoder<'_>) -> Result<(), JsonError> {
        let is_object = unmarshal_struct_fields(dec, "api.StaticModuleResolution", |name, dec| {
            match name {
                "resolvedFileName" => json_unmarshal_decode(dec, &mut self.resolved_file_name)?,
                "originalPath" => json_unmarshal_decode(dec, &mut self.original_path)?,
                "packageId" => json_unmarshal_decode(dec, &mut self.package_id)?,
                _ => return Ok(false),
            }
            Ok(true)
        })?;
        if !is_object {
            *self = StaticModuleResolution::default();
        }
        Ok(())
    }
}

proto_json!(marshal StaticModuleResolution {
    resolved_file_name: "resolvedFileName" omitempty,
    original_path: "originalPath" omitempty,
    package_id: "packageId" omitempty,
});

// Go: proto.go CreateModuleResolverParams (ts#64299)
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CreateModuleResolverParams {
    pub compiler_options: CompilerOptions,
    pub module_resolutions: Option<ModuleResolutionSpec>,
    pub resolve_module_name_callback: String,
}

impl UnmarshalerFrom for CreateModuleResolverParams {
    fn unmarshal_json_from(&mut self, dec: &mut JsonDecoder<'_>) -> Result<(), JsonError> {
        let is_object =
            unmarshal_struct_fields(dec, "api.CreateModuleResolverParams", |name, dec| {
                match name {
                    "compilerOptions" => json_unmarshal_decode(dec, &mut self.compiler_options)?,
                    "moduleResolutions" => {
                        json_unmarshal_decode(dec, &mut self.module_resolutions)?
                    }
                    "resolveModuleNameCallback" => {
                        json_unmarshal_decode(dec, &mut self.resolve_module_name_callback)?
                    }
                    _ => return Ok(false),
                }
                Ok(true)
            })?;
        if !is_object {
            *self = CreateModuleResolverParams::default();
        }
        Ok(())
    }
}

impl MarshalerTo for CreateModuleResolverParams {
    fn marshal_json_to(&self, enc: &mut String) -> Result<(), JsonError> {
        write_object_start(enc);
        let mut first = true;
        marshal_field(
            enc,
            &mut first,
            "compilerOptions",
            &CompilerOptionsJSON(&self.compiler_options),
        )?;
        marshal_field_omitempty(
            enc,
            &mut first,
            "moduleResolutions",
            &self.module_resolutions,
        )?;
        marshal_field_omitempty(
            enc,
            &mut first,
            "resolveModuleNameCallback",
            &self.resolve_module_name_callback,
        )?;
        write_object_end(enc);
        Ok(())
    }
}

// Go: proto.go ReleaseModuleResolverParams (ts#64299)
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ReleaseModuleResolverParams {
    pub resolver: ModuleResolverID,
}

proto_json!(both ReleaseModuleResolverParams {
    resolver: "resolver" plain,
});

// Go: proto.go ResolveModuleNameParams (ts#64299)
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ResolveModuleNameParams {
    pub snapshot: SnapshotID,
    pub in_progress_snapshot: u64,
    pub resolver: ModuleResolverID,
    pub module_name: String,
    pub containing_directory: DocumentIdentifier,
    pub resolution_mode: Option<ResolutionMode>,
}

impl UnmarshalerFrom for ResolveModuleNameParams {
    fn unmarshal_json_from(&mut self, dec: &mut JsonDecoder<'_>) -> Result<(), JsonError> {
        let is_object =
            unmarshal_struct_fields(dec, "api.ResolveModuleNameParams", |name, dec| {
                match name {
                    "snapshot" => json_unmarshal_decode(dec, &mut self.snapshot)?,
                    "inProgressSnapshot" => {
                        json_unmarshal_decode(dec, &mut self.in_progress_snapshot)?
                    }
                    "resolver" => json_unmarshal_decode(dec, &mut self.resolver)?,
                    "moduleName" => json_unmarshal_decode(dec, &mut self.module_name)?,
                    "containingDirectory" => {
                        json_unmarshal_decode(dec, &mut self.containing_directory)?
                    }
                    "resolutionMode" => json_unmarshal_decode(dec, &mut self.resolution_mode)?,
                    _ => return Ok(false),
                }
                Ok(true)
            })?;
        if !is_object {
            *self = ResolveModuleNameParams::default();
        }
        Ok(())
    }
}

proto_json!(marshal ResolveModuleNameParams {
    snapshot: "snapshot" omitempty,
    in_progress_snapshot: "inProgressSnapshot" omitempty,
    resolver: "resolver" plain,
    module_name: "moduleName" plain,
    containing_directory: "containingDirectory" plain,
    resolution_mode: "resolutionMode" omitempty,
});

// Go: proto.go ResolveModuleNameCallbackParams (ts#64299)
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ResolveModuleNameCallbackParams {
    pub module_name: String,
    pub containing_directory: String,
    pub resolution_mode: Option<ResolutionMode>,
    pub snapshot: Option<SnapshotID>,
    pub in_progress_snapshot: Option<u64>,
}

proto_json!(marshal ResolveModuleNameCallbackParams {
    module_name: "moduleName" plain,
    containing_directory: "containingDirectory" plain,
    resolution_mode: "resolutionMode" omitempty,
    snapshot: "snapshot" omitempty,
    in_progress_snapshot: "inProgressSnapshot" omitempty,
});

// Go: proto.go ResolveModuleNameResult (ts#64299)
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ResolveModuleNameResult {
    pub resolved_module: Option<ResolvedModule>,
    // Trace is provided when compilerOptions.traceResolution is true.
    pub trace: Vec<String>,
}

proto_json!(marshal ResolveModuleNameResult {
    resolved_module: "resolvedModule" omitempty,
    trace: "trace" omitempty,
});

// ProjectFileChanges describes what source files changed within a single project.
// Go: proto.go:528 ProjectFileChanges
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ProjectFileChanges {
    // ChangedFiles lists source file paths whose content differs.
    pub changed_files: Vec<tspath::Path>,
    // DeletedFiles lists source file paths removed from the project's program.
    pub deleted_files: Vec<tspath::Path>,
}

impl MarshalerTo for ProjectFileChanges {
    fn marshal_json_to(&self, enc: &mut String) -> Result<(), JsonError> {
        // PORT: `tspath.Path` is a Go string type; it writes as a string.
        let changed_files: Vec<&str> = self.changed_files.iter().map(|p| p.as_str()).collect();
        let deleted_files: Vec<&str> = self.deleted_files.iter().map(|p| p.as_str()).collect();
        write_object_start(enc);
        let mut first = true;
        marshal_field_omitempty(enc, &mut first, "changedFiles", &changed_files)?;
        marshal_field_omitempty(enc, &mut first, "deletedFiles", &deleted_files)?;
        write_object_end(enc);
        Ok(())
    }
}

// SnapshotChanges describes what changed between a response base and a new
// snapshot. Changes are reported per-project so clients
// can track cache refs at the (snapshot, project) level.
// Go: proto.go SnapshotChanges (ts#64204, ts#64319)
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SnapshotChanges {
    // ChangedProjects maps project handles to the file changes within that project.
    // Projects not listed here (and not in RemovedProjects) are unchanged.
    pub changed_projects: IndexMap<project::ID, ProjectFileChanges>,
    // RemovedProjects lists project handles that were present in the previous
    // snapshot but absent from the new one.
    pub removed_projects: Vec<project::ID>,
}

impl MarshalerTo for SnapshotChanges {
    fn marshal_json_to(&self, enc: &mut String) -> Result<(), JsonError> {
        write_object_start(enc);
        let mut first = true;
        marshal_field_omitempty(
            enc,
            &mut first,
            "changedProjects",
            &ProjectChangesJSON(&self.changed_projects),
        )?;
        marshal_field_omitempty(enc, &mut first, "removedProjects", &self.removed_projects)?;
        write_object_end(enc);
        Ok(())
    }
}

// Go v2 map marshal of `map[project.ID]*ProjectFileChanges`: an object with
// the handles as names.
// PORT: Go map order is random; the port writes insertion order. Go values
// are never nil pointers, so the map holds values.
struct ProjectChangesJSON<'a>(&'a IndexMap<project::ID, ProjectFileChanges>);

impl MarshalerTo for ProjectChangesJSON<'_> {
    fn marshal_json_to(&self, enc: &mut String) -> Result<(), JsonError> {
        enc.push('{');
        for (i, (k, v)) in self.0.iter().enumerate() {
            if i > 0 {
                enc.push(',');
            }
            k.marshal_json_to(enc)?;
            enc.push(':');
            v.marshal_json_to(enc)?;
        }
        enc.push('}');
        Ok(())
    }
}

// CreateSnapshotResponse is returned by createSnapshot.
// Go: proto.go CreateSnapshotResponse (ts#64204; was UpdateSnapshotResponse)
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CreateSnapshotResponse {
    // Snapshot is the handle for the newly created snapshot.
    pub snapshot: SnapshotID,
    // Projects contains all projects when no response base was supplied, or only
    // projects added or replaced relative to that base.
    pub projects: Vec<ProjectResponse>,
    // Changes describes source file differences from the response base.
    pub changes: Option<SnapshotChanges>,
    // Operation describes results correlated with the request that produced the snapshot.
    pub operation: Option<SnapshotOperationResponse>,
}

proto_json!(marshal CreateSnapshotResponse {
    snapshot: "snapshot" plain,
    projects: "projects" plain,
    changes: "changes" omitempty,
    operation: "operation" plain,
});

// Go: proto.go SnapshotOperationResponse (ts#64204, ts#64319)
// PORT: Go `*[]T` with `omitzero`: `None` is the nil pointer (omitted);
// `Some(vec![])` writes `[]`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SnapshotOperationResponse {
    pub created_programs: Option<Vec<project::SyntheticProjectID>>,
    pub opened_files: Option<Vec<OpenedFileOperationResult>>,
}

impl MarshalerTo for SnapshotOperationResponse {
    fn marshal_json_to(&self, enc: &mut String) -> Result<(), JsonError> {
        write_object_start(enc);
        let mut first = true;
        marshal_opt_field(enc, &mut first, "createdPrograms", &self.created_programs)?;
        marshal_opt_field(enc, &mut first, "openedFiles", &self.opened_files)?;
        write_object_end(enc);
        Ok(())
    }
}

// Go: proto.go OpenedFileOperationResult (ts#64204, ts#64319)
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OpenedFileOperationResult {
    pub project: project::ID,
}

proto_json!(marshal OpenedFileOperationResult {
    project: "project" plain,
});

/// Go `func([]byte) (any, error)` in `unmarshalers`.
pub type Unmarshaler = fn(&[u8]) -> Result<Option<Box<dyn AnyValue>>, GoError>;

// Go: proto.go:569 unmarshalers
pub static UNMARSHALERS: LazyLock<FxHashMap<Method, Unmarshaler>> = LazyLock::new(|| {
    let mut m: FxHashMap<Method, Unmarshaler> = FxHashMap::default();
    // ts#63937
    m.insert(
        Method::BATCH_REQUESTS,
        unmarshaller_for::<BatchRequestsParams>,
    );
    m.insert(Method::RELEASE, unmarshaller_for::<ReleaseParams>);
    // ts#64158
    m.insert(
        Method::CREATE_BUILD_ORCHESTRATOR,
        unmarshaller_for::<CreateBuildOrchestratorParams>,
    );
    m.insert(
        Method::DISPOSE_BUILD_ORCHESTRATOR,
        unmarshaller_for::<DisposeBuildOrchestratorParams>,
    );
    m.insert(Method::BUILD, unmarshaller_for::<BuildParams>);
    m.insert(Method::BUILD_REFERENCES, unmarshaller_for::<BuildParams>);
    m.insert(Method::CLEAN_BUILD, unmarshaller_for::<CleanBuildParams>);
    m.insert(
        Method::CLEAN_REFERENCES,
        unmarshaller_for::<CleanBuildParams>,
    );
    // ts#64434
    m.insert(
        Method::RELEASE_SOURCE_FILE,
        unmarshaller_for::<ReleaseSourceFileParams>,
    );
    // ts#64518
    m.insert(
        Method::RETAIN_SOURCE_FILE,
        unmarshaller_for::<RetainSourceFileParams>,
    );
    m.insert(
        Method::GET_CACHED_SOURCE_FILE,
        unmarshaller_for::<GetCachedSourceFileParams>,
    );
    m.insert(Method::INITIALIZE, no_params);
    // ts#64204
    m.insert(
        Method::CREATE_SNAPSHOT,
        unmarshaller_for::<CreateSnapshotParams>,
    );
    m.insert(
        Method::UPDATE_SNAPSHOT,
        unmarshaller_for::<UpdateSnapshotParams>,
    );
    m.insert(
        Method::GET_CURRENT_LANGUAGE_SERVER_SNAPSHOT,
        unmarshaller_for::<GetCurrentLanguageServerSnapshotParams>,
    );
    // ts#64299
    m.insert(
        Method::CREATE_MODULE_RESOLVER,
        unmarshaller_for::<CreateModuleResolverParams>,
    );
    m.insert(
        Method::RELEASE_MODULE_RESOLVER,
        unmarshaller_for::<ReleaseModuleResolverParams>,
    );
    m.insert(
        Method::RESOLVE_MODULE_NAME,
        unmarshaller_for::<ResolveModuleNameParams>,
    );
    m.insert(
        Method::PARSE_COMMAND_LINE,
        unmarshaller_for::<ParseCommandLineParams>,
    );
    m.insert(
        Method::READ_CONFIG_FILE,
        unmarshaller_for::<ReadConfigFileParams>,
    );
    m.insert(
        Method::PARSE_JSON_CONFIG_FILE,
        unmarshaller_for::<ParseJsonConfigFileContentParams>,
    );
    m.insert(
        Method::PARSE_CONFIG_FILE,
        unmarshaller_for::<ParseConfigFileParams>,
    );
    // ts#64216
    m.insert(
        Method::CREATE_SOURCE_FILE,
        unmarshaller_for::<CreateSourceFileParams>,
    );
    m.insert(
        Method::CREATE_SOURCE_FILE_FROM_FILE,
        unmarshaller_for::<CreateSourceFileFromFileParams>,
    );
    // tsgo#4849
    m.insert(
        Method::TRANSPILE_MODULE,
        unmarshaller_for::<TranspileParams>,
    );
    m.insert(
        Method::TRANSPILE_MODULE_FROM_FILE,
        unmarshaller_for::<TranspileFromFileParams>,
    );
    m.insert(
        Method::TRANSPILE_DECLARATION,
        unmarshaller_for::<TranspileParams>,
    );
    m.insert(
        Method::TRANSPILE_DECLARATION_FROM_FILE,
        unmarshaller_for::<TranspileFromFileParams>,
    );
    m.insert(
        Method::GET_DEFAULT_PROJECT_FOR_FILE,
        unmarshaller_for::<GetDefaultProjectForFileParams>,
    );
    m.insert(
        Method::GET_SOURCE_FILE,
        unmarshaller_for::<GetSourceFileParams>,
    );
    m.insert(
        Method::GET_SOURCE_FILE_NAMES,
        unmarshaller_for::<GetSourceFileNamesParams>,
    );
    m.insert(
        Method::GET_SOURCE_FILE_METADATA,
        unmarshaller_for::<GetSourceFileParams>,
    );
    // ts#64292
    m.insert(
        Method::GET_MODE_FOR_USAGE_LOCATION,
        unmarshaller_for::<GetModeForUsageLocationParams>,
    );
    m.insert(
        Method::GET_MODE_FOR_RESOLUTION_AT_INDEX,
        unmarshaller_for::<GetModeForResolutionAtIndexParams>,
    );
    // ts#64247
    m.insert(
        Method::GET_RESOLVED_MODULE,
        unmarshaller_for::<GetResolvedModuleParams>,
    );
    m.insert(
        Method::GET_RESOLVED_MODULE_FROM_MODULE_SPECIFIER,
        unmarshaller_for::<GetResolvedModuleFromModuleSpecifierParams>,
    );
    m.insert(
        Method::GET_RESOLVED_TYPE_REFERENCE_DIRECTIVE,
        unmarshaller_for::<GetResolvedTypeReferenceDirectiveParams>,
    );
    m.insert(
        Method::GET_RESOLVED_TYPE_REFERENCE_DIRECTIVE_FROM_REFERENCE,
        unmarshaller_for::<GetResolvedTypeReferenceDirectiveFromReferenceParams>,
    );
    m.insert(
        Method::GET_CONFIG_FILE_NAMES,
        unmarshaller_for::<GetProjectDiagnosticsParams>,
    );
    m.insert(
        Method::GET_CONFIG_SOURCE_FILE,
        unmarshaller_for::<GetSourceFileParams>,
    );
    m.insert(
        Method::GET_SYMBOL_AT_POSITION,
        unmarshaller_for::<GetSymbolAtPositionParams>,
    );
    m.insert(
        Method::GET_SYMBOLS_AT_POSITIONS,
        unmarshaller_for::<GetSymbolsAtPositionsParams>,
    );
    m.insert(
        Method::GET_SYMBOL_AT_LOCATION,
        unmarshaller_for::<GetSymbolAtLocationParams>,
    );
    m.insert(
        Method::GET_SYMBOLS_AT_LOCATIONS,
        unmarshaller_for::<GetSymbolsAtLocationsParams>,
    );
    m.insert(
        Method::GET_SYMBOL_OF_SOURCE_FILE,
        unmarshaller_for::<GetSymbolOfSourceFileParams>,
    );
    m.insert(
        Method::GET_SYMBOLS_OF_SOURCE_FILES,
        unmarshaller_for::<GetSymbolsOfSourceFilesParams>,
    );
    m.insert(
        Method::GET_TYPE_OF_SYMBOL,
        unmarshaller_for::<GetTypeOfSymbolParams>,
    );
    m.insert(
        Method::GET_TYPES_OF_SYMBOLS,
        unmarshaller_for::<GetTypesOfSymbolsParams>,
    );
    m.insert(
        Method::GET_DECLARED_TYPE_OF_SYMBOL,
        unmarshaller_for::<GetTypeOfSymbolParams>,
    );
    // ts#63956
    m.insert(
        Method::GET_NON_MISSING_TYPE_OF_SYMBOL,
        unmarshaller_for::<GetTypeOfSymbolParams>,
    );
    m.insert(Method::RESOLVE_NAME, unmarshaller_for::<ResolveNameParams>);
    m.insert(
        Method::GET_SYMBOLS_IN_SCOPE,
        unmarshaller_for::<GetSymbolsInScopeParams>,
    );
    m.insert(
        Method::GET_SIGNATURES_OF_TYPE,
        unmarshaller_for::<GetSignaturesOfTypeParams>,
    );
    m.insert(
        Method::GET_RESOLVED_SIGNATURE,
        unmarshaller_for::<GetResolvedSignatureParams>,
    );
    m.insert(
        Method::GET_TYPE_AT_LOCATION,
        unmarshaller_for::<GetTypeAtLocationParams>,
    );
    m.insert(
        Method::GET_TYPE_AT_LOCATIONS,
        unmarshaller_for::<GetTypeAtLocationsParams>,
    );
    m.insert(
        Method::GET_TYPE_AT_POSITION,
        unmarshaller_for::<GetTypeAtPositionParams>,
    );
    m.insert(
        Method::GET_TYPES_AT_POSITIONS,
        unmarshaller_for::<GetTypesAtPositionsParams>,
    );

    m.insert(
        Method::GET_PARENT_OF_SYMBOL,
        unmarshaller_for::<GetSymbolPropertyParams>,
    );
    m.insert(
        Method::GET_MEMBERS_OF_SYMBOL,
        unmarshaller_for::<GetSymbolPropertyParams>,
    );
    m.insert(
        Method::GET_EXPORTS_OF_SYMBOL,
        unmarshaller_for::<GetSymbolPropertyParams>,
    );
    m.insert(
        Method::GET_EXPORT_SYMBOL_OF_SYMBOL,
        unmarshaller_for::<GetSymbolPropertyParams>,
    );

    m.insert(
        Method::GET_SYMBOL_OF_TYPE,
        unmarshaller_for::<GetTypePropertyParams>,
    );
    m.insert(
        Method::GET_TARGET_OF_TYPE,
        unmarshaller_for::<GetTypePropertyParams>,
    );
    m.insert(
        Method::GET_FRESH_TYPE_OF_TYPE,
        unmarshaller_for::<GetTypePropertyParams>,
    );
    m.insert(
        Method::GET_REGULAR_TYPE_OF_TYPE,
        unmarshaller_for::<GetTypePropertyParams>,
    );
    m.insert(
        Method::GET_TYPES_OF_TYPE,
        unmarshaller_for::<GetTypePropertyParams>,
    );
    m.insert(
        Method::GET_TYPE_PARAMETERS_OF_TYPE,
        unmarshaller_for::<GetTypePropertyParams>,
    );
    m.insert(
        Method::GET_OUTER_TYPE_PARAMETERS_OF_TYPE,
        unmarshaller_for::<GetTypePropertyParams>,
    );
    m.insert(
        Method::GET_LOCAL_TYPE_PARAMETERS_OF_TYPE,
        unmarshaller_for::<GetTypePropertyParams>,
    );
    // ts#64264
    m.insert(
        Method::GET_THIS_TYPE_OF_TYPE,
        unmarshaller_for::<GetTypePropertyParams>,
    );
    m.insert(
        Method::GET_ALIAS_TYPE_ARGUMENTS_OF_TYPE,
        unmarshaller_for::<GetTypePropertyParams>,
    );
    m.insert(
        Method::GET_ALIAS_SYMBOL_OF_TYPE,
        unmarshaller_for::<GetTypePropertyParams>,
    );
    m.insert(
        Method::GET_OBJECT_TYPE_OF_TYPE,
        unmarshaller_for::<GetTypePropertyParams>,
    );
    m.insert(
        Method::GET_INDEX_TYPE_OF_TYPE,
        unmarshaller_for::<GetTypePropertyParams>,
    );
    m.insert(
        Method::GET_CHECK_TYPE_OF_TYPE,
        unmarshaller_for::<GetTypePropertyParams>,
    );
    m.insert(
        Method::GET_EXTENDS_TYPE_OF_TYPE,
        unmarshaller_for::<GetTypePropertyParams>,
    );
    m.insert(
        Method::GET_BASE_TYPE_OF_TYPE,
        unmarshaller_for::<GetTypePropertyParams>,
    );
    m.insert(
        Method::GET_CONSTRAINT_OF_TYPE,
        unmarshaller_for::<GetTypePropertyParams>,
    );
    // ts#64397
    m.insert(
        Method::GET_TYPE_PARAMETER_OF_MAPPED_TYPE,
        unmarshaller_for::<GetTypePropertyParams>,
    );
    m.insert(
        Method::GET_CONSTRAINT_TYPE_OF_MAPPED_TYPE,
        unmarshaller_for::<GetTypePropertyParams>,
    );
    m.insert(
        Method::GET_NAME_TYPE_OF_MAPPED_TYPE,
        unmarshaller_for::<GetTypePropertyParams>,
    );
    m.insert(
        Method::GET_TEMPLATE_TYPE_OF_MAPPED_TYPE,
        unmarshaller_for::<GetTypePropertyParams>,
    );
    m.insert(
        Method::GET_TRUE_TYPE_OF_CONDITIONAL_TYPE,
        unmarshaller_for::<GetTypePropertyParams>,
    );
    m.insert(
        Method::GET_FALSE_TYPE_OF_CONDITIONAL_TYPE,
        unmarshaller_for::<GetTypePropertyParams>,
    );

    m.insert(
        Method::GET_TYPE_PARAMETERS_OF_SIGNATURE,
        unmarshaller_for::<GetSignaturePropertyParams>,
    );
    m.insert(
        Method::GET_PARAMETERS_OF_SIGNATURE,
        unmarshaller_for::<GetSignaturePropertyParams>,
    );
    m.insert(
        Method::GET_THIS_PARAMETER_OF_SIGNATURE,
        unmarshaller_for::<GetSignaturePropertyParams>,
    );
    m.insert(
        Method::GET_TARGET_OF_SIGNATURE,
        unmarshaller_for::<GetSignaturePropertyParams>,
    );

    m.insert(
        Method::GET_CONTEXTUAL_TYPE,
        unmarshaller_for::<GetContextualTypeParams>,
    );
    // ts#64264
    m.insert(
        Method::GET_CONTEXTUAL_TYPE_FOR_ARGUMENT,
        unmarshaller_for::<GetContextualTypeForArgumentParams>,
    );
    m.insert(
        Method::GET_AWAITED_TYPE,
        unmarshaller_for::<CheckerTypeParams>,
    );
    m.insert(
        Method::GET_BASE_TYPE_OF_LITERAL_TYPE,
        unmarshaller_for::<GetBaseTypeOfLiteralTypeParams>,
    );
    m.insert(
        Method::GET_NON_NULLABLE_TYPE,
        unmarshaller_for::<GetTypePropertyParams>,
    );
    m.insert(
        Method::GET_TYPE_FROM_TYPE_NODE,
        unmarshaller_for::<GetTypeFromTypeNodeParams>,
    );
    m.insert(
        Method::GET_WIDENED_TYPE,
        unmarshaller_for::<GetWidenedTypeParams>,
    );
    m.insert(
        Method::GET_PARAMETER_TYPE,
        unmarshaller_for::<GetParameterTypeParams>,
    );
    m.insert(
        Method::GET_TYPE_PARAMETER_AT_POSITION,
        unmarshaller_for::<GetParameterTypeParams>,
    );
    m.insert(
        Method::IS_ARRAY_LIKE_TYPE,
        unmarshaller_for::<IsArrayLikeTypeParams>,
    );
    m.insert(
        Method::IS_TYPE_ASSIGNABLE_TO,
        unmarshaller_for::<IsTypeAssignableToParams>,
    );
    m.insert(
        Method::GET_SHORTHAND_ASSIGNMENT_VALUE_SYMBOL,
        unmarshaller_for::<GetTypeAtLocationParams>,
    );
    m.insert(
        Method::GET_TYPE_OF_SYMBOL_AT_LOCATION,
        unmarshaller_for::<GetTypeOfSymbolAtLocationParams>,
    );
    m.insert(
        Method::TYPE_TO_TYPE_NODE,
        unmarshaller_for::<TypeToTypeNodeParams>,
    );
    m.insert(
        Method::SIGNATURE_TO_SIGNATURE_DECLARATION,
        unmarshaller_for::<SignatureToSignatureDeclarationParams>,
    );
    m.insert(
        Method::TYPE_TO_STRING,
        unmarshaller_for::<TypeToTypeNodeParams>,
    );
    m.insert(
        Method::IS_CONTEXT_SENSITIVE,
        unmarshaller_for::<GetContextualTypeParams>,
    );
    m.insert(
        Method::GET_RETURN_TYPE_OF_SIGNATURE,
        unmarshaller_for::<GetSignaturePropertyParams>,
    );
    m.insert(
        Method::GET_REST_TYPE_OF_SIGNATURE,
        unmarshaller_for::<CheckerSignatureParams>,
    );
    m.insert(
        Method::GET_TYPE_PREDICATE_OF_SIGNATURE,
        unmarshaller_for::<CheckerSignatureParams>,
    );
    m.insert(
        Method::GET_BASE_TYPES,
        unmarshaller_for::<CheckerTypeParams>,
    );
    m.insert(
        Method::GET_PROPERTIES_OF_TYPE,
        unmarshaller_for::<CheckerTypeParams>,
    );
    m.insert(
        Method::GET_APPARENT_PROPERTIES_OF_TYPE,
        unmarshaller_for::<GetTypePropertyParams>,
    );
    m.insert(
        Method::GET_APPARENT_TYPE,
        unmarshaller_for::<GetTypePropertyParams>,
    );
    // ts#63899
    m.insert(
        Method::GET_REDUCED_TYPE,
        unmarshaller_for::<GetTypePropertyParams>,
    );
    m.insert(
        Method::GET_PROPERTY_OF_TYPE,
        unmarshaller_for::<GetPropertyOfTypeParams>,
    );
    // ts#64264
    m.insert(
        Method::GET_TYPE_OF_PROPERTY_OF_TYPE,
        unmarshaller_for::<GetPropertyOfTypeParams>,
    );
    m.insert(
        Method::GET_INDEX_INFO_OF_TYPE,
        unmarshaller_for::<GetIndexInfoOfTypeParams>,
    );
    m.insert(
        Method::GET_INDEX_INFOS_OF_TYPE,
        unmarshaller_for::<CheckerTypeParams>,
    );
    m.insert(
        Method::GET_CONSTRAINT_OF_TYPE_PARAMETER,
        unmarshaller_for::<GetTypePropertyParams>,
    );
    m.insert(
        Method::GET_BASE_CONSTRAINT_OF_TYPE,
        unmarshaller_for::<CheckerTypeParams>,
    );
    m.insert(
        Method::GET_DEFAULT_FROM_TYPE_PARAMETER,
        unmarshaller_for::<GetTypePropertyParams>,
    );
    m.insert(
        Method::GET_TYPE_ARGUMENTS,
        unmarshaller_for::<CheckerTypeParams>,
    );
    m.insert(
        Method::GET_IMPORT_ADDER_EDITS,
        unmarshaller_for::<GetImportAdderEditsParams>,
    );
    m.insert(
        Method::GET_CONSTANT_VALUE,
        unmarshaller_for::<CheckerNodeParams>,
    );
    m.insert(
        Method::GET_SIGNATURE_FROM_DECLARATION,
        unmarshaller_for::<CheckerNodeParams>,
    );
    m.insert(
        Method::GET_EXPORT_SPECIFIER_LOCAL_TARGET,
        unmarshaller_for::<CheckerNodeParams>,
    );
    m.insert(
        Method::GET_ALIASED_SYMBOL,
        unmarshaller_for::<CheckerSymbolParams>,
    );
    m.insert(
        Method::GET_IMMEDIATE_ALIASED_SYMBOL,
        unmarshaller_for::<CheckerSymbolParams>,
    );
    // ts#63945
    m.insert(
        Method::GET_TARGET_SYMBOL,
        unmarshaller_for::<CheckerSymbolParams>,
    );
    // ts#64264
    m.insert(
        Method::GET_EXPORT_SYMBOL_OF_SYMBOL_FOR_CHECKER,
        unmarshaller_for::<CheckerSymbolParams>,
    );
    m.insert(
        Method::GET_FULLY_QUALIFIED_NAME,
        unmarshaller_for::<CheckerSymbolParams>,
    );
    m.insert(
        Method::GET_EXPORTS_OF_MODULE,
        unmarshaller_for::<CheckerSymbolParams>,
    );
    m.insert(
        Method::GET_MEMBER_IN_MODULE_EXPORTS,
        unmarshaller_for::<GetMemberInModuleExportsParams>,
    );
    m.insert(
        Method::GET_JS_DOC_TAGS,
        unmarshaller_for::<CheckerSymbolParams>,
    );
    m.insert(
        Method::GET_DOCUMENTATION_COMMENT,
        unmarshaller_for::<CheckerSymbolParams>,
    );
    m.insert(Method::IS_ARRAY_TYPE, unmarshaller_for::<CheckerTypeParams>);
    // ts#63943
    m.insert(
        Method::IS_READONLY_SYMBOL,
        unmarshaller_for::<CheckerSymbolParams>,
    );
    m.insert(
        Method::GET_REFERENCES_TO_SYMBOL_IN_FILE,
        unmarshaller_for::<GetReferencesToSymbolInFileParams>,
    );
    m.insert(
        Method::GET_REFERENCED_SYMBOLS_FOR_NODE,
        unmarshaller_for::<GetReferencedSymbolsForNodeParams>,
    );
    m.insert(
        Method::GET_SIGNATURE_USAGES,
        unmarshaller_for::<GetSignatureUsagesParams>,
    );
    m.insert(
        Method::GET_COMPLETIONS_AT_POSITION,
        unmarshaller_for::<GetCompletionsAtPositionParams>,
    );
    m.insert(Method::PRINT_NODE, unmarshaller_for::<PrintNodeParams>);
    m.insert(
        Method::FORMAT_NODE_FOR_INSERTION,
        unmarshaller_for::<FormatNodeForInsertionParams>,
    );
    // tsgo#4699
    m.insert(Method::EMIT, unmarshaller_for::<EmitParams>);
    m.insert(Method::EMIT_TO_STRING, unmarshaller_for::<EmitParams>);
    m.insert(
        Method::GET_JAVA_SCRIPT_EMIT,
        unmarshaller_for::<SelectedFilesEmitParams>,
    );
    m.insert(
        Method::GET_DECLARATION_EMIT,
        unmarshaller_for::<SelectedFilesEmitParams>,
    );
    m.insert(
        Method::GET_ANY_TYPE,
        unmarshaller_for::<GetIntrinsicTypeParams>,
    );
    m.insert(
        Method::GET_STRING_TYPE,
        unmarshaller_for::<GetIntrinsicTypeParams>,
    );
    m.insert(
        Method::GET_NUMBER_TYPE,
        unmarshaller_for::<GetIntrinsicTypeParams>,
    );
    m.insert(
        Method::GET_BOOLEAN_TYPE,
        unmarshaller_for::<GetIntrinsicTypeParams>,
    );
    m.insert(
        Method::GET_VOID_TYPE,
        unmarshaller_for::<GetIntrinsicTypeParams>,
    );
    m.insert(
        Method::GET_UNDEFINED_TYPE,
        unmarshaller_for::<GetIntrinsicTypeParams>,
    );
    m.insert(
        Method::GET_NULL_TYPE,
        unmarshaller_for::<GetIntrinsicTypeParams>,
    );
    m.insert(
        Method::GET_NEVER_TYPE,
        unmarshaller_for::<GetIntrinsicTypeParams>,
    );
    m.insert(
        Method::GET_UNKNOWN_TYPE,
        unmarshaller_for::<GetIntrinsicTypeParams>,
    );
    m.insert(
        Method::GET_BIG_INT_TYPE,
        unmarshaller_for::<GetIntrinsicTypeParams>,
    );
    m.insert(
        Method::GET_ES_SYMBOL_TYPE,
        unmarshaller_for::<GetIntrinsicTypeParams>,
    );
    m.insert(
        Method::GET_NON_PRIMITIVE_TYPE,
        unmarshaller_for::<GetIntrinsicTypeParams>,
    );
    m.insert(
        Method::GET_WELL_KNOWN_SYMBOLS,
        unmarshaller_for::<GetIntrinsicTypeParams>,
    );
    m.insert(
        Method::GET_WELL_KNOWN_SIGNATURES,
        unmarshaller_for::<GetIntrinsicTypeParams>,
    );
    m.insert(
        Method::GET_SYNTACTIC_DIAGNOSTICS,
        unmarshaller_for::<GetDiagnosticsParams>,
    );
    m.insert(
        Method::GET_BIND_DIAGNOSTICS,
        unmarshaller_for::<GetDiagnosticsParams>,
    );
    m.insert(
        Method::GET_SEMANTIC_DIAGNOSTICS,
        unmarshaller_for::<GetDiagnosticsParams>,
    );
    m.insert(
        Method::GET_SUGGESTION_DIAGNOSTICS,
        unmarshaller_for::<GetDiagnosticsParams>,
    );
    m.insert(
        Method::GET_DECLARATION_DIAGNOSTICS,
        unmarshaller_for::<GetDiagnosticsParams>,
    );
    m.insert(
        Method::GET_PROGRAM_DIAGNOSTICS,
        unmarshaller_for::<GetProjectDiagnosticsParams>,
    );
    m.insert(
        Method::GET_GLOBAL_DIAGNOSTICS,
        unmarshaller_for::<GetProjectDiagnosticsParams>,
    );
    m.insert(
        Method::GET_CONFIG_FILE_PARSING_DIAGNOSTICS,
        unmarshaller_for::<GetProjectDiagnosticsParams>,
    );
    m.insert(Method::START_CPU_PROFILE, unmarshaller_for::<ProfileParams>);
    m.insert(Method::STOP_CPU_PROFILE, no_params);
    m.insert(Method::SAVE_HEAP_PROFILE, unmarshaller_for::<ProfileParams>);
    m
});

// Go: proto.go:746 ParseConfigFileParams
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ParseConfigFileParams {
    pub file: DocumentIdentifier,
}

proto_json!(both ParseConfigFileParams {
    file: "file" plain,
});

// Go: proto.go:750 ParseCommandLineParams
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ParseCommandLineParams {
    pub command_line: Vec<String>,
}

proto_json!(both ParseCommandLineParams {
    command_line: "commandLine" plain,
});

// Go: proto.go:754 ReadConfigFileParams
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ReadConfigFileParams {
    pub file: DocumentIdentifier,
}

proto_json!(both ReadConfigFileParams {
    file: "file" plain,
});

// Go: proto.go:758 ParseJsonConfigFileContentParams
// PORT: Go `JSON packagejson.JSONValue` is `LspAny`. A decoded request value
// must be `Send` (`AnyValue`), and `packagejson::JSONValue` holds `Rc`.
// Both decode a JSON value to the same tree: null, bool, float64, string,
// array, and an object that keeps its member order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ParseJsonConfigFileContentParams {
    pub json: LspAny,
    pub config_directory: Option<String>,
    pub config_file_name: Option<DocumentIdentifier>,
}

proto_json!(both ParseJsonConfigFileContentParams {
    json: "json" plain,
    config_directory: "configDirectory" omitempty,
    config_file_name: "configFileName" omitempty,
});

// Go: proto.go:764 jsonValueToAny
// PORT: Go `any` from the tsoptions JSON code is `CompilerOptionsValue`
// (nil, string, float64, bool, `[]any`, `*collections.OrderedMap`). The
// input is the `LspAny` that stands for `packagejson.JSONValue` (see
// `ParseJsonConfigFileContentParams`); Go's "not present" is `Null` there.
pub fn json_value_to_any(value: &LspAny) -> tsoptions::CompilerOptionsValue {
    use crate::frontend::tsoptions::CompilerOptionsValue;
    match value {
        LspAny::Null => CompilerOptionsValue::Nil,
        LspAny::String(value) => CompilerOptionsValue::String(value.clone()),
        LspAny::Number(value) => CompilerOptionsValue::Number(*value),
        LspAny::Bool(value) => CompilerOptionsValue::Bool(*value),
        LspAny::Array(array) => {
            let mut result = Vec::with_capacity(array.len());
            for child in array {
                result.push(json_value_to_any(child));
            }
            CompilerOptionsValue::List(result)
        }
        LspAny::Object(object) => {
            let mut result = IndexMap::with_capacity(object.len());
            for (key, child) in object {
                result.insert(key.clone(), json_value_to_any(child));
            }
            CompilerOptionsValue::Map(result)
        }
    }
}

// Go: proto.go:789 TranspileOptions (tsgo#4849)
// PORT: Go `*core.CompilerOptions` is `Option<CompilerOptions>` (nil is
// `None`). Its JSON form is the Go struct default (`CompilerOptionsJSON` to
// write, the `CompilerOptions` `UnmarshalerFrom` below to read).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TranspileOptions {
    pub compiler_options: Option<CompilerOptions>,
    pub file_name: String,
    pub report_diagnostics: bool,
}

impl MarshalerTo for TranspileOptions {
    fn marshal_json_to(&self, enc: &mut String) -> Result<(), JsonError> {
        write_object_start(enc);
        let mut first = true;
        marshal_field_omitempty(
            enc,
            &mut first,
            "compilerOptions",
            &self.compiler_options.as_ref().map(CompilerOptionsJSON),
        )?;
        marshal_field_omitempty(enc, &mut first, "fileName", &self.file_name)?;
        marshal_field_omitempty(
            enc,
            &mut first,
            "reportDiagnostics",
            &self.report_diagnostics,
        )?;
        write_object_end(enc);
        Ok(())
    }
}

impl UnmarshalerFrom for TranspileOptions {
    fn unmarshal_json_from(&mut self, dec: &mut JsonDecoder<'_>) -> Result<(), JsonError> {
        let is_object = unmarshal_struct_fields(dec, "api.TranspileOptions", |name, dec| {
            match name {
                "compilerOptions" => json_unmarshal_decode(dec, &mut self.compiler_options)?,
                "fileName" => json_unmarshal_decode(dec, &mut self.file_name)?,
                "reportDiagnostics" => json_unmarshal_decode(dec, &mut self.report_diagnostics)?,
                _ => return Ok(false),
            }
            Ok(true)
        })?;
        if !is_object {
            *self = TranspileOptions::default();
        }
        Ok(())
    }
}

// Go: proto.go CreateSourceFileOptions (ts#64216)
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CreateSourceFileOptions {
    pub script_kind: ScriptKind,
}

proto_json!(both CreateSourceFileOptions {
    script_kind: "scriptKind" omitempty,
});

// Go: proto.go CreateSourceFileParams (ts#64216)
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CreateSourceFileParams {
    pub file_name: String,
    pub source_text: String,
    pub options: CreateSourceFileOptions,
}

proto_json!(both CreateSourceFileParams {
    file_name: "fileName" plain,
    source_text: "sourceText" plain,
    options: "options" plain,
});

// Go: proto.go CreateSourceFileFromFileParams (ts#64216)
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CreateSourceFileFromFileParams {
    pub file_name: String,
    pub options: CreateSourceFileOptions,
}

proto_json!(both CreateSourceFileFromFileParams {
    file_name: "fileName" plain,
    options: "options" plain,
});

// Go: proto.go:810 TranspileParams (tsgo#4849)
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TranspileParams {
    pub input: String,
    pub options: TranspileOptions,
}

proto_json!(both TranspileParams {
    input: "input" plain,
    options: "options" plain,
});

// Go: proto.go:815 TranspileFromFileParams (tsgo#4849)
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TranspileFromFileParams {
    pub file_name: String,
    pub options: TranspileOptions,
}

proto_json!(both TranspileFromFileParams {
    file_name: "fileName" plain,
    options: "options" plain,
});

// Go: proto.go:820 TranspileOutputResponse (tsgo#4849)
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TranspileOutputResponse {
    pub output_text: String,
    pub diagnostics: Vec<DiagnosticResponse>,
    pub source_map_text: String,
}

proto_json!(marshal TranspileOutputResponse {
    output_text: "outputText" plain,
    diagnostics: "diagnostics" omitempty,
    source_map_text: "sourceMapText" omitempty,
});

// Go: proto.go BatchRequestsParams (ts#63937)
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BatchRequestsParams {
    pub requests: Vec<BatchRequest>,
    // ts#64061
    pub continuation_token: String,
    pub max_response_bytes_per_page: i32,
}

proto_json!(both BatchRequestsParams {
    requests: "requests" plain,
    continuation_token: "continuationToken" omitempty,
    max_response_bytes_per_page: "maxResponseBytesPerPage" omitempty,
});

// Go: proto.go BatchRequest (ts#63937)
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BatchRequest {
    pub method: Method,
    pub params: JsonValue,
}

proto_json!(both BatchRequest {
    method: "method" plain,
    params: "params" omitempty,
});

// Go: proto.go BatchRequestsResponse (ts#63937, ts#64061)
// PORT: Go `encodedResponses []json.Value` is `Option`: `None` is nil.
#[derive(Debug, Default)]
pub struct BatchRequestsResponse {
    pub responses: Vec<BatchResponse>,
    pub continuation_token: String,
    pub encoded_responses: Option<Vec<JsonValue>>,
}

// Go: proto.go BatchRequestsResponse.MarshalJSONTo (ts#64061)
impl MarshalerTo for BatchRequestsResponse {
    fn marshal_json_to(&self, enc: &mut String) -> Result<(), JsonError> {
        write_object_start(enc);
        enc.push_str("\"responses\":[");
        if let Some(encoded_responses) = &self.encoded_responses {
            for (i, response) in encoded_responses.iter().enumerate() {
                if i > 0 {
                    enc.push(',');
                }
                json_ext::write_value(enc, &response.0)?;
            }
        } else {
            for (i, response) in self.responses.iter().enumerate() {
                if i > 0 {
                    enc.push(',');
                }
                response.marshal_json_to(enc)?;
            }
        }
        enc.push(']');
        if !self.continuation_token.is_empty() {
            enc.push_str(",\"continuationToken\":");
            self.continuation_token.marshal_json_to(enc)?;
        }
        write_object_end(enc);
        Ok(())
    }
}

// Go: proto.go BatchResponse (ts#63937)
// PORT: Go `Result any` is the handler result (`None` is a nil `any`).
#[derive(Debug, Default)]
pub struct BatchResponse {
    pub method: Method,
    pub result: Option<Box<dyn AnyValue>>,
    pub error: String,
}

proto_json!(marshal BatchResponse {
    method: "method" plain,
    result: "result" plain,
    error: "error" omitempty,
});

// ReleaseParams are the parameters for the release method.
// Go: proto.go:889 ReleaseParams
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ReleaseParams {
    pub snapshot: SnapshotID,
}

proto_json!(both ReleaseParams {
    snapshot: "snapshot" plain,
});

// Go: proto.go CreateBuildOrchestratorParams (ts#64158)
// PORT: Go embeds `*core.BuildOptions` and `*core.CompilerOptions` with
// JSON names; they decode as the members `buildOptions` and
// `compilerOptions`.
#[derive(Clone, Debug, Default)]
pub struct CreateBuildOrchestratorParams {
    pub root_names: Vec<String>,
    pub cwd: String,
    // Only a subset of these options are exposed  the API
    pub build_options: Option<crate::execute::build::BuildOptions>,
    pub compiler_options: Option<CompilerOptions>,
}

impl UnmarshalerFrom for CreateBuildOrchestratorParams {
    fn unmarshal_json_from(&mut self, dec: &mut JsonDecoder<'_>) -> Result<(), JsonError> {
        let is_object =
            unmarshal_struct_fields(dec, "api.CreateBuildOrchestratorParams", |name, dec| {
                match name {
                    "rootNames" => json_unmarshal_decode(dec, &mut self.root_names)?,
                    "cwd" => json_unmarshal_decode(dec, &mut self.cwd)?,
                    "buildOptions" => {
                        if dec.peek_kind() == b'n' {
                            dec.read_token()?;
                            self.build_options = None;
                        } else {
                            let build_options =
                                self.build_options.get_or_insert_with(Default::default);
                            unmarshal_build_options(dec, build_options)?;
                        }
                    }
                    "compilerOptions" => json_unmarshal_decode(dec, &mut self.compiler_options)?,
                    _ => return Ok(false),
                }
                Ok(true)
            })?;
        if !is_object {
            *self = CreateBuildOrchestratorParams::default();
        }
        Ok(())
    }
}

impl MarshalerTo for CreateBuildOrchestratorParams {
    fn marshal_json_to(&self, enc: &mut String) -> Result<(), JsonError> {
        write_object_start(enc);
        let mut first = true;
        marshal_field(enc, &mut first, "rootNames", &self.root_names)?;
        marshal_field_omitempty(enc, &mut first, "cwd", &self.cwd)?;
        marshal_field_omitempty(
            enc,
            &mut first,
            "buildOptions",
            &self.build_options.as_ref().map(BuildOptionsJSON),
        )?;
        marshal_field_omitempty(
            enc,
            &mut first,
            "compilerOptions",
            &self.compiler_options.as_ref().map(CompilerOptionsJSON),
        )?;
        write_object_end(enc);
        Ok(())
    }
}

/// Go v2 marshal of `core.BuildOptions` (by reflection: the tags `dry`,
/// `force`, `verbose`, `builders`, `stopBuildOnErrors` and `clean`, each
/// `omitzero`). A `Tristate` writes its legacy `MarshalJSON` text.
struct BuildOptionsJSON<'a>(&'a crate::execute::build::BuildOptions);

impl MarshalerTo for BuildOptionsJSON<'_> {
    fn marshal_json_to(&self, enc: &mut String) -> Result<(), JsonError> {
        // `TSUnknown` is the `Tristate` zero value.
        fn tristate_omitzero(
            enc: &mut String,
            first: &mut bool,
            name: &str,
            value: Tristate,
        ) -> Result<(), JsonError> {
            if value == Tristate::Unknown {
                return Ok(());
            }
            if !*first {
                enc.push(',');
            }
            *first = false;
            name.marshal_json_to(enc)?;
            enc.push(':');
            enc.push_str(
                std::str::from_utf8(value.marshal_json()).expect("Tristate JSON is ASCII"),
            );
            Ok(())
        }
        let o = self.0;
        write_object_start(enc);
        let mut first = true;
        tristate_omitzero(enc, &mut first, "dry", o.dry)?;
        tristate_omitzero(enc, &mut first, "force", o.force)?;
        tristate_omitzero(enc, &mut first, "verbose", o.verbose)?;
        marshal_opt_field(enc, &mut first, "builders", &o.builders)?;
        tristate_omitzero(enc, &mut first, "stopBuildOnErrors", o.stop_build_on_errors)?;
        tristate_omitzero(enc, &mut first, "clean", o.clean)?;
        write_object_end(enc);
        Ok(())
    }
}

// PORT: the Go v2 struct decode of `core.BuildOptions` (the tags
// `dry`, `force`, `verbose`, `builders`, `stopBuildOnErrors`, `clean`).
// `core.Tristate` decodes through its legacy `UnmarshalJSON` (raw value).
fn unmarshal_build_options(
    dec: &mut JsonDecoder<'_>,
    o: &mut crate::execute::build::BuildOptions,
) -> Result<(), JsonError> {
    unmarshal_struct_fields(dec, "core.BuildOptions", |name, dec| {
        match name {
            "dry" => o.dry.unmarshal_json(dec.read_value()?),
            "force" => o.force.unmarshal_json(dec.read_value()?),
            "verbose" => o.verbose.unmarshal_json(dec.read_value()?),
            "builders" => crate::options_json::unmarshal_go_int_ptr(dec, &mut o.builders)?,
            "stopBuildOnErrors" => o.stop_build_on_errors.unmarshal_json(dec.read_value()?),
            "clean" => o.clean.unmarshal_json(dec.read_value()?),
            _ => return Ok(false),
        }
        Ok(true)
    })?;
    Ok(())
}

// Go: proto.go CreateBuildOrchestratorResponse (ts#64158)
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CreateBuildOrchestratorResponse {
    pub build_orchestrator_id: BuildOrchestratorID,
}

proto_json!(marshal CreateBuildOrchestratorResponse {
    build_orchestrator_id: "buildOrchestratorID" plain,
});

// Go: proto.go DisposeBuildOrchestratorParams (ts#64158)
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DisposeBuildOrchestratorParams {
    pub build_orchestrator_id: BuildOrchestratorID,
}

proto_json!(both DisposeBuildOrchestratorParams {
    build_orchestrator_id: "buildOrchestratorID" plain,
});

// Go: proto.go BuildParams (ts#64158)
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BuildParams {
    pub build_orchestrator_id: BuildOrchestratorID,
    pub project: String,
}

proto_json!(both BuildParams {
    build_orchestrator_id: "buildOrchestratorID" plain,
    project: "project" omitempty,
});

/// Go v2 marshal of `tsc.Statistics` (by reflection: the exported fields
/// `Projects`, `ProjectsBuilt` and `TimestampUpdates`, with their Go names).
pub struct StatisticsJSON<'a>(pub &'a crate::execute::tsc::statistics::Statistics);

impl MarshalerTo for StatisticsJSON<'_> {
    fn marshal_json_to(&self, enc: &mut String) -> Result<(), JsonError> {
        write_object_start(enc);
        let mut first = true;
        marshal_field(enc, &mut first, "Projects", &self.0.projects)?;
        marshal_field(enc, &mut first, "ProjectsBuilt", &self.0.projects_built)?;
        marshal_field(
            enc,
            &mut first,
            "TimestampUpdates",
            &self.0.timestamp_updates,
        )?;
        write_object_end(enc);
        Ok(())
    }
}

// Go: proto.go BuildResponse (ts#64158)
// PORT: Go `tsc.ExitStatus` is a Go int type; it writes as a number.
#[derive(Clone, Debug, Default)]
pub struct BuildResponse {
    pub status: crate::execute::tsc::ExitStatus,
    pub diagnostics: Vec<DiagnosticResponse>,
    pub statistics: crate::execute::tsc::statistics::Statistics,
}

impl MarshalerTo for BuildResponse {
    fn marshal_json_to(&self, enc: &mut String) -> Result<(), JsonError> {
        write_object_start(enc);
        let mut first = true;
        marshal_field(enc, &mut first, "status", &(self.status as i32))?;
        marshal_field_omitempty(enc, &mut first, "diagnostics", &self.diagnostics)?;
        marshal_field(
            enc,
            &mut first,
            "statistics",
            &StatisticsJSON(&self.statistics),
        )?;
        write_object_end(enc);
        Ok(())
    }
}

// Go: proto.go CleanBuildParams (ts#64158)
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CleanBuildParams {
    pub build_orchestrator_id: BuildOrchestratorID,
    pub project: String,
}

proto_json!(both CleanBuildParams {
    build_orchestrator_id: "buildOrchestratorID" plain,
    project: "project" omitempty,
});

// Go: proto.go CleanBuildResponse (ts#64158)
#[derive(Clone, Debug, Default)]
pub struct CleanBuildResponse {
    pub status: crate::execute::tsc::ExitStatus,
    pub diagnostics: Vec<DiagnosticResponse>,
    pub statistics: crate::execute::tsc::statistics::Statistics,
    pub files_deleted: Vec<String>,
}

impl MarshalerTo for CleanBuildResponse {
    fn marshal_json_to(&self, enc: &mut String) -> Result<(), JsonError> {
        write_object_start(enc);
        let mut first = true;
        marshal_field(enc, &mut first, "status", &(self.status as i32))?;
        marshal_field_omitempty(enc, &mut first, "diagnostics", &self.diagnostics)?;
        marshal_field(
            enc,
            &mut first,
            "statistics",
            &StatisticsJSON(&self.statistics),
        )?;
        marshal_field_omitempty(enc, &mut first, "filesDeleted", &self.files_deleted)?;
        write_object_end(enc);
        Ok(())
    }
}

// PORT: Go `BuildOrchestrator` (a struct of three func fields, ts#64158)
// is not used by any Go code; it is not ported. Go
// `ConfigFileResponse.BuildOptions` (ts#64158) is never set, so it is
// always omitted and not ported.

// Go: proto.go ReleaseSourceFileParams (ts#64434)
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ReleaseSourceFileParams {
    pub lease: SourceFileLeaseID,
}

proto_json!(both ReleaseSourceFileParams {
    lease: "lease" plain,
});

// Go: proto.go:930 SourceFileDescriptor (ts#64518)
// The complete identity of an ordinary cached source file: its parse cache
// key and the node ID of the exact AST that the client saw.
// PORT: ts#64159 types FileName and Path (`tspath.RootedFilePath`,
// `tspath.PathKey`); the port keeps strings.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SourceFileDescriptor {
    pub file_name: String,
    pub path: String,
    pub content_hash: String,
    pub parse_options_key: String,
    pub script_kind: ScriptKind,
    pub node_id: String,
}

proto_json!(both SourceFileDescriptor {
    file_name: "fileName" plain,
    path: "path" plain,
    content_hash: "contentHash" plain,
    parse_options_key: "parseOptionsKey" plain,
    script_kind: "scriptKind" plain,
    node_id: "nodeId" plain,
});

// Go: proto.go:939 RetainSourceFileParams (ts#64518)
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RetainSourceFileParams {
    pub file: SourceFileDescriptor,
}

proto_json!(both RetainSourceFileParams {
    file: "file" plain,
});

// Go: proto.go:943 RetainSourceFileResponse (ts#64518)
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RetainSourceFileResponse {
    pub lease: SourceFileLeaseID,
}

proto_json!(marshal RetainSourceFileResponse {
    lease: "lease" plain,
});

// Go: proto.go:949 GetCachedSourceFileParams (ts#64518)
// GetCachedSourceFileParams address an ordinary cached source file by its complete identity,
// independent of any snapshot or lease.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GetCachedSourceFileParams {
    pub file: SourceFileDescriptor,
}

proto_json!(both GetCachedSourceFileParams {
    file: "file" plain,
});

// Go: proto.go:897 ProfileParams
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ProfileParams {
    pub dir: String,
}

proto_json!(both ProfileParams {
    dir: "dir" plain,
});

// Go: proto.go:901 ProfileResult
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ProfileResult {
    pub file: String,
}

proto_json!(marshal ProfileResult {
    file: "file" plain,
});

// Go: proto.go:950 ConfigFileResponse
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ConfigFileResponse {
    pub file_names: Vec<String>,
    pub options: Option<CompilerOptions>,
    pub project_references: Vec<crate::frontend::core_ext::ProjectReference>,
    pub type_acquisition: Option<crate::frontend::core_ext::TypeAcquisition>,
    pub compile_on_save: Option<bool>,
    pub raw: tsoptions::CompilerOptionsValue,
    pub errors: Vec<DiagnosticResponse>,
}

impl MarshalerTo for ConfigFileResponse {
    fn marshal_json_to(&self, enc: &mut String) -> Result<(), JsonError> {
        write_object_start(enc);
        let mut first = true;
        marshal_field(enc, &mut first, "fileNames", &self.file_names)?;
        marshal_field(
            enc,
            &mut first,
            "options",
            &self.options.as_ref().map(CompilerOptionsJSON),
        )?;
        marshal_field_omitempty(
            enc,
            &mut first,
            "projectReferences",
            &self.project_references,
        )?;
        marshal_field_omitempty(
            enc,
            &mut first,
            "typeAcquisition",
            &self.type_acquisition.as_ref().map(TypeAcquisitionJSON),
        )?;
        marshal_field_omitempty(enc, &mut first, "compileOnSave", &self.compile_on_save)?;
        marshal_field_omitempty(enc, &mut first, "raw", &AnyJSON(&self.raw))?;
        marshal_field(enc, &mut first, "errors", &self.errors)?;
        write_object_end(enc);
        Ok(())
    }
}

// Go: proto.go:961 ReadConfigFileResponse
// PORT: Go `Config any` is `CompilerOptionsValue` (see `json_value_to_any`).
#[derive(Clone, Debug, PartialEq)]
pub struct ReadConfigFileResponse {
    pub config: tsoptions::CompilerOptionsValue,
    pub error: Option<DiagnosticResponse>,
}

impl MarshalerTo for ReadConfigFileResponse {
    fn marshal_json_to(&self, enc: &mut String) -> Result<(), JsonError> {
        write_object_start(enc);
        let mut first = true;
        marshal_field(enc, &mut first, "config", &AnyJSON(&self.config))?;
        marshal_field_omitempty(enc, &mut first, "error", &self.error)?;
        write_object_end(enc);
        Ok(())
    }
}

/// Go `any` that holds a tsoptions JSON value: the v2 marshaler of its
/// dynamic type (`build_info::marshal_any`).
pub struct AnyJSON<'a>(pub &'a tsoptions::CompilerOptionsValue);

impl MarshalerTo for AnyJSON<'_> {
    fn marshal_json_to(&self, enc: &mut String) -> Result<(), JsonError> {
        crate::execute::incremental::build_info::marshal_any(enc, self.0)
    }
}

// Go: proto.go:966 GetDefaultProjectForFileParams
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GetDefaultProjectForFileParams {
    pub snapshot: SnapshotID,
    pub file: DocumentIdentifier,
}

proto_json!(both GetDefaultProjectForFileParams {
    snapshot: "snapshot" plain,
    file: "file" plain,
});

// Go: proto.go:971 ProjectResponse
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ProjectResponse {
    pub id: project::ID,
    pub config_file_name: String,
    // ts#63935
    pub current_directory: String,
    // ts#64204
    pub dirty: bool,
    pub parsed_command_line: Option<ConfigFileResponse>,
    // Deprecated: Use parsedCommandLine.fileNames.
    pub root_files: Vec<String>,
    // Deprecated: Use parsedCommandLine.options.
    pub compiler_options: Option<CompilerOptions>,
}

impl MarshalerTo for ProjectResponse {
    fn marshal_json_to(&self, enc: &mut String) -> Result<(), JsonError> {
        write_object_start(enc);
        let mut first = true;
        marshal_field(enc, &mut first, "id", &self.id)?;
        marshal_field(enc, &mut first, "configFileName", &self.config_file_name)?;
        marshal_field(enc, &mut first, "currentDirectory", &self.current_directory)?;
        marshal_field(enc, &mut first, "dirty", &self.dirty)?;
        marshal_field(
            enc,
            &mut first,
            "parsedCommandLine",
            &self.parsed_command_line,
        )?;
        marshal_field(enc, &mut first, "rootFiles", &self.root_files)?;
        marshal_field(
            enc,
            &mut first,
            "compilerOptions",
            &self.compiler_options.as_ref().map(CompilerOptionsJSON),
        )?;
        write_object_end(enc);
        Ok(())
    }
}

// Go: proto.go:983 NewConfigFileResponse
// PORT: Go shares the `*core.CompilerOptions`, `*core.TypeAcquisition` and
// project reference pointers; the response keeps copies (see
// `new_project_response`). Go `Raw.(*collections.OrderedMap[string, any])`
// is `CompilerOptionsValue::Map`.
pub fn new_config_file_response(
    parsed_command_line: Option<&tsoptions::ParsedCommandLine>,
) -> Option<ConfigFileResponse> {
    let parsed_command_line = parsed_command_line?;
    let mut compile_on_save = parsed_command_line.compile_on_save;
    if compile_on_save.is_none()
        && let tsoptions::CompilerOptionsValue::Map(raw_config) = &parsed_command_line.raw
        && let Some(tsoptions::CompilerOptionsValue::Bool(value)) = raw_config.get("compileOnSave")
    {
        compile_on_save = Some(*value);
    }
    let compiler_options = parsed_command_line.compiler_options();
    // PORT: Go replaces a nil slice with an empty one; a `Vec` is never nil.
    // Go `p.Errors` holds the TS6059 errors of `CommonSourceDirectory` so
    // far (`errors_with_common_source_directory_errors`).
    let errors =
        new_diagnostic_responses(&parsed_command_line.errors_with_common_source_directory_errors());
    Some(ConfigFileResponse {
        file_names: parsed_command_line.file_names().to_vec(),
        options: Some((**compiler_options).clone()),
        project_references: parsed_command_line.project_references().to_vec(),
        type_acquisition: parsed_command_line.type_acquisition().cloned(),
        compile_on_save,
        raw: to_protocol_json_value(&parsed_command_line.raw),
        errors,
    })
}

// Go: proto.go:1011 toProtocolJSONValue
// PORT: ts#64457 removes the Go function with the watch options (Go passes
// `Raw` through). Only its watch kind cases are gone here; the api lane ports
// the rest of that change.
pub fn to_protocol_json_value(
    value: &tsoptions::CompilerOptionsValue,
) -> tsoptions::CompilerOptionsValue {
    use crate::frontend::tsoptions::CompilerOptionsValue;
    match value {
        CompilerOptionsValue::Map(value) => {
            let mut result = IndexMap::with_capacity(value.len());
            for (key, child) in value {
                result.insert(key.clone(), to_protocol_json_value(child));
            }
            CompilerOptionsValue::Map(result)
        }
        CompilerOptionsValue::List(value) => {
            let mut result = Vec::with_capacity(value.len());
            for child in value {
                result.push(to_protocol_json_value(child));
            }
            CompilerOptionsValue::List(result)
        }
        // Go `case []any` makes a non-nil slice, also for a nil `[]any`.
        CompilerOptionsValue::NilList => CompilerOptionsValue::List(Vec::new()),
        value => value.clone(),
    }
}

// Go: proto.go:1036 NewProjectResponse
// PORT: Go shares the `*core.CompilerOptions` pointer; the response keeps
// a copy (responses cross into `Box<dyn AnyValue>`, which is `Send`).
pub fn new_project_response(p: &project::Project) -> ProjectResponse {
    let Some(command_line) = p.command_line.as_ref() else {
        panic!("NewProjectResponse called with unloaded project");
    };
    // ts#64204: the config file name of a configured project only.
    let mut config_file_name = String::new();
    if p.kind == project::Kind::CONFIGURED {
        config_file_name = p.config_file_name();
    }
    ProjectResponse {
        id: p.id(),
        config_file_name,
        // PORT: Go `p.CurrentDirectory()` (ts#63935) returns this field.
        current_directory: p.current_directory.clone(),
        // PORT: Go `p.IsDirty()` returns this field.
        dirty: p.dirty,
        parsed_command_line: new_config_file_response(Some(command_line)),
        root_files: command_line.file_names().to_vec(),
        compiler_options: Some((**command_line.compiler_options()).clone()),
    }
}

// Go: proto.go:1055 GetSymbolAtPositionParams
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GetSymbolAtPositionParams {
    pub snapshot: SnapshotID,
    pub project: project::ID,
    pub file: DocumentIdentifier,
    pub position: u32,
}

proto_json!(both GetSymbolAtPositionParams {
    snapshot: "snapshot" plain,
    project: "project" plain,
    file: "file" plain,
    position: "position" plain,
});

// Go: proto.go:1062 GetSymbolsAtPositionsParams
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GetSymbolsAtPositionsParams {
    pub snapshot: SnapshotID,
    pub project: project::ID,
    pub file: DocumentIdentifier,
    pub positions: Vec<u32>,
}

proto_json!(both GetSymbolsAtPositionsParams {
    snapshot: "snapshot" plain,
    project: "project" plain,
    file: "file" plain,
    positions: "positions" plain,
});

// Go: proto.go:1069 GetSymbolOfSourceFileParams
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GetSymbolOfSourceFileParams {
    pub snapshot: SnapshotID,
    pub project: project::ID,
    pub file: DocumentIdentifier,
}

proto_json!(both GetSymbolOfSourceFileParams {
    snapshot: "snapshot" plain,
    project: "project" plain,
    file: "file" plain,
});

// Go: proto.go:1075 GetSymbolsOfSourceFilesParams
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GetSymbolsOfSourceFilesParams {
    pub snapshot: SnapshotID,
    pub project: project::ID,
    pub files: Vec<DocumentIdentifier>,
}

proto_json!(both GetSymbolsOfSourceFilesParams {
    snapshot: "snapshot" plain,
    project: "project" plain,
    files: "files" plain,
});

// Go: proto.go:1081 GetSymbolAtLocationParams
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GetSymbolAtLocationParams {
    pub snapshot: SnapshotID,
    pub project: project::ID,
    pub location: NodeHandle,
}

proto_json!(both GetSymbolAtLocationParams {
    snapshot: "snapshot" plain,
    project: "project" plain,
    location: "location" plain,
});

// Go: proto.go:1087 GetSymbolsAtLocationsParams
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GetSymbolsAtLocationsParams {
    pub snapshot: SnapshotID,
    pub project: project::ID,
    pub locations: Vec<NodeHandle>,
}

proto_json!(both GetSymbolsAtLocationsParams {
    snapshot: "snapshot" plain,
    project: "project" plain,
    locations: "locations" plain,
});

// Go: proto.go:1130 SymbolResponse
// ts#64518: `reference` names the symbol and its owner; `parent` and
// `exportSymbol` are compact references.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SymbolResponse {
    pub reference: SymbolReference,
    pub name: String,
    pub flags: u32,
    pub check_flags: u32,
    pub declarations: Vec<NodeHandle>,
    pub value_declaration: NodeHandle,
    pub parent: Option<CompactSymbolReference>,
    pub export_symbol: Option<CompactSymbolReference>,
}

proto_json!(marshal SymbolResponse {
    reference: "reference" plain,
    name: "name" plain,
    flags: "flags" plain,
    check_flags: "checkFlags" plain,
    declarations: "declarations" omitempty,
    value_declaration: "valueDeclaration" omitempty,
    parent: "parent" omitempty,
    export_symbol: "exportSymbol" omitempty,
});

// Go: proto.go:1141 SymbolOwnerKind (ts#64518)
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct SymbolOwnerKind(pub u32);

impl SymbolOwnerKind {
    // Go: proto.go:1143
    pub const FILE: SymbolOwnerKind = SymbolOwnerKind(0);
    pub const SNAPSHOT: SymbolOwnerKind = SymbolOwnerKind(1);
}

handle_json!(uint: SymbolOwnerKind);

// Go: proto.go:1148 SymbolOwner and proto.go:1156 SymbolReference (ts#64518)
// SymbolReference identifies a symbol and its server-resolvable owner.
// PORT: Go `SymbolReference` embeds `SymbolOwner`, whose fields JSON v2
// inlines (`kind`, `file`, `snapshot`, `project`, then `id`). Nothing else
// uses `SymbolOwner`, so the port has one flat struct.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SymbolReference {
    pub kind: SymbolOwnerKind,
    pub file: Option<SourceFileDescriptor>,
    pub snapshot: SnapshotID,
    pub project: project::ID,
    pub id: SymbolID,
}

proto_json!(both SymbolReference {
    kind: "kind" plain,
    file: "file" omitempty,
    snapshot: "snapshot" omitzero,
    project: "project" omitempty,
    id: "id" plain,
});

// Go: proto.go:1165 CompactSymbolReference (ts#64518)
// CompactSymbolReference is embedded in other responses. It identifies a cached
// symbol without repeating its owning file's full descriptor: File is the owning source file's
// node ID, or empty for a symbol owned by the response's snapshot. When the client has not cached
// the symbol, it fetches a full SymbolResponse through the corresponding property method.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CompactSymbolReference {
    pub id: SymbolID,
    pub file: String,
}

proto_json!(marshal CompactSymbolReference {
    id: "id" plain,
    file: "file" omitempty,
});

// Go: proto.go:1107 symbolHandles (at 673a5f17d713; removed by ts#64518)

// Go: proto.go:1170 GetTypeOfSymbolParams
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GetTypeOfSymbolParams {
    pub snapshot: SnapshotID,
    pub project: project::ID,
    // ts#64518
    pub symbol: SymbolReference,
}

proto_json!(both GetTypeOfSymbolParams {
    snapshot: "snapshot" plain,
    project: "project" plain,
    symbol: "symbol" plain,
});

// Go: proto.go:1176 GetTypesOfSymbolsParams
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GetTypesOfSymbolsParams {
    pub snapshot: SnapshotID,
    pub project: project::ID,
    // ts#64518
    pub symbols: Vec<SymbolReference>,
}

proto_json!(both GetTypesOfSymbolsParams {
    snapshot: "snapshot" plain,
    project: "project" plain,
    symbols: "symbols" plain,
});

// Go: proto.go:1182 TypeResponse
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TypeResponse {
    pub id: TypeID,
    pub flags: u32,
    pub object_flags: u32,
    // ts#64080
    pub is_tuple_type: bool,

    // Value is literal type data. BigInt literals are encoded as signed decimal
    // strings because JSON cannot represent bigint; absent values are null.
    pub value: LspAny,

    // ObjectType / TypeReference / StringMappingType / IndexType target
    pub target: TypeID,

    // InterfaceType type parameters
    pub type_parameters: Vec<TypeID>,
    pub outer_type_parameters: Vec<TypeID>,
    pub local_type_parameters: Vec<TypeID>,

    // TupleType data
    pub element_flags: Vec<ElementFlags>,
    pub fixed_length: Option<i32>,
    pub tuple_readonly: Option<bool>,
    // ts#64109
    pub labeled_element_declarations: Vec<NodeHandle>,

    // IndexedAccessType data
    pub object_type: TypeID,
    pub index_type: TypeID,

    // ConditionalType data
    pub check_type: TypeID,
    pub extends_type: TypeID,

    // SubstitutionType data
    pub base_type: TypeID,
    pub subst_constraint: TypeID,

    // MappedType data (ts#64397)
    pub type_parameter: TypeID,
    pub constraint_type: TypeID,
    pub name_type: TypeID,
    pub template_type: TypeID,

    // TemplateLiteralType text segments
    pub texts: Vec<String>,

    // FreshableType data (LiteralType and computed enum types)
    pub fresh_type: TypeID,
    pub regular_type: TypeID,

    // TypeParameter data
    pub is_this_type: bool,

    // InterfaceType data (ts#64264)
    pub this_type: TypeID,

    // IntrinsicType data
    pub intrinsic_name: String,

    // TypeAlias data
    pub alias_type_arguments: Vec<TypeID>,
    // ts#64518
    pub alias_symbol: Option<CompactSymbolReference>,

    // Symbol associated with structured types
    pub symbol: Option<CompactSymbolReference>,
}

impl MarshalerTo for TypeResponse {
    fn marshal_json_to(&self, enc: &mut String) -> Result<(), JsonError> {
        // PORT: `checker.ElementFlags` is a Go uint32 type; it writes as a number.
        let element_flags: Vec<u32> = self.element_flags.iter().map(|f| f.0).collect();
        write_object_start(enc);
        let mut first = true;
        marshal_field(enc, &mut first, "id", &self.id)?;
        marshal_field(enc, &mut first, "flags", &self.flags)?;
        marshal_field_omitempty(enc, &mut first, "objectFlags", &self.object_flags)?;
        marshal_field_omitempty(enc, &mut first, "isTupleType", &self.is_tuple_type)?;
        marshal_field(enc, &mut first, "value", &self.value)?;
        marshal_field_omitzero(enc, &mut first, "target", &self.target)?;
        marshal_field_omitempty(enc, &mut first, "typeParameters", &self.type_parameters)?;
        marshal_field_omitempty(
            enc,
            &mut first,
            "outerTypeParameters",
            &self.outer_type_parameters,
        )?;
        marshal_field_omitempty(
            enc,
            &mut first,
            "localTypeParameters",
            &self.local_type_parameters,
        )?;
        marshal_field_omitempty(enc, &mut first, "elementFlags", &element_flags)?;
        marshal_field_omitempty(enc, &mut first, "fixedLength", &self.fixed_length)?;
        marshal_field_omitempty(enc, &mut first, "readonly", &self.tuple_readonly)?;
        marshal_field_omitempty(
            enc,
            &mut first,
            "labeledElementDeclarations",
            &self.labeled_element_declarations,
        )?;
        marshal_field_omitzero(enc, &mut first, "objectType", &self.object_type)?;
        marshal_field_omitzero(enc, &mut first, "indexType", &self.index_type)?;
        marshal_field_omitzero(enc, &mut first, "checkType", &self.check_type)?;
        marshal_field_omitzero(enc, &mut first, "extendsType", &self.extends_type)?;
        marshal_field_omitzero(enc, &mut first, "baseType", &self.base_type)?;
        marshal_field_omitzero(enc, &mut first, "substConstraint", &self.subst_constraint)?;
        marshal_field_omitzero(enc, &mut first, "typeParameter", &self.type_parameter)?;
        marshal_field_omitzero(enc, &mut first, "constraintType", &self.constraint_type)?;
        marshal_field_omitzero(enc, &mut first, "nameType", &self.name_type)?;
        marshal_field_omitzero(enc, &mut first, "templateType", &self.template_type)?;
        marshal_field_omitempty(enc, &mut first, "texts", &self.texts)?;
        marshal_field_omitzero(enc, &mut first, "freshType", &self.fresh_type)?;
        marshal_field_omitzero(enc, &mut first, "regularType", &self.regular_type)?;
        marshal_field_omitempty(enc, &mut first, "isThisType", &self.is_this_type)?;
        marshal_field_omitzero(enc, &mut first, "thisType", &self.this_type)?;
        marshal_field_omitempty(enc, &mut first, "intrinsicName", &self.intrinsic_name)?;
        marshal_field_omitempty(
            enc,
            &mut first,
            "aliasTypeArguments",
            &self.alias_type_arguments,
        )?;
        marshal_field_omitempty(enc, &mut first, "aliasSymbol", &self.alias_symbol)?;
        marshal_field_omitempty(enc, &mut first, "symbol", &self.symbol)?;
        write_object_end(enc);
        Ok(())
    }
}

// Go: proto.go:1248 newTypeResponse
// PORT: Go reads the type through its pointer; the port reads it from the
// checker arena that owns `t`.
pub fn new_type_response(c: &Checker, t: TypeId, id: TypeID) -> TypeResponse {
    let ty = c.ty(t);
    let mut resp = TypeResponse {
        id,
        flags: ty.flags().0,
        ..TypeResponse::default()
    };

    // ts#64518: the symbol and alias symbol references are set by
    // `SnapshotData::new_type_response`.
    if let Some(alias) = ty.alias() {
        resp.alias_type_arguments = type_handles(alias.type_arguments());
    }

    let flags = ty.flags();
    if flags.intersects(TypeFlags::FRESHABLE) {
        let lit = ty.as_literal_type();
        if flags.intersects(TypeFlags::LITERAL) {
            resp.value = literal_value_to_json(lit.value());
        }
        if lit.fresh_type().is_some() {
            resp.fresh_type = type_handle(lit.fresh_type());
        }
        if lit.regular_type().is_some() {
            resp.regular_type = type_handle(lit.regular_type());
        }
    } else if flags.intersects(TypeFlags::OBJECT) {
        resp.object_flags = ty.object_flags().0;
        // ts#64080
        resp.is_tuple_type = c.is_tuple_type_exported(t);
        let object_flags = ty.object_flags();
        if object_flags.intersects(ObjectFlags::REFERENCE) {
            // PORT: Go takes `ref := t.AsTypeReference()` and calls the
            // promoted `Type.Target()` of the same type.
            let _ = ty.as_type_reference();
            if c.is_tuple_type_target(t) {
                let tuple = ty.as_tuple_type();
                resp.element_flags = tuple.element_flags();
                let fixed_len = tuple.fixed_length();
                resp.fixed_length = Some(fixed_len);
                let is_readonly = tuple.is_readonly();
                resp.tuple_readonly = Some(is_readonly);
            }
            if ty.target().is_some() {
                resp.target = type_handle(ty.target());
            }
        }
        if object_flags.intersects(ObjectFlags::CLASS_OR_INTERFACE) {
            let iface = ty.as_interface_type();
            resp.type_parameters = type_handles(iface.type_parameters());
            resp.outer_type_parameters = type_handles(iface.outer_type_parameters());
            resp.local_type_parameters = type_handles(iface.local_type_parameters());
            // ts#64264
            // PORT: Go `iface.ThisType()` returns this field.
            if iface.this_type.is_some() {
                resp.this_type = type_handle(iface.this_type);
            }
        }
    } else if flags.intersects(TypeFlags::UNION_OR_INTERSECTION) {
        // types omitted; fetched via separate request
    } else if flags.intersects(TypeFlags::INDEX) {
        resp.target = type_handle(ty.as_index_type().target());
    } else if flags.intersects(TypeFlags::INDEXED_ACCESS) {
        let data = ty.as_indexed_access_type();
        resp.object_type = type_handle(data.object_type());
        resp.index_type = type_handle(data.index_type());
    } else if flags.intersects(TypeFlags::CONDITIONAL) {
        let data = ty.as_conditional_type();
        resp.check_type = type_handle(data.check_type());
        resp.extends_type = type_handle(data.extends_type());
    } else if flags.intersects(TypeFlags::SUBSTITUTION) {
        let data = ty.as_substitution_type();
        resp.base_type = type_handle(data.base_type());
        resp.subst_constraint = type_handle(data.subst_constraint());
    } else if flags.intersects(TypeFlags::TEMPLATE_LITERAL) {
        let tl = ty.as_template_literal_type();
        resp.texts = tl.texts().to_vec();
        // types omitted; fetched via separate request
    } else if flags.intersects(TypeFlags::STRING_MAPPING) {
        resp.target = type_handle(ty.as_string_mapping_type().target());
    } else if flags.intersects(TypeFlags::TYPE_PARAMETER) {
        resp.is_this_type = ty.as_type_parameter().is_this_type();
    } else if flags.intersects(TypeFlags::INTRINSIC) {
        resp.intrinsic_name = ty.as_intrinsic_type().intrinsic_name().to_string();
    }

    resp
}

// Go: proto.go:1283 typeHandles
pub fn type_handles(types: &[TypeId]) -> Vec<TypeID> {
    if types.is_empty() {
        return Vec::new();
    }
    let mut handles = Vec::with_capacity(types.len());
    for &t in types {
        handles.push(type_handle(t));
    }
    handles
}

// Go: proto.go:1294 literalValueToJSON
// PORT: the Go `any` value is `Option<&LiteralValue>` (nil is `None`); the
// result holds the same JSON primitive as `LspAny`.
pub fn literal_value_to_json(value: Option<&LiteralValue>) -> LspAny {
    match value {
        Some(LiteralValue::String(v)) => LspAny::String(v.clone()),
        Some(LiteralValue::Number(v)) => {
            // ts#64241
            if v.is_infinite() {
                if v.0 > 0.0 {
                    return LspAny::String("+Infinity".to_string());
                }
                return LspAny::String("-Infinity".to_string());
            }
            if v.is_nan() {
                return LspAny::String("NaN".to_string());
            }
            LspAny::Number(v.0)
        }
        Some(LiteralValue::Bool(v)) => LspAny::Bool(*v),
        // Encode bigint literals as a signed decimal string (e.g. "-123"); the
        // API client decodes this back into a real bigint. JSON has no bigint.
        Some(LiteralValue::PseudoBigInt(v)) => LspAny::String(v.to_string()),
        None => LspAny::Null,
    }
}

// Go: proto.go ConstantValueResponse (ts#64241)
// PORT: Go `Value any` holds a JSON primitive (`literalValueToJSON`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ConstantValueResponse {
    pub is_number: bool,
    pub value: LspAny,
}

proto_json!(marshal ConstantValueResponse {
    is_number: "isNumber" plain,
    value: "value" plain,
});

// Go: proto.go:1370 SignatureResponse
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SignatureResponse {
    pub id: SignatureID,
    pub flags: u32,
    pub declaration: NodeHandle,
    pub type_parameters: Vec<TypeID>,
    // ts#64518
    pub parameters: Vec<CompactSymbolReference>,
    pub this_parameter: Option<CompactSymbolReference>,
    pub target: SignatureID,
}

proto_json!(marshal SignatureResponse {
    id: "id" plain,
    flags: "flags" plain,
    declaration: "declaration" omitempty,
    type_parameters: "typeParameters" omitempty,
    parameters: "parameters" omitempty,
    this_parameter: "thisParameter" omitempty,
    target: "target" omitzero,
});

// Go: proto.go:1335 GetSourceFileParams
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GetSourceFileParams {
    pub snapshot: SnapshotID,
    pub project: project::ID,
    pub file: DocumentIdentifier,
}

proto_json!(both GetSourceFileParams {
    snapshot: "snapshot" plain,
    project: "project" plain,
    file: "file" plain,
});

// Go: proto.go:1341 GetSourceFileNamesParams
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GetSourceFileNamesParams {
    pub snapshot: SnapshotID,
    pub project: project::ID,
}

proto_json!(both GetSourceFileNamesParams {
    snapshot: "snapshot" plain,
    project: "project" plain,
});

// PORT: Go `core.ModuleKind` (and its alias `core.ResolutionMode`) and
// `core.ScriptKind` are Go int32 types with no JSON methods, so JSON uses the
// v2 int arshaler. The params of ts#64247, ts#64292 and ts#64216 decode
// them. Errors name the Go type.
macro_rules! core_int_json {
    ($($ty:ident: $go:literal),* $(,)?) => {$(
        impl MarshalerTo for $ty {
            fn marshal_json_to(&self, enc: &mut String) -> Result<(), JsonError> {
                self.0.marshal_json_to(enc)
            }
        }

        impl UnmarshalerFrom for $ty {
            fn unmarshal_json_from(&mut self, dec: &mut JsonDecoder<'_>) -> Result<(), JsonError> {
                self.0
                    .unmarshal_json_from(dec)
                    .map_err(|err| match SemanticError::of(&err) {
                        Some(mut s) => {
                            s.go_type = $go.to_string();
                            s.into_json_error()
                        }
                        None => err,
                    })
            }
        }

        impl IsZero for $ty {
            fn is_zero(&self) -> bool {
                self.0 == 0
            }
        }
    )*};
}

core_int_json!(ModuleKind: "core.ModuleKind", ScriptKind: "core.ScriptKind");

// Go: proto.go GetModeForUsageLocationParams (ts#64292)
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GetModeForUsageLocationParams {
    pub snapshot: SnapshotID,
    pub project: project::ID,
    pub file: DocumentIdentifier,
    pub usage: NodeHandle,
}

proto_json!(both GetModeForUsageLocationParams {
    snapshot: "snapshot" plain,
    project: "project" plain,
    file: "file" plain,
    usage: "usage" plain,
});

// Go: proto.go GetModeForResolutionAtIndexParams (ts#64292)
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GetModeForResolutionAtIndexParams {
    pub snapshot: SnapshotID,
    pub project: project::ID,
    pub file: DocumentIdentifier,
    pub index: i32,
}

proto_json!(both GetModeForResolutionAtIndexParams {
    snapshot: "snapshot" plain,
    project: "project" plain,
    file: "file" plain,
    index: "index" plain,
});

// Go: proto.go GetResolvedModuleParams (ts#64247)
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GetResolvedModuleParams {
    pub snapshot: SnapshotID,
    pub project: project::ID,
    pub file: DocumentIdentifier,
    pub module_name: String,
    pub mode: ModuleKind, // Go core.ResolutionMode
}

proto_json!(both GetResolvedModuleParams {
    snapshot: "snapshot" plain,
    project: "project" plain,
    file: "file" plain,
    module_name: "moduleName" plain,
    mode: "mode" plain,
});

// Go: proto.go GetResolvedModuleFromModuleSpecifierParams (ts#64247)
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GetResolvedModuleFromModuleSpecifierParams {
    pub snapshot: SnapshotID,
    pub project: project::ID,
    pub module_specifier: NodeHandle,
    pub source_file: Option<DocumentIdentifier>,
}

proto_json!(both GetResolvedModuleFromModuleSpecifierParams {
    snapshot: "snapshot" plain,
    project: "project" plain,
    module_specifier: "moduleSpecifier" plain,
    source_file: "sourceFile" omitempty,
});

// Go: proto.go GetResolvedTypeReferenceDirectiveParams (ts#64247)
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GetResolvedTypeReferenceDirectiveParams {
    pub snapshot: SnapshotID,
    pub project: project::ID,
    pub file: DocumentIdentifier,
    pub type_directive_name: String,
    pub mode: ModuleKind, // Go core.ResolutionMode
}

proto_json!(both GetResolvedTypeReferenceDirectiveParams {
    snapshot: "snapshot" plain,
    project: "project" plain,
    file: "file" plain,
    type_directive_name: "typeDirectiveName" plain,
    mode: "mode" plain,
});

// Go: proto.go GetResolvedTypeReferenceDirectiveFromReferenceParams (ts#64247)
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GetResolvedTypeReferenceDirectiveFromReferenceParams {
    pub snapshot: SnapshotID,
    pub project: project::ID,
    pub source_file: DocumentIdentifier,
    pub type_directive_name: String,
    pub resolution_mode: ModuleKind, // Go core.ResolutionMode
}

proto_json!(both GetResolvedTypeReferenceDirectiveFromReferenceParams {
    snapshot: "snapshot" plain,
    project: "project" plain,
    source_file: "sourceFile" plain,
    type_directive_name: "typeDirectiveName" plain,
    resolution_mode: "resolutionMode" plain,
});

// Go: proto.go PackageId (ts#64247)
// PORT: `crate::program::PackageId` is Go `module.PackageId`; code outside
// this file names the api one `proto::PackageId`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PackageId {
    pub name: String,
    pub sub_module_name: String,
    pub version: String,
    pub peer_dependencies: String,
}

proto_json!(both PackageId {
    name: "name" plain,
    sub_module_name: "subModuleName" plain,
    version: "version" plain,
    peer_dependencies: "peerDependencies" plain,
});

// Go: proto.go NewPackageId (ts#64247)
pub fn new_package_id(package_id: &crate::program::PackageId) -> Option<PackageId> {
    if package_id.name.is_empty() {
        return None;
    }
    Some(PackageId {
        name: package_id.name.clone(),
        sub_module_name: package_id.sub_module_name.clone(),
        version: package_id.version.clone(),
        peer_dependencies: package_id.peer_dependencies.clone(),
    })
}

// Go: proto.go ResolvedModule (ts#64247)
// PORT: `crate::program::ResolvedModule` is Go `module.ResolvedModule`; code
// outside this file names the api one `proto::ResolvedModule`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ResolvedModule {
    pub resolved_file_name: String,
    pub original_path: String,
    pub extension: String,
    pub resolved_using_ts_extension: bool,
    pub resolved_using_extra_extensions: bool,
    pub package_id: Option<PackageId>,
    pub is_external_library_import: bool,
    pub alternate_result: String,
}

proto_json!(marshal ResolvedModule {
    resolved_file_name: "resolvedFileName" plain,
    original_path: "originalPath" omitempty,
    extension: "extension" plain,
    resolved_using_ts_extension: "resolvedUsingTsExtension" omitempty,
    resolved_using_extra_extensions: "resolvedUsingExtraExtensions" omitempty,
    package_id: "packageId" omitempty,
    is_external_library_import: "isExternalLibraryImport" omitempty,
    alternate_result: "alternateResult" omitempty,
});

// Go: proto.go ResolvedTypeReferenceDirective (ts#64247)
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ResolvedTypeReferenceDirective {
    pub primary: bool,
    pub resolved_file_name: String,
    pub original_path: String,
    pub package_id: Option<PackageId>,
    pub is_external_library_import: bool,
}

proto_json!(marshal ResolvedTypeReferenceDirective {
    primary: "primary" plain,
    resolved_file_name: "resolvedFileName" plain,
    original_path: "originalPath" omitempty,
    package_id: "packageId" omitempty,
    is_external_library_import: "isExternalLibraryImport" omitempty,
});

// SourceFileMetadata carries program-stored metadata about a single source file.
// Go: proto.go:1430 SourceFileMetadata
// PORT: Go `core.ResolutionMode` marshals as its int32 value.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SourceFileMetadata {
    pub is_default_library: bool,
    pub is_from_external_library: bool,
    pub package_json_type: String,
    pub package_json_directory: String,
    pub implied_node_format: i32,
}

proto_json!(marshal SourceFileMetadata {
    is_default_library: "isDefaultLibrary" plain,
    is_from_external_library: "isFromExternalLibrary" plain,
    package_json_type: "packageJsonType" plain,
    package_json_directory: "packageJsonDirectory" plain,
    implied_node_format: "impliedNodeFormat" plain,
});

// Go: proto.go:1438 ResolveNameParams
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ResolveNameParams {
    pub snapshot: SnapshotID,
    pub project: project::ID,
    pub name: String,
    pub location: NodeHandle, // Optional: node handle for location context
    pub file: Option<DocumentIdentifier>, // Optional: file for location context (alternative to Location)
    pub position: Option<u32>, // Optional: position in file for location context (with File)
    pub meaning: u32,          // SymbolFlags for what kind of symbol to find
    pub exclude_globals: bool, // Whether to exclude global symbols
}

proto_json!(both ResolveNameParams {
    snapshot: "snapshot" plain,
    project: "project" plain,
    name: "name" plain,
    location: "location" omitempty,
    file: "file" omitempty,
    position: "position" omitempty,
    meaning: "meaning" plain,
    exclude_globals: "excludeGlobals" omitempty,
});

// GetSymbolsInScopeParams are parameters for getSymbolsInScope, which returns
// all symbols visible at a given location.
// Go: proto.go:1451 GetSymbolsInScopeParams
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GetSymbolsInScopeParams {
    pub snapshot: SnapshotID,
    pub project: project::ID,
    pub location: NodeHandle, // Optional: node handle for location context
    pub file: Option<DocumentIdentifier>, // Optional: file for location context (alternative to Location)
    pub position: Option<u32>, // Optional: position in file for location context (with File)
    pub meaning: u32,          // SymbolFlags for what kind of symbols to find
}

proto_json!(both GetSymbolsInScopeParams {
    snapshot: "snapshot" plain,
    project: "project" plain,
    location: "location" omitempty,
    file: "file" omitempty,
    position: "position" omitempty,
    meaning: "meaning" plain,
});

// GetTypePropertyParams is used for all type sub-property endpoints.
// Go: proto.go:1461 GetTypePropertyParams
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GetTypePropertyParams {
    pub snapshot: SnapshotID,
    pub project: project::ID,
    pub type_: TypeID,
}

proto_json!(both GetTypePropertyParams {
    snapshot: "snapshot" plain,
    project: "project" plain,
    type_: "objectId" plain,
});

// GetSymbolPropertyParams is used for all symbol sub-property endpoints.
// Go: proto.go:1513 GetSymbolPropertyParams
#[derive(Clone, Debug, Default, PartialEq)]
// ts#64518: only the symbol reference, which names its snapshot and project.
pub struct GetSymbolPropertyParams {
    pub symbol: SymbolReference,
}

proto_json!(both GetSymbolPropertyParams {
    symbol: "symbol" plain,
});

// GetSignaturePropertyParams is used for all signature sub-property endpoints.
// Go: proto.go:1475 GetSignaturePropertyParams
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GetSignaturePropertyParams {
    pub snapshot: SnapshotID,
    pub project: project::ID,
    pub signature: SignatureID,
}

proto_json!(both GetSignaturePropertyParams {
    snapshot: "snapshot" plain,
    project: "project" plain,
    signature: "objectId" plain,
});

// GetContextualTypeParams returns the contextual type for a node.
// Go: proto.go:1482 GetContextualTypeParams
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GetContextualTypeParams {
    pub snapshot: SnapshotID,
    pub project: project::ID,
    pub location: NodeHandle,
}

proto_json!(both GetContextualTypeParams {
    snapshot: "snapshot" plain,
    project: "project" plain,
    location: "location" plain,
});

// Go: proto.go GetContextualTypeForArgumentParams (ts#64264)
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GetContextualTypeForArgumentParams {
    pub snapshot: SnapshotID,
    pub project: project::ID,
    pub location: NodeHandle,
    pub index: i32,
}

proto_json!(both GetContextualTypeForArgumentParams {
    snapshot: "snapshot" plain,
    project: "project" plain,
    location: "location" plain,
    index: "index" plain,
});

// GetTypeOfSymbolAtLocationParams returns the narrowed type of a symbol at a specific location.
// Go: proto.go:1539 GetTypeOfSymbolAtLocationParams
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GetTypeOfSymbolAtLocationParams {
    pub snapshot: SnapshotID,
    pub project: project::ID,
    pub symbol: SymbolReference,
    pub location: NodeHandle,
}

proto_json!(both GetTypeOfSymbolAtLocationParams {
    snapshot: "snapshot" plain,
    project: "project" plain,
    symbol: "symbol" plain,
    location: "location" plain,
});

// GetReferencesToSymbolInFileParams are the parameters for the getReferencesToSymbolInFile method.
// Go: proto.go:1547 GetReferencesToSymbolInFileParams
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GetReferencesToSymbolInFileParams {
    pub snapshot: SnapshotID,
    pub project: project::ID,
    pub file: DocumentIdentifier,
    pub symbol: SymbolReference,
}

proto_json!(both GetReferencesToSymbolInFileParams {
    snapshot: "snapshot" plain,
    project: "project" plain,
    file: "file" plain,
    symbol: "symbol" plain,
});

// GetReferencedSymbolsForNodeParams are the parameters for the getReferencedSymbolsForNode method.
// Go: proto.go:1512 GetReferencedSymbolsForNodeParams
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GetReferencedSymbolsForNodeParams {
    pub snapshot: SnapshotID,
    pub project: project::ID,
    pub node: NodeHandle,
    pub position: i32,
}

proto_json!(both GetReferencedSymbolsForNodeParams {
    snapshot: "snapshot" plain,
    project: "project" plain,
    node: "node" plain,
    position: "position" plain,
});

// ReferencedSymbolEntry represents a symbol definition and its references.
// Go: proto.go:1520 ReferencedSymbolEntry
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ReferencedSymbolEntry {
    pub definition: NodeHandle,
    pub symbol: Option<SymbolResponse>,
    pub references: Vec<NodeHandle>,
}

proto_json!(marshal ReferencedSymbolEntry {
    definition: "definition" plain,
    symbol: "symbol" omitempty,
    references: "references" plain,
});

// GetSignatureUsagesParams are the parameters for the getSignatureUsages method.
// Go: proto.go:1527 GetSignatureUsagesParams
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GetSignatureUsagesParams {
    pub snapshot: SnapshotID,
    pub project: project::ID,
    pub signature_decl: NodeHandle,
}

proto_json!(both GetSignatureUsagesParams {
    snapshot: "snapshot" plain,
    project: "project" plain,
    signature_decl: "signatureDecl" plain,
});

// SignatureUsageResponse represents a single usage of a signature as a name-call pair.
// Go: proto.go:1534 SignatureUsageResponse
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SignatureUsageResponse {
    pub name: NodeHandle,
    pub call: NodeHandle,
}

proto_json!(marshal SignatureUsageResponse {
    name: "name" plain,
    call: "call" omitempty,
});

// GetCompletionsAtPositionParams are the parameters for the getCompletionsAtPosition method.
// Go: proto.go:1540 GetCompletionsAtPositionParams
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GetCompletionsAtPositionParams {
    pub snapshot: SnapshotID,
    pub project: project::ID,
    pub file: DocumentIdentifier,
    pub position: u32,
    pub trigger_character: Option<String>,
    pub include_symbol: bool,
}

proto_json!(both GetCompletionsAtPositionParams {
    snapshot: "snapshot" plain,
    project: "project" plain,
    file: "file" plain,
    position: "position" plain,
    trigger_character: "triggerCharacter" omitempty,
    include_symbol: "includeSymbol" omitempty,
});

// CompletionEntryLabelDetailsResponse holds additional label display text for a completion entry.
// Go: proto.go:1550 CompletionEntryLabelDetailsResponse
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CompletionEntryLabelDetailsResponse {
    pub detail: Option<String>,
    pub description: Option<String>,
}

proto_json!(marshal CompletionEntryLabelDetailsResponse {
    detail: "detail" omitempty,
    description: "description" omitempty,
});

// CompletionEntryResponse represents a single completion item.
// Go: proto.go:1556 CompletionEntryResponse
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CompletionEntryResponse {
    pub name: String,
    pub kind: u32,
    pub sort_text: Option<String>,
    pub insert_text: Option<String>,
    pub filter_text: Option<String>,
    pub detail: Option<String>,
    pub label_details: Option<CompletionEntryLabelDetailsResponse>,
    pub symbol: Option<SymbolResponse>,
}

proto_json!(marshal CompletionEntryResponse {
    name: "name" plain,
    kind: "kind" omitempty,
    sort_text: "sortText" omitempty,
    insert_text: "insertText" omitempty,
    filter_text: "filterText" omitempty,
    detail: "detail" omitempty,
    label_details: "labelDetails" omitempty,
    symbol: "symbol" omitempty,
});

// CompletionInfoResponse wraps a list of completion entries.
// Go: proto.go:1568 CompletionInfoResponse
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CompletionInfoResponse {
    pub is_incomplete: bool,
    pub entries: Vec<CompletionEntryResponse>,
}

proto_json!(marshal CompletionInfoResponse {
    is_incomplete: "isIncomplete" plain,
    entries: "entries" plain,
});

// GetIntrinsicTypeParams is used for intrinsic type getters (anyType, stringType, etc.).
// Go: proto.go:1574 GetIntrinsicTypeParams
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GetIntrinsicTypeParams {
    pub snapshot: SnapshotID,
    pub project: project::ID,
}

proto_json!(both GetIntrinsicTypeParams {
    snapshot: "snapshot" plain,
    project: "project" plain,
});

// WellKnownSymbolsResponse carries the handle ids of the per-checker singleton
// symbols (unknown, undefined, arguments) so the client can identify them by id
// without a round-trip on every check.
// Go: proto.go:1582 WellKnownSymbolsResponse
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WellKnownSymbolsResponse {
    pub unknown: SymbolID,
    pub undefined: SymbolID,
    pub arguments: SymbolID,
}

proto_json!(marshal WellKnownSymbolsResponse {
    unknown: "unknown" plain,
    undefined: "undefined" plain,
    arguments: "arguments" plain,
});

// WellKnownSignaturesResponse carries the handle id of the per-checker singleton
// unknown signature (the signature the checker yields when a call cannot be
// resolved) so the client can identify it by id without a round-trip on every check.
// Go: proto.go:1591 WellKnownSignaturesResponse
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WellKnownSignaturesResponse {
    pub unknown: SignatureID,
}

proto_json!(marshal WellKnownSignaturesResponse {
    unknown: "unknown" plain,
});

// GetBaseTypeOfLiteralTypeParams returns the base type of a literal type.
// Go: proto.go:1596 GetBaseTypeOfLiteralTypeParams
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GetBaseTypeOfLiteralTypeParams {
    pub snapshot: SnapshotID,
    pub project: project::ID,
    pub type_: TypeID,
}

proto_json!(both GetBaseTypeOfLiteralTypeParams {
    snapshot: "snapshot" plain,
    project: "project" plain,
    type_: "type" plain,
});

// GetNonNullableTypeParams are the parameters for the getNonNullableType method.
// Go: proto.go:1603 GetNonNullableTypeParams
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GetNonNullableTypeParams {
    pub snapshot: SnapshotID,
    pub project: project::ID,
    pub type_: TypeID,
}

proto_json!(both GetNonNullableTypeParams {
    snapshot: "snapshot" plain,
    project: "project" plain,
    type_: "type" plain,
});

// GetTypeFromTypeNodeParams are the parameters for the getTypeFromTypeNode method.
// Go: proto.go:1610 GetTypeFromTypeNodeParams
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GetTypeFromTypeNodeParams {
    pub snapshot: SnapshotID,
    pub project: project::ID,
    pub location: NodeHandle,
}

proto_json!(both GetTypeFromTypeNodeParams {
    snapshot: "snapshot" plain,
    project: "project" plain,
    location: "location" plain,
});

// GetWidenedTypeParams are the parameters for the getWidenedType method.
// Go: proto.go:1617 GetWidenedTypeParams
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GetWidenedTypeParams {
    pub snapshot: SnapshotID,
    pub project: project::ID,
    pub type_: TypeID,
}

proto_json!(both GetWidenedTypeParams {
    snapshot: "snapshot" plain,
    project: "project" plain,
    type_: "type" plain,
});

// GetParameterTypeParams are the parameters for the getParameterType method.
// Go: proto.go:1624 GetParameterTypeParams
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GetParameterTypeParams {
    pub snapshot: SnapshotID,
    pub project: project::ID,
    pub signature: SignatureID,
    pub index: i32,
}

proto_json!(both GetParameterTypeParams {
    snapshot: "snapshot" plain,
    project: "project" plain,
    signature: "signature" plain,
    index: "index" plain,
});

// IsArrayLikeTypeParams checks whether a type is array-like.
// Go: proto.go:1632 IsArrayLikeTypeParams
#[derive(Clone, Debug, Default, PartialEq)]
pub struct IsArrayLikeTypeParams {
    pub snapshot: SnapshotID,
    pub project: project::ID,
    pub type_: TypeID,
}

proto_json!(both IsArrayLikeTypeParams {
    snapshot: "snapshot" plain,
    project: "project" plain,
    type_: "type" plain,
});

// IsTypeAssignableToParams checks assignability between two types.
// Go: proto.go:1639 IsTypeAssignableToParams
#[derive(Clone, Debug, Default, PartialEq)]
pub struct IsTypeAssignableToParams {
    pub snapshot: SnapshotID,
    pub project: project::ID,
    pub source: TypeID,
    pub target: TypeID,
}

proto_json!(both IsTypeAssignableToParams {
    snapshot: "snapshot" plain,
    project: "project" plain,
    source: "source" plain,
    target: "target" plain,
});

// Go: proto.go:1646 GetSignaturesOfTypeParams
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GetSignaturesOfTypeParams {
    pub snapshot: SnapshotID,
    pub project: project::ID,
    pub type_: TypeID,
    pub kind: i32,
}

proto_json!(both GetSignaturesOfTypeParams {
    snapshot: "snapshot" plain,
    project: "project" plain,
    type_: "type" plain,
    kind: "kind" plain,
});

// Go: proto.go:1653 GetResolvedSignatureParams
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GetResolvedSignatureParams {
    pub snapshot: SnapshotID,
    pub project: project::ID,
    pub location: NodeHandle,
}

proto_json!(both GetResolvedSignatureParams {
    snapshot: "snapshot" plain,
    project: "project" plain,
    location: "location" plain,
});

// Go: proto.go:1659 GetTypeAtLocationParams
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GetTypeAtLocationParams {
    pub snapshot: SnapshotID,
    pub project: project::ID,
    pub location: NodeHandle,
}

proto_json!(both GetTypeAtLocationParams {
    snapshot: "snapshot" plain,
    project: "project" plain,
    location: "location" plain,
});

// Go: proto.go:1665 GetTypeAtLocationsParams
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GetTypeAtLocationsParams {
    pub snapshot: SnapshotID,
    pub project: project::ID,
    pub locations: Vec<NodeHandle>,
}

proto_json!(both GetTypeAtLocationsParams {
    snapshot: "snapshot" plain,
    project: "project" plain,
    locations: "locations" plain,
});

// Go: proto.go:1671 GetTypeAtPositionParams
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GetTypeAtPositionParams {
    pub snapshot: SnapshotID,
    pub project: project::ID,
    pub file: DocumentIdentifier,
    pub position: u32,
}

proto_json!(both GetTypeAtPositionParams {
    snapshot: "snapshot" plain,
    project: "project" plain,
    file: "file" plain,
    position: "position" plain,
});

// Go: proto.go:1678 GetTypesAtPositionsParams
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GetTypesAtPositionsParams {
    pub snapshot: SnapshotID,
    pub project: project::ID,
    pub file: DocumentIdentifier,
    pub positions: Vec<u32>,
}

proto_json!(both GetTypesAtPositionsParams {
    snapshot: "snapshot" plain,
    project: "project" plain,
    file: "file" plain,
    positions: "positions" plain,
});

// Go: proto.go:1685 ImportAdderActionKind (tsgo#3881)
// PORT: a Go string type, a newtype as the handles are (see `handle_json!`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ImportAdderActionKind(pub String);

handle_json!(string: ImportAdderActionKind);

// Go: proto.go:1688 ImportAdderActionKindImportSymbol (tsgo#3881)
pub const IMPORT_ADDER_ACTION_KIND_IMPORT_SYMBOL: &str = "importSymbol";

// Go: proto.go:1734 ImportAdderAction (tsgo#3881)
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ImportAdderAction {
    pub kind: ImportAdderActionKind,
    // ts#64518
    pub symbol: Option<SymbolReference>,
    pub is_valid_type_only_use_site: Option<bool>,
}

proto_json!(both ImportAdderAction {
    kind: "kind" plain,
    symbol: "symbol" omitempty,
    is_valid_type_only_use_site: "isValidTypeOnlyUseSite" omitempty,
});

// Go: proto.go:1697 GetImportAdderEditsParams (tsgo#3881)
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GetImportAdderEditsParams {
    pub snapshot: SnapshotID,
    pub project: project::ID,
    pub file: DocumentIdentifier,
    pub actions: Vec<ImportAdderAction>,
}

proto_json!(both GetImportAdderEditsParams {
    snapshot: "snapshot" plain,
    project: "project" plain,
    file: "file" plain,
    actions: "actions" plain,
});

// Go: proto.go:1704 TextEdit (tsgo#3881)
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TextEdit {
    pub pos: i32,
    pub end: i32,
    pub new_text: String,
}

proto_json!(marshal TextEdit {
    pos: "pos" plain,
    end: "end" plain,
    new_text: "newText" plain,
});

// TypeToTypeNodeParams are the parameters for the typeToTypeNode method.
// Go: proto.go:1711 TypeToTypeNodeParams
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TypeToTypeNodeParams {
    pub snapshot: SnapshotID,
    pub project: project::ID,
    pub type_: TypeID,
    pub location: NodeHandle,
    pub flags: i32,
}

proto_json!(both TypeToTypeNodeParams {
    snapshot: "snapshot" plain,
    project: "project" plain,
    type_: "type" plain,
    location: "location" omitempty,
    flags: "flags" omitempty,
});

// SignatureToSignatureDeclarationParams are the parameters for the signatureToSignatureDeclaration method.
// Go: proto.go:1720 SignatureToSignatureDeclarationParams
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SignatureToSignatureDeclarationParams {
    pub snapshot: SnapshotID,
    pub project: project::ID,
    pub signature: SignatureID,
    pub kind: i32,
    pub location: NodeHandle,
    pub flags: i32,
}

proto_json!(both SignatureToSignatureDeclarationParams {
    snapshot: "snapshot" plain,
    project: "project" plain,
    signature: "signature" plain,
    kind: "kind" plain,
    location: "location" omitempty,
    flags: "flags" omitempty,
});

// PrintNodeParams are the parameters for the printNode method.
// Go: proto.go:1730 PrintNodeParams
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PrintNodeParams {
    pub data: String, // base64-encoded binary AST data
    pub preserve_source_newlines: bool,
    pub never_ascii_escape: bool,
    pub terminate_unterminated_literals: bool,
}

proto_json!(both PrintNodeParams {
    data: "data" plain,
    preserve_source_newlines: "preserveSourceNewlines" omitempty,
    never_ascii_escape: "neverAsciiEscape" omitempty,
    terminate_unterminated_literals: "terminateUnterminatedLiterals" omitempty,
});

// Go: proto.go:1737 EmitParams (tsgo#4699)
// PORT: Go `*uint32` is `Option<u32>`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EmitParams {
    pub snapshot: SnapshotID,
    pub project: project::ID,
    pub emit_only: Option<u32>,
}

proto_json!(both EmitParams {
    snapshot: "snapshot" plain,
    project: "project" plain,
    emit_only: "emitOnly" omitempty,
});

// Go: proto.go:1743 SelectedFilesEmitParams (tsgo#4699)
// PORT: Go tells a nil `Files` (absent or `null`) from an empty one, so it
// is `Option<Vec>`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SelectedFilesEmitParams {
    pub snapshot: SnapshotID,
    pub project: project::ID,
    pub files: Option<Vec<DocumentIdentifier>>,
}

proto_json!(both SelectedFilesEmitParams {
    snapshot: "snapshot" plain,
    project: "project" plain,
    files: "files" plain,
});

// Go: proto.go:1749 EmitResponse (tsgo#4699)
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EmitResponse {
    pub emit_skipped: bool,
    pub diagnostics: Vec<DiagnosticResponse>,
    pub emitted_files: Vec<String>,
    // EmittedFilesContents contains contents parallel to EmittedFiles when the
    // source snapshot uses a full filesystem. It is empty for write-through emits.
    // ts#64115
    pub emitted_files_contents: Vec<String>,
}

proto_json!(marshal EmitResponse {
    emit_skipped: "emitSkipped" plain,
    diagnostics: "diagnostics" plain,
    emitted_files: "emittedFiles" plain,
    emitted_files_contents: "emittedFilesContents" plain,
});

// Go: proto.go:1758 EmitOutputFile (tsgo#4699)
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EmitOutputFile {
    pub file_name: String,
    pub text: String,
    pub source_file_name: Option<String>,
}

proto_json!(marshal EmitOutputFile {
    file_name: "fileName" plain,
    text: "text" plain,
    source_file_name: "sourceFileName" omitempty,
});

// Go: proto.go:1764 EmitOutputResponse (tsgo#4699)
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EmitOutputResponse {
    pub emit_skipped: bool,
    pub diagnostics: Vec<DiagnosticResponse>,
    pub output_files: Vec<EmitOutputFile>,
}

proto_json!(marshal EmitOutputResponse {
    emit_skipped: "emitSkipped" plain,
    diagnostics: "diagnostics" plain,
    output_files: "outputFiles" plain,
});

// FormatNodeForInsertionParams are the parameters for the formatNodeForInsertion method.
// Go: proto.go:1771 FormatNodeForInsertionParams
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FormatNodeForInsertionParams {
    pub snapshot: SnapshotID,
    pub project: project::ID,
    pub file: DocumentIdentifier, // target file where the node will be inserted
    pub position: u32, // UTF-16 code-unit offset of the insertion position in the target file
    pub data: String,  // base64-encoded binary AST data for the synthesized node
}

proto_json!(both FormatNodeForInsertionParams {
    snapshot: "snapshot" plain,
    project: "project" plain,
    file: "file" plain,
    position: "position" plain,
    data: "data" plain,
});

// CheckerTypeParams are parameters for checker methods that operate on a type.
// Go: proto.go:1780 CheckerTypeParams
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CheckerTypeParams {
    pub snapshot: SnapshotID,
    pub project: project::ID,
    pub type_: TypeID,
}

proto_json!(both CheckerTypeParams {
    snapshot: "snapshot" plain,
    project: "project" plain,
    type_: "type" plain,
});

// GetPropertyOfTypeParams are parameters for getPropertyOfType (a named property of a type).
// Go: proto.go:1787 GetPropertyOfTypeParams
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GetPropertyOfTypeParams {
    pub snapshot: SnapshotID,
    pub project: project::ID,
    pub type_: TypeID,
    pub name: String,
}

proto_json!(both GetPropertyOfTypeParams {
    snapshot: "snapshot" plain,
    project: "project" plain,
    type_: "type" plain,
    name: "name" plain,
});

// Go: proto.go GetIndexInfoOfTypeParams (ts#64264)
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GetIndexInfoOfTypeParams {
    pub snapshot: SnapshotID,
    pub project: project::ID,
    pub type_: TypeID,
    pub kind: i32,
}

proto_json!(both GetIndexInfoOfTypeParams {
    snapshot: "snapshot" plain,
    project: "project" plain,
    type_: "type" plain,
    kind: "kind" plain,
});

// CheckerNodeParams are parameters for checker methods that operate on a node location.
// Go: proto.go:1810 CheckerNodeParams
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CheckerNodeParams {
    pub snapshot: SnapshotID,
    pub project: project::ID,
    pub location: NodeHandle,
}

proto_json!(both CheckerNodeParams {
    snapshot: "snapshot" plain,
    project: "project" plain,
    location: "location" plain,
});

// GetMemberInModuleExportsParams are parameters for getMemberInModuleExports.
// Go: proto.go:1845 GetMemberInModuleExportsParams
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GetMemberInModuleExportsParams {
    pub snapshot: SnapshotID,
    pub project: project::ID,
    pub symbol: SymbolReference,
    pub name: String,
}

proto_json!(both GetMemberInModuleExportsParams {
    snapshot: "snapshot" plain,
    project: "project" plain,
    symbol: "symbol" plain,
    name: "name" plain,
});

// CheckerSymbolParams are parameters for checker methods that operate on a symbol.
// Go: proto.go:1860 CheckerSymbolParams
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CheckerSymbolParams {
    pub snapshot: SnapshotID,
    pub project: project::ID,
    pub symbol: SymbolReference,
}

proto_json!(both CheckerSymbolParams {
    snapshot: "snapshot" plain,
    project: "project" plain,
    symbol: "symbol" plain,
});

// JSDocTagInfo is a single JSDoc tag, mirroring Strada's JSDocTagInfo but with the tag text
// rendered as a plain string rather than SymbolDisplayPart[].
// Go: proto.go:1825 JSDocTagInfo
#[derive(Clone, Debug, Default, PartialEq)]
pub struct JSDocTagInfo {
    pub name: String,
    pub text: String,
}

proto_json!(marshal JSDocTagInfo {
    name: "name" plain,
    text: "text" omitempty,
});

// CheckerSignatureParams are parameters for checker methods that operate on a signature.
// Go: proto.go:1831 CheckerSignatureParams
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CheckerSignatureParams {
    pub snapshot: SnapshotID,
    pub project: project::ID,
    pub signature: SignatureID,
}

proto_json!(both CheckerSignatureParams {
    snapshot: "snapshot" plain,
    project: "project" plain,
    signature: "signature" plain,
});

// TypePredicateResponse is the response for getTypePredicateOfSignature.
// Go: proto.go:1838 TypePredicateResponse
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TypePredicateResponse {
    pub kind: i32,
    pub parameter_index: i32,
    pub parameter_name: String,
    pub type_: Option<TypeResponse>,
}

proto_json!(marshal TypePredicateResponse {
    kind: "kind" plain,
    parameter_index: "parameterIndex" plain,
    parameter_name: "parameterName" omitempty,
    type_: "type" omitempty,
});

// IndexInfoResponse represents a single index signature.
// Go: proto.go:1846 IndexInfoResponse
#[derive(Clone, Debug, Default, PartialEq)]
pub struct IndexInfoResponse {
    pub key_type: TypeResponse,
    pub value_type: TypeResponse,
    pub is_readonly: bool,
    pub declaration: NodeHandle,
}

proto_json!(marshal IndexInfoResponse {
    key_type: "keyType" plain,
    value_type: "valueType" plain,
    is_readonly: "isReadonly" omitempty,
    declaration: "declaration" omitempty,
});

// SourceFileResponse contains the binary-encoded AST data for a source file.
// The Data field is base64-encoded binary data in the encoder's format.
// Go: proto.go:1855 SourceFileResponse
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SourceFileResponse {
    // Data is the base64-encoded binary AST data in the encoder's format.
    pub data: String,
}

// Go: the v2 default struct marshal (`data` is `plain`).
// PERF: (apiperf1) `data` is base64 text, which has no byte that JSON
// escapes, so `PlainJsonText` copies it as it is. The string marshal of
// each char was 12% of an API createSourceFile loop of a 260 KB file.
impl MarshalerTo for SourceFileResponse {
    fn marshal_json_to(&self, enc: &mut String) -> Result<(), JsonError> {
        write_object_start(enc);
        let mut first = true;
        marshal_field(enc, &mut first, "data", &PlainJsonText(&self.data))?;
        write_object_end(enc);
        Ok(())
    }
}

/// PERF: (apiperf1) the string marshal of a text that may need no JSON
/// escape (`SourceFileResponse.data`). When every byte is ASCII and none is
/// a control character, `"` or `\`, the string marshal writes the text as
/// it is between quotes, so this copies it at once. Other text takes the
/// string marshal.
struct PlainJsonText<'a>(&'a str);

impl MarshalerTo for PlainJsonText<'_> {
    fn marshal_json_to(&self, enc: &mut String) -> Result<(), JsonError> {
        // A fold over each chunk, with no early exit inside it, so the test
        // is vector code.
        let plain = self.0.as_bytes().chunks(64).all(|chunk| {
            chunk.iter().fold(true, |plain, &b| {
                plain & (b >= 0x20) & (b < 0x80) & (b != b'"') & (b != b'\\')
            })
        });
        if !plain {
            return self.0.marshal_json_to(enc);
        }
        enc.reserve(self.0.len() + 2);
        enc.push('"');
        enc.push_str(self.0);
        enc.push('"');
        Ok(())
    }
}

// GetDiagnosticsParams are parameters for per-file diagnostic methods.
// Go: proto.go:1861 GetDiagnosticsParams
#[derive(Clone, Debug, Default, PartialEq)]
// PORT: Go `Files []DocumentIdentifier` is `Option`: `None` is a nil slice
// (no `files`, or `null`) and `Some(vec![])` is `[]`. `getDiagnostics`
// tells them apart.
pub struct GetDiagnosticsParams {
    pub snapshot: SnapshotID,
    pub project: project::ID,
    pub files: Option<Vec<DocumentIdentifier>>,
}

proto_json!(both GetDiagnosticsParams {
    snapshot: "snapshot" plain,
    project: "project" plain,
    files: "files" omitempty,
});

// GetProjectDiagnosticsParams are parameters for project-wide diagnostic methods.
// Go: proto.go:1868 GetProjectDiagnosticsParams
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GetProjectDiagnosticsParams {
    pub snapshot: SnapshotID,
    pub project: project::ID,
}

proto_json!(both GetProjectDiagnosticsParams {
    snapshot: "snapshot" plain,
    project: "project" plain,
});

// DiagnosticResponse is the API response for a single diagnostic.
// Go: proto.go:1874 DiagnosticResponse
// PORT: `Default` is written below; `diagnostics.Category` has none in the
// port.
#[derive(Clone, Debug, PartialEq)]
pub struct DiagnosticResponse {
    // FileName is the path of the file this diagnostic belongs to, if any.
    pub file_name: String,
    // Pos is the start position of the diagnostic in the source file.
    pub pos: i32,
    // End is the end position of the diagnostic in the source file.
    pub end: i32,
    // StartPosition is the zero-based line and UTF-16 character position of Pos.
    pub start_position: Option<DiagnosticPositionResponse>,
    // EndPosition is the zero-based line and UTF-16 character position of End.
    pub end_position: Option<DiagnosticPositionResponse>,
    // SourceLines contains the source lines needed to render this diagnostic with context.
    pub source_lines: Vec<DiagnosticSourceLineResponse>,
    // Code is the diagnostic error code.
    pub code: i32,
    // Category is the diagnostic category (error, warning, suggestion, message).
    pub category: crate::diagnostics::Category,
    // Source is a custom diagnostic-code prefix. An empty value uses the default "TS".
    pub source: String,
    // Text is the localized diagnostic message text.
    pub text: String,
    // ReportsUnnecessary indicates this diagnostic highlights unnecessary code.
    pub reports_unnecessary: bool,
    // ReportsDeprecated indicates this diagnostic highlights deprecated code.
    pub reports_deprecated: bool,
    // MessageChain contains chained diagnostic messages, if any.
    pub message_chain: Vec<DiagnosticResponse>,
    // RelatedInformation contains related diagnostic information, if any.
    pub related_information: Vec<DiagnosticResponse>,
}

impl MarshalerTo for DiagnosticResponse {
    fn marshal_json_to(&self, enc: &mut String) -> Result<(), JsonError> {
        write_object_start(enc);
        let mut first = true;
        marshal_field_omitempty(enc, &mut first, "fileName", &self.file_name)?;
        marshal_field(enc, &mut first, "pos", &self.pos)?;
        marshal_field(enc, &mut first, "end", &self.end)?;
        marshal_field_omitempty(enc, &mut first, "startPosition", &self.start_position)?;
        marshal_field_omitempty(enc, &mut first, "endPosition", &self.end_position)?;
        marshal_field_omitempty(enc, &mut first, "sourceLines", &self.source_lines)?;
        marshal_field(enc, &mut first, "code", &self.code)?;
        // PORT: `diagnostics.Category` is a Go int32 type; it writes as a number.
        marshal_field(enc, &mut first, "category", &self.category.0)?;
        marshal_field_omitempty(enc, &mut first, "source", &self.source)?;
        marshal_field(enc, &mut first, "text", &self.text)?;
        marshal_field_omitzero(
            enc,
            &mut first,
            "reportsUnnecessary",
            &self.reports_unnecessary,
        )?;
        marshal_field_omitzero(
            enc,
            &mut first,
            "reportsDeprecated",
            &self.reports_deprecated,
        )?;
        marshal_field_omitempty(enc, &mut first, "messageChain", &self.message_chain)?;
        marshal_field_omitempty(
            enc,
            &mut first,
            "relatedInformation",
            &self.related_information,
        )?;
        write_object_end(enc);
        Ok(())
    }
}

// PORT: Go `diagnostics.Category` is a Go int32 type; the Go zero value is
// `CategoryWarning`.
impl Default for DiagnosticResponse {
    fn default() -> Self {
        DiagnosticResponse {
            file_name: String::new(),
            pos: 0,
            end: 0,
            start_position: None,
            end_position: None,
            source_lines: Vec::new(),
            code: 0,
            category: crate::diagnostics::Category::Warning,
            source: String::new(),
            text: String::new(),
            reports_unnecessary: false,
            reports_deprecated: false,
            message_chain: Vec::new(),
            related_information: Vec::new(),
        }
    }
}

// PORT: the Go v2 default struct unmarshal of `DiagnosticResponse`
// (`CreateProgramOptions.ConfigFileParsingDiagnostics`, ts#63950).
impl UnmarshalerFrom for DiagnosticResponse {
    fn unmarshal_json_from(&mut self, dec: &mut JsonDecoder<'_>) -> Result<(), JsonError> {
        let is_object = unmarshal_struct_fields(dec, "api.DiagnosticResponse", |name, dec| {
            match name {
                "fileName" => json_unmarshal_decode(dec, &mut self.file_name)?,
                "pos" => json_unmarshal_decode(dec, &mut self.pos)?,
                "end" => json_unmarshal_decode(dec, &mut self.end)?,
                "startPosition" => json_unmarshal_decode(dec, &mut self.start_position)?,
                "endPosition" => json_unmarshal_decode(dec, &mut self.end_position)?,
                "sourceLines" => json_unmarshal_decode(dec, &mut self.source_lines)?,
                "code" => json_unmarshal_decode(dec, &mut self.code)?,
                "category" => {
                    // Go `diagnostics.Category` is an int32 type: any int32
                    // is kept (diagnostics/diagnostics.go:18), and an error
                    // names that type.
                    let mut category: i32 = 0;
                    json_unmarshal_decode(dec, &mut category).map_err(
                        |err| match SemanticError::of(&err) {
                            Some(mut s) => {
                                s.go_type = "diagnostics.Category".to_string();
                                s.into_json_error()
                            }
                            None => err,
                        },
                    )?;
                    self.category = crate::diagnostics::Category(category);
                }
                "source" => json_unmarshal_decode(dec, &mut self.source)?,
                "text" => json_unmarshal_decode(dec, &mut self.text)?,
                "reportsUnnecessary" => json_unmarshal_decode(dec, &mut self.reports_unnecessary)?,
                "reportsDeprecated" => json_unmarshal_decode(dec, &mut self.reports_deprecated)?,
                "messageChain" => json_unmarshal_decode(dec, &mut self.message_chain)?,
                "relatedInformation" => json_unmarshal_decode(dec, &mut self.related_information)?,
                _ => return Ok(false),
            }
            Ok(true)
        })?;
        if !is_object {
            *self = DiagnosticResponse::default();
        }
        Ok(())
    }
}

// Go: proto.go DiagnosticResponse.ToDiagnostic (ts#63950)
impl DiagnosticResponse {
    pub fn to_diagnostic(&self) -> Diagnostic {
        new_diagnostic_from_text(
            Node::NIL,
            TextRange::new(self.pos, self.end),
            self.code,
            self.category,
            &self.text,
            self.message_chain
                .iter()
                .map(DiagnosticResponse::to_diagnostic)
                .collect(),
            self.related_information
                .iter()
                .map(DiagnosticResponse::to_diagnostic)
                .collect(),
            self.reports_unnecessary,
            self.reports_deprecated,
        )
    }
}

// Go: proto.go DiagnosticPositionResponse (ts#63935)
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DiagnosticPositionResponse {
    pub line: i32,
    // PORT: Go `core.UTF16Offset`.
    pub character: i32,
}

proto_json!(both DiagnosticPositionResponse {
    line: "line" plain,
    character: "character" plain,
});

// Go: proto.go DiagnosticSourceLineResponse (ts#63935)
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DiagnosticSourceLineResponse {
    pub line: i32,
    pub text: String,
}

proto_json!(both DiagnosticSourceLineResponse {
    line: "line" plain,
    text: "text" plain,
});

// Go: proto.go diagnosticSourceLines (ts#63935)
// PORT: Go returns nil for an empty line map; the field is `omitempty`, so
// an empty `Vec` writes the same JSON.
fn diagnostic_source_lines(
    file: &diagnosticwriter::FileLike,
    first_line: i32,
    last_line: i32,
) -> Vec<DiagnosticSourceLineResponse> {
    let line_map = file.ecma_line_map();
    if line_map.is_empty() {
        return Vec::new();
    }

    let mut lines: Vec<i32> = Vec::with_capacity((last_line - first_line + 1).clamp(0, 4) as usize);
    if last_line - first_line >= 4 {
        lines.extend([first_line, first_line + 1, last_line - 1, last_line]);
    } else {
        for line in first_line..=last_line {
            lines.push(line);
        }
    }

    let text = file.text();
    let mut result = Vec::with_capacity(lines.len());
    for line in lines {
        let start = line_map[line as usize] as usize;
        let mut end = text.len();
        if ((line + 1) as usize) < line_map.len() {
            end = line_map[(line + 1) as usize] as usize;
        }
        result.push(DiagnosticSourceLineResponse {
            line,
            text: text[start..end].to_string(),
        });
    }
    result
}

// Go: proto.go:1944 NewDiagnosticResponse
// NewDiagnosticResponse converts an ast.Diagnostic to a DiagnosticResponse.
pub fn new_diagnostic_response(d: &Diagnostic) -> DiagnosticResponse {
    new_diagnostic_response_wrapped(diagnosticwriter::wrap_ast_diagnostic(d))
}

// Go: proto.go newDiagnosticResponse (ts#63935)
// PORT: Go names it `newDiagnosticResponse`, which snakes to the name of the
// exported function; the wrapped form gets the `_wrapped` suffix.
fn new_diagnostic_response_wrapped(d: diagnosticwriter::AstDiagnostic<'_>) -> DiagnosticResponse {
    let file = d.file();
    let (mut pos, mut end) = (d.pos(), d.end());
    if let Some(file) = &file {
        let text_len = file.text().len() as i32;
        pos = 0.max(pos.min(text_len));
        end = pos.max(end.min(text_len));
    }
    let mut resp = DiagnosticResponse {
        file_name: String::new(),
        pos,
        end,
        start_position: None,
        end_position: None,
        source_lines: Vec::new(),
        code: d.0.code,
        category: d.0.category,
        source: d.source().to_string(),
        text: d.0.localize(&crate::locale::DEFAULT),
        reports_unnecessary: d.0.reports_unnecessary,
        reports_deprecated: d.0.reports_deprecated,
        message_chain: Vec::new(),
        related_information: Vec::new(),
    };

    if let Some(file) = &file {
        resp.file_name = file.file_name().to_string();
        if let diagnosticwriter::FileLike::Source(source_file) = file {
            let position_map = source_file_get_position_map(*source_file);
            resp.pos = position_map.utf8_to_utf16(pos);
            resp.end = position_map.utf8_to_utf16(end);
        } else {
            // Go `int(core.UTF16Len(text[:pos]))`.
            let text = file.text();
            resp.pos = utf16_len_of_range(&text, 0, pos as usize);
            resp.end = utf16_len_of_range(&text, 0, end as usize);
        }
        let (start_line, start_character) =
            diagnosticwriter::get_ecma_line_and_utf16_character_of_file_position(file, pos);
        let (end_line, end_character) =
            diagnosticwriter::get_ecma_line_and_utf16_character_of_file_position(file, end);
        resp.start_position = Some(DiagnosticPositionResponse {
            line: start_line,
            character: start_character,
        });
        resp.end_position = Some(DiagnosticPositionResponse {
            line: end_line,
            character: end_character,
        });
        resp.source_lines = diagnostic_source_lines(file, start_line, end_line);
    }

    let chain = d.message_chain();
    if !chain.is_empty() {
        resp.message_chain = Vec::with_capacity(chain.len());
        for c in &chain {
            resp.message_chain.push(new_diagnostic_response_wrapped(
                diagnosticwriter::wrap_ast_diagnostic(c),
            ));
        }
    }

    let related: Vec<_> = d.related_information().collect();
    if !related.is_empty() {
        resp.related_information = Vec::with_capacity(related.len());
        for r in related {
            resp.related_information
                .push(new_diagnostic_response_wrapped(r));
        }
    }

    resp
}

// Go: proto.go:2015 NewDiagnosticResponses
// NewDiagnosticResponses converts a slice of ast.Diagnostics to DiagnosticResponses.
pub fn new_diagnostic_responses(diags: &[Diagnostic]) -> Vec<DiagnosticResponse> {
    if diags.is_empty() {
        return Vec::new();
    }
    let mut result = Vec::with_capacity(diags.len());
    for d in diags {
        result.push(new_diagnostic_response(d));
    }
    result
}

// Go: proto.go:2026 unmarshalPayload
pub fn unmarshal_payload(
    method: &str,
    payload: impl AsRef<[u8]>,
) -> Result<Option<Box<dyn AnyValue>>, GoError> {
    let Some(unmarshaler) = UNMARSHALERS.get(&Method(Cow::Owned(method.to_string()))) else {
        return Err(errors::new(format!(
            "unknown API method {}",
            strconv::quote(method)
        )));
    };
    unmarshaler(payload.as_ref())
}

// Go: proto.go:2034 unmarshallerFor
// `unmarshal_root` is Go `json.Unmarshal` with the v2 error text.
pub fn unmarshaller_for<T: UnmarshalerFrom + Default + AnyValue>(
    data: &[u8],
) -> Result<Option<Box<dyn AnyValue>>, GoError> {
    let mut v = T::default();
    if let Err(err) = unmarshal_root(data, &mut v) {
        let err = errors::from_value(err);
        return Err(errors::errorf(
            format!(
                "failed to unmarshal *{}: {}",
                go_type_name::<T>(),
                err.error()
            ),
            vec![err],
        ));
    }
    Ok(Some(Box::new(v)))
}

// Go: proto.go:2042 noParams
pub fn no_params(data: &[u8]) -> Result<Option<Box<dyn AnyValue>>, GoError> {
    let _ = data;
    Ok(None)
}

// PORT: the JSON form of `core.CompilerOptions` (`CompilerOptionsJSON`,
// `TypeAcquisitionJSON` and the `CompilerOptions` decode) and
// `marshal_field_omitempty` live in `crate::options_json`, so the compiler
// side (content mappers) does not depend on `api`. Go keeps them in `core`
// (struct tags) and in the json package.
pub use crate::options_json::{CompilerOptionsJSON, TypeAcquisitionJSON, marshal_field_omitempty};

#[cfg(test)]
mod source_file_response_tests {
    use super::*;
    use crate::frontend::json::json_marshal;

    // apiperf1: the marshal of `SourceFileResponse` is the v2 struct marshal
    // of its string, for base64 text (copied as it is) and for text with
    // bytes that JSON escapes or that are not ASCII (the string marshal).
    #[test]
    fn data_is_the_string_marshal() {
        for data in ["", "Zm9vYmE=", "ab+/09==", "a\"b\\c\n\u{1}", "\u{7f}é"] {
            let response = SourceFileResponse {
                data: data.to_string(),
            };
            let want = format!(
                "{{\"data\":{}}}",
                json_marshal(&data.to_string(), &[]).expect("a string marshals")
            );
            assert_eq!(json_marshal(&response, &[]).expect("it marshals"), want);
        }
    }
}

#[cfg(test)]
mod config_file_response_tests {
    use super::*;
    use crate::frontend::tsoptions::new_parsed_command_line;
    use crate::frontend::tspath::ComparePathsOptions;
    use std::rc::Rc;

    // Go NewConfigFileResponse reads `p.Errors` (api/proto.go:996), where
    // checkSourceFilesBelongToPath (tsoptions/parsedcommandline.go:181)
    // appended TS6059 when the project's program read
    // `CommonSourceDirectory` (projfix1 skeptic: a referenced project with
    // outDir, `rootDir: src` and a file outside src gives
    // `parsedCommandLine.errors: [TS6059]` in Go N's createSnapshot).
    #[test]
    fn errors_hold_the_common_source_directory_errors_like_go() {
        let command_line = new_parsed_command_line(
            Rc::new(CompilerOptions {
                root_dir: "/p/src".to_string(),
                out_dir: "/p/out".to_string(),
                ..Default::default()
            }),
            vec!["/p/src/a.ts".to_string(), "/p/extra.ts".to_string()],
            None,
            ComparePathsOptions {
                use_case_sensitive_file_names: true,
                current_directory: "/p".to_string(),
            },
        );
        let codes = |command_line| {
            new_config_file_response(Some(command_line))
                .expect("a response")
                .errors
                .iter()
                .map(|error| error.code)
                .collect::<Vec<_>>()
        };
        assert!(codes(&command_line).is_empty());
        assert_eq!(command_line.common_source_directory(), "/p/src/");
        assert_eq!(codes(&command_line), [6059]);
    }
}

#[cfg(test)]
mod unmarshal_error_tests {
    use super::*;

    fn err_text<T: UnmarshalerFrom + Default + AnyValue>(data: &str) -> String {
        unmarshaller_for::<T>(data.as_bytes())
            .expect_err("want an error")
            .error()
    }

    // Expected texts come from the pinned Go API (tests2 S5-003 goldens and
    // Go JSON v2); Go writes "cannot" or "unable to".
    #[test]
    fn decode_errors_match_go() {
        assert_eq!(
            err_text::<GetSymbolAtPositionParams>(
                r#"{"snapshot":1,"project":"p","file":"a.ts","position":"x"}"#
            ),
            r#"failed to unmarshal *api.GetSymbolAtPositionParams: json: cannot unmarshal JSON string into Go uint32 within "/position""#
        );
        assert_eq!(
            err_text::<ReleaseParams>(r#"{"snapshot":"x"}"#),
            r#"failed to unmarshal *api.ReleaseParams: json: cannot unmarshal JSON string into Go api.SnapshotID within "/snapshot""#
        );
        assert_eq!(
            err_text::<GetSymbolsAtPositionsParams>(r#"{"snapshot":1,"positions":[1,"x"]}"#),
            r#"failed to unmarshal *api.GetSymbolsAtPositionsParams: json: cannot unmarshal JSON string into Go uint32 within "/positions/1""#
        );
        assert_eq!(
            err_text::<GetSourceFileParams>(r#"{"snapshot":1,"file":5}"#),
            r#"failed to unmarshal *api.GetSourceFileParams: json: cannot unmarshal into Go api.DocumentIdentifier within "/file": DocumentIdentifier: expected string or object, got number"#
        );
    }

    // Go `diagnostics.Category` is `type Category int32`
    // (diagnostics/diagnostics.go:18): a config file parsing diagnostic that
    // the client sends keeps any int32 category, and a number out of range
    // names that type. The error text is Go N's for the same
    // `updateSnapshot` request.
    #[test]
    fn a_sent_diagnostic_category_is_any_int32() {
        let decode = |category: &str| {
            let data = format!(
                r#"{{"configFileParsingDiagnostics":[{{"code":1,"category":{category},"text":"x"}}]}}"#
            );
            let mut options = CreateProgramOptions::default();
            crate::frontend::json_ext::unmarshal_root(data.as_bytes(), &mut options)
                .map(|()| options.config_file_parsing_diagnostics[0].category)
                .map_err(|err| err.message)
        };
        assert_eq!(decode("1"), Ok(crate::diagnostics::Category::Error));
        assert_eq!(decode("999"), Ok(crate::diagnostics::Category(999)));
        assert_eq!(
            decode("-2147483648"),
            Ok(crate::diagnostics::Category(i32::MIN))
        );
        assert_eq!(
            decode("3000000000"),
            Err(r#"json: cannot unmarshal JSON number 3000000000 into Go diagnostics.Category within "/configFileParsingDiagnostics/0/category": value out of range"#.to_string())
        );
    }

    // tsgo#4849: the client sends back the `compilerOptions` that the API
    // wrote (`CompilerOptionsJSON`). Decoding them and writing them again
    // gives the same text. The options are those of the hono-ext traces.
    #[test]
    fn transpile_compiler_options_round_trip() {
        let options = r#"{"composite":true,"declaration":true,"forceConsistentCasingInFileNames":true,"module":6,"moduleResolution":100,"noUnusedLocals":true,"noUnusedParameters":true,"outDir":"/p/dist/types","paths":{"a/*":["b/*"],"c":["d"]},"rootDir":"/p/src","skipLibCheck":true,"strict":true,"target":9,"types":["node"],"esModuleInterop":true,"configFilePath":"/p/tsconfig.build.json"}"#;
        let data = format!(
            r#"{{"fileName":"/p/src/a.ts","options":{{"compilerOptions":{options},"reportDiagnostics":true}}}}"#
        );
        let parsed = unmarshaller_for::<TranspileFromFileParams>(data.as_bytes())
            .expect("valid params")
            .expect("params value");
        let params = parsed
            .downcast_ref::<TranspileFromFileParams>()
            .expect("TranspileFromFileParams");
        assert!(params.options.report_diagnostics);
        let compiler_options = params
            .options
            .compiler_options
            .as_ref()
            .expect("compilerOptions");
        assert_eq!(compiler_options.module, ModuleKind(6));
        assert_eq!(compiler_options.strict, Tristate::True);
        let written =
            crate::frontend::json::json_marshal(&CompilerOptionsJSON(compiler_options), &[])
                .expect("marshal");
        assert_eq!(written, options);
    }
}
