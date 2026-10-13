"use strict";

// Enum and flag translation between tsc-rs (TypeScript 7.1 numbering, the numbers the API sends)
// and TypeScript 6.0.3. tsc-rs numbers come from crates/ts_goport/src/astdata/syntax_kind.rs and
// crates/ts_goport/src/flags.rs (same values as packages/typescript/src/enums at the pin); TS 6.0
// numbers are the pinned 6.0.3 runtime enums. test/translate.test.mjs checks every entry against both.

// Index = tsc-rs SyntaxKind number.
const NATIVE_SYNTAX_KIND_NAMES = [
    "Unknown", "EndOfFile", "SingleLineCommentTrivia", "MultiLineCommentTrivia", "NewLineTrivia",
    "WhitespaceTrivia", "ConflictMarkerTrivia", "NonTextFileMarkerTrivia", "NumericLiteral",
    "BigIntLiteral", "StringLiteral", "JsxText", "JsxTextAllWhiteSpaces",
    "RegularExpressionLiteral", "NoSubstitutionTemplateLiteral", "TemplateHead", "TemplateMiddle",
    "TemplateTail", "OpenBraceToken", "CloseBraceToken", "OpenParenToken", "CloseParenToken",
    "OpenBracketToken", "CloseBracketToken", "DotToken", "DotDotDotToken", "SemicolonToken",
    "CommaToken", "QuestionDotToken", "LessThanToken", "LessThanSlashToken", "GreaterThanToken",
    "LessThanEqualsToken", "GreaterThanEqualsToken", "EqualsEqualsToken", "ExclamationEqualsToken",
    "EqualsEqualsEqualsToken", "ExclamationEqualsEqualsToken", "EqualsGreaterThanToken",
    "PlusToken", "MinusToken", "AsteriskToken", "AsteriskAsteriskToken", "SlashToken",
    "PercentToken", "PlusPlusToken", "MinusMinusToken", "LessThanLessThanToken",
    "GreaterThanGreaterThanToken", "GreaterThanGreaterThanGreaterThanToken", "AmpersandToken",
    "BarToken", "CaretToken", "ExclamationToken", "TildeToken", "AmpersandAmpersandToken",
    "BarBarToken", "QuestionToken", "ColonToken", "AtToken", "QuestionQuestionToken",
    "BacktickToken", "HashToken", "EqualsToken", "PlusEqualsToken", "MinusEqualsToken",
    "AsteriskEqualsToken", "AsteriskAsteriskEqualsToken", "SlashEqualsToken", "PercentEqualsToken",
    "LessThanLessThanEqualsToken", "GreaterThanGreaterThanEqualsToken",
    "GreaterThanGreaterThanGreaterThanEqualsToken", "AmpersandEqualsToken", "BarEqualsToken",
    "BarBarEqualsToken", "AmpersandAmpersandEqualsToken", "QuestionQuestionEqualsToken",
    "CaretEqualsToken", "Identifier", "PrivateIdentifier", "JSDocCommentTextToken", "BreakKeyword",
    "CaseKeyword", "CatchKeyword", "ClassKeyword", "ConstKeyword", "ContinueKeyword",
    "DebuggerKeyword", "DefaultKeyword", "DeleteKeyword", "DoKeyword", "ElseKeyword", "EnumKeyword",
    "ExportKeyword", "ExtendsKeyword", "FalseKeyword", "FinallyKeyword", "ForKeyword",
    "FunctionKeyword", "IfKeyword", "ImportKeyword", "InKeyword", "InstanceOfKeyword", "NewKeyword",
    "NullKeyword", "ReturnKeyword", "SuperKeyword", "SwitchKeyword", "ThisKeyword", "ThrowKeyword",
    "TrueKeyword", "TryKeyword", "TypeOfKeyword", "VarKeyword", "VoidKeyword", "WhileKeyword",
    "WithKeyword", "ImplementsKeyword", "InterfaceKeyword", "LetKeyword", "PackageKeyword",
    "PrivateKeyword", "ProtectedKeyword", "PublicKeyword", "StaticKeyword", "YieldKeyword",
    "AbstractKeyword", "AccessorKeyword", "AsKeyword", "AssertsKeyword", "AssertKeyword",
    "AnyKeyword", "AsyncKeyword", "AwaitKeyword", "BooleanKeyword", "ConstructorKeyword",
    "DeclareKeyword", "GetKeyword", "ImmediateKeyword", "InferKeyword", "IntrinsicKeyword",
    "IsKeyword", "KeyOfKeyword", "ModuleKeyword", "NamespaceKeyword", "NeverKeyword", "OutKeyword",
    "ReadonlyKeyword", "RequireKeyword", "NumberKeyword", "ObjectKeyword", "SatisfiesKeyword",
    "SetKeyword", "StringKeyword", "SymbolKeyword", "TypeKeyword", "UndefinedKeyword",
    "UniqueKeyword", "UnknownKeyword", "UsingKeyword", "FromKeyword", "GlobalKeyword",
    "BigIntKeyword", "OverrideKeyword", "OfKeyword", "DeferKeyword", "SourceKeyword",
    "QualifiedName", "ComputedPropertyName", "TypeParameter", "Parameter", "Decorator",
    "PropertySignature", "PropertyDeclaration", "MethodSignature", "MethodDeclaration",
    "ClassStaticBlockDeclaration", "Constructor", "GetAccessor", "SetAccessor", "CallSignature",
    "ConstructSignature", "IndexSignature", "TypePredicate", "TypeReference", "FunctionType",
    "ConstructorType", "TypeQuery", "TypeLiteral", "ArrayType", "TupleType", "OptionalType",
    "RestType", "UnionType", "IntersectionType", "ConditionalType", "InferType",
    "ParenthesizedType", "ThisType", "TypeOperator", "IndexedAccessType", "MappedType",
    "LiteralType", "NamedTupleMember", "TemplateLiteralType", "TemplateLiteralTypeSpan",
    "ImportType", "ObjectBindingPattern", "ArrayBindingPattern", "BindingElement",
    "ArrayLiteralExpression", "ObjectLiteralExpression", "PropertyAccessExpression",
    "ElementAccessExpression", "CallExpression", "NewExpression", "TaggedTemplateExpression",
    "TypeAssertionExpression", "ParenthesizedExpression", "FunctionExpression", "ArrowFunction",
    "DeleteExpression", "TypeOfExpression", "VoidExpression", "AwaitExpression",
    "PrefixUnaryExpression", "PostfixUnaryExpression", "BinaryExpression", "ConditionalExpression",
    "TemplateExpression", "YieldExpression", "SpreadElement", "ClassExpression",
    "OmittedExpression", "ExpressionWithTypeArguments", "AsExpression", "NonNullExpression",
    "MetaProperty", "SyntheticExpression", "SatisfiesExpression", "TemplateSpan",
    "SemicolonClassElement", "Block", "EmptyStatement", "VariableStatement", "ExpressionStatement",
    "IfStatement", "DoStatement", "WhileStatement", "ForStatement", "ForInStatement",
    "ForOfStatement", "ContinueStatement", "BreakStatement", "ReturnStatement", "WithStatement",
    "SwitchStatement", "LabeledStatement", "ThrowStatement", "TryStatement", "DebuggerStatement",
    "VariableDeclaration", "VariableDeclarationList", "FunctionDeclaration", "ClassDeclaration",
    "InterfaceDeclaration", "TypeAliasDeclaration", "EnumDeclaration", "ModuleDeclaration",
    "ModuleBlock", "CaseBlock", "NamespaceExportDeclaration", "ImportEqualsDeclaration",
    "ImportDeclaration", "ImportClause", "NamespaceImport", "NamedImports", "ImportSpecifier",
    "ExportAssignment", "ExportDeclaration", "NamedExports", "NamespaceExport", "ExportSpecifier",
    "MissingDeclaration", "ExternalModuleReference", "JsxElement", "JsxSelfClosingElement",
    "JsxOpeningElement", "JsxClosingElement", "JsxFragment", "JsxOpeningFragment",
    "JsxClosingFragment", "JsxAttribute", "JsxAttributes", "JsxSpreadAttribute", "JsxExpression",
    "JsxNamespacedName", "CaseClause", "DefaultClause", "HeritageClause", "CatchClause",
    "ImportAttributes", "ImportAttribute", "PropertyAssignment", "ShorthandPropertyAssignment",
    "SpreadAssignment", "EnumMember", "SourceFile", "JSDocTypeExpression", "JSDocNameReference",
    "JSDocAllType", "JSDocNullableType", "JSDocNonNullableType", "JSDocOptionalType",
    "JSDocVariadicType", "JSDoc", "JSDocText", "JSDocTypeLiteral", "JSDocSignature", "JSDocLink",
    "JSDocLinkCode", "JSDocLinkPlain", "JSDocUnknownTag", "JSDocAugmentsTag", "JSDocImplementsTag",
    "JSDocDeprecatedTag", "JSDocPublicTag", "JSDocPrivateTag", "JSDocProtectedTag",
    "JSDocReadonlyTag", "JSDocOverrideTag", "JSDocCallbackTag", "JSDocOverloadTag",
    "JSDocParameterTag", "JSDocReturnTag", "JSDocThisTag", "JSDocTypeTag", "JSDocTemplateTag",
    "JSDocTypedefTag", "JSDocSeeTag", "JSDocPropertyTag", "JSDocThrowsTag", "JSDocSatisfiesTag",
    "JSDocImportTag", "SyntaxList", "JSTypeAliasDeclaration", "JSImportDeclaration",
    "NotEmittedStatement", "PartiallyEmittedExpression", "SyntheticReferenceExpression",
    "NotEmittedTypeElement", "Count",
];

// Index = TS 6.0.3 SyntaxKind number (the first name of each value, not the First*/Last* markers).
const TS6_SYNTAX_KIND_NAMES = [
    "Unknown", "EndOfFileToken", "SingleLineCommentTrivia", "MultiLineCommentTrivia",
    "NewLineTrivia", "WhitespaceTrivia", "ShebangTrivia", "ConflictMarkerTrivia",
    "NonTextFileMarkerTrivia", "NumericLiteral", "BigIntLiteral", "StringLiteral", "JsxText",
    "JsxTextAllWhiteSpaces", "RegularExpressionLiteral", "NoSubstitutionTemplateLiteral",
    "TemplateHead", "TemplateMiddle", "TemplateTail", "OpenBraceToken", "CloseBraceToken",
    "OpenParenToken", "CloseParenToken", "OpenBracketToken", "CloseBracketToken", "DotToken",
    "DotDotDotToken", "SemicolonToken", "CommaToken", "QuestionDotToken", "LessThanToken",
    "LessThanSlashToken", "GreaterThanToken", "LessThanEqualsToken", "GreaterThanEqualsToken",
    "EqualsEqualsToken", "ExclamationEqualsToken", "EqualsEqualsEqualsToken",
    "ExclamationEqualsEqualsToken", "EqualsGreaterThanToken", "PlusToken", "MinusToken",
    "AsteriskToken", "AsteriskAsteriskToken", "SlashToken", "PercentToken", "PlusPlusToken",
    "MinusMinusToken", "LessThanLessThanToken", "GreaterThanGreaterThanToken",
    "GreaterThanGreaterThanGreaterThanToken", "AmpersandToken", "BarToken", "CaretToken",
    "ExclamationToken", "TildeToken", "AmpersandAmpersandToken", "BarBarToken", "QuestionToken",
    "ColonToken", "AtToken", "QuestionQuestionToken", "BacktickToken", "HashToken", "EqualsToken",
    "PlusEqualsToken", "MinusEqualsToken", "AsteriskEqualsToken", "AsteriskAsteriskEqualsToken",
    "SlashEqualsToken", "PercentEqualsToken", "LessThanLessThanEqualsToken",
    "GreaterThanGreaterThanEqualsToken", "GreaterThanGreaterThanGreaterThanEqualsToken",
    "AmpersandEqualsToken", "BarEqualsToken", "BarBarEqualsToken", "AmpersandAmpersandEqualsToken",
    "QuestionQuestionEqualsToken", "CaretEqualsToken", "Identifier", "PrivateIdentifier",
    "JSDocCommentTextToken", "BreakKeyword", "CaseKeyword", "CatchKeyword", "ClassKeyword",
    "ConstKeyword", "ContinueKeyword", "DebuggerKeyword", "DefaultKeyword", "DeleteKeyword",
    "DoKeyword", "ElseKeyword", "EnumKeyword", "ExportKeyword", "ExtendsKeyword", "FalseKeyword",
    "FinallyKeyword", "ForKeyword", "FunctionKeyword", "IfKeyword", "ImportKeyword", "InKeyword",
    "InstanceOfKeyword", "NewKeyword", "NullKeyword", "ReturnKeyword", "SuperKeyword",
    "SwitchKeyword", "ThisKeyword", "ThrowKeyword", "TrueKeyword", "TryKeyword", "TypeOfKeyword",
    "VarKeyword", "VoidKeyword", "WhileKeyword", "WithKeyword", "ImplementsKeyword",
    "InterfaceKeyword", "LetKeyword", "PackageKeyword", "PrivateKeyword", "ProtectedKeyword",
    "PublicKeyword", "StaticKeyword", "YieldKeyword", "AbstractKeyword", "AccessorKeyword",
    "AsKeyword", "AssertsKeyword", "AssertKeyword", "AnyKeyword", "AsyncKeyword", "AwaitKeyword",
    "BooleanKeyword", "ConstructorKeyword", "DeclareKeyword", "GetKeyword", "InferKeyword",
    "IntrinsicKeyword", "IsKeyword", "KeyOfKeyword", "ModuleKeyword", "NamespaceKeyword",
    "NeverKeyword", "OutKeyword", "ReadonlyKeyword", "RequireKeyword", "NumberKeyword",
    "ObjectKeyword", "SatisfiesKeyword", "SetKeyword", "StringKeyword", "SymbolKeyword",
    "TypeKeyword", "UndefinedKeyword", "UniqueKeyword", "UnknownKeyword", "UsingKeyword",
    "FromKeyword", "GlobalKeyword", "BigIntKeyword", "OverrideKeyword", "OfKeyword", "DeferKeyword",
    "QualifiedName", "ComputedPropertyName", "TypeParameter", "Parameter", "Decorator",
    "PropertySignature", "PropertyDeclaration", "MethodSignature", "MethodDeclaration",
    "ClassStaticBlockDeclaration", "Constructor", "GetAccessor", "SetAccessor", "CallSignature",
    "ConstructSignature", "IndexSignature", "TypePredicate", "TypeReference", "FunctionType",
    "ConstructorType", "TypeQuery", "TypeLiteral", "ArrayType", "TupleType", "OptionalType",
    "RestType", "UnionType", "IntersectionType", "ConditionalType", "InferType",
    "ParenthesizedType", "ThisType", "TypeOperator", "IndexedAccessType", "MappedType",
    "LiteralType", "NamedTupleMember", "TemplateLiteralType", "TemplateLiteralTypeSpan",
    "ImportType", "ObjectBindingPattern", "ArrayBindingPattern", "BindingElement",
    "ArrayLiteralExpression", "ObjectLiteralExpression", "PropertyAccessExpression",
    "ElementAccessExpression", "CallExpression", "NewExpression", "TaggedTemplateExpression",
    "TypeAssertionExpression", "ParenthesizedExpression", "FunctionExpression", "ArrowFunction",
    "DeleteExpression", "TypeOfExpression", "VoidExpression", "AwaitExpression",
    "PrefixUnaryExpression", "PostfixUnaryExpression", "BinaryExpression", "ConditionalExpression",
    "TemplateExpression", "YieldExpression", "SpreadElement", "ClassExpression",
    "OmittedExpression", "ExpressionWithTypeArguments", "AsExpression", "NonNullExpression",
    "MetaProperty", "SyntheticExpression", "SatisfiesExpression", "TemplateSpan",
    "SemicolonClassElement", "Block", "EmptyStatement", "VariableStatement", "ExpressionStatement",
    "IfStatement", "DoStatement", "WhileStatement", "ForStatement", "ForInStatement",
    "ForOfStatement", "ContinueStatement", "BreakStatement", "ReturnStatement", "WithStatement",
    "SwitchStatement", "LabeledStatement", "ThrowStatement", "TryStatement", "DebuggerStatement",
    "VariableDeclaration", "VariableDeclarationList", "FunctionDeclaration", "ClassDeclaration",
    "InterfaceDeclaration", "TypeAliasDeclaration", "EnumDeclaration", "ModuleDeclaration",
    "ModuleBlock", "CaseBlock", "NamespaceExportDeclaration", "ImportEqualsDeclaration",
    "ImportDeclaration", "ImportClause", "NamespaceImport", "NamedImports", "ImportSpecifier",
    "ExportAssignment", "ExportDeclaration", "NamedExports", "NamespaceExport", "ExportSpecifier",
    "MissingDeclaration", "ExternalModuleReference", "JsxElement", "JsxSelfClosingElement",
    "JsxOpeningElement", "JsxClosingElement", "JsxFragment", "JsxOpeningFragment",
    "JsxClosingFragment", "JsxAttribute", "JsxAttributes", "JsxSpreadAttribute", "JsxExpression",
    "JsxNamespacedName", "CaseClause", "DefaultClause", "HeritageClause", "CatchClause",
    "ImportAttributes", "ImportAttribute", "ImportTypeAssertionContainer", "PropertyAssignment",
    "ShorthandPropertyAssignment", "SpreadAssignment", "EnumMember", "SourceFile", "Bundle",
    "JSDocTypeExpression", "JSDocNameReference", "JSDocMemberName", "JSDocAllType",
    "JSDocUnknownType", "JSDocNullableType", "JSDocNonNullableType", "JSDocOptionalType",
    "JSDocFunctionType", "JSDocVariadicType", "JSDocNamepathType", "JSDoc", "JSDocText",
    "JSDocTypeLiteral", "JSDocSignature", "JSDocLink", "JSDocLinkCode", "JSDocLinkPlain",
    "JSDocTag", "JSDocAugmentsTag", "JSDocImplementsTag", "JSDocAuthorTag", "JSDocDeprecatedTag",
    "JSDocClassTag", "JSDocPublicTag", "JSDocPrivateTag", "JSDocProtectedTag", "JSDocReadonlyTag",
    "JSDocOverrideTag", "JSDocCallbackTag", "JSDocOverloadTag", "JSDocEnumTag", "JSDocParameterTag",
    "JSDocReturnTag", "JSDocThisTag", "JSDocTypeTag", "JSDocTemplateTag", "JSDocTypedefTag",
    "JSDocSeeTag", "JSDocPropertyTag", "JSDocThrowsTag", "JSDocSatisfiesTag", "JSDocImportTag",
    "SyntaxList", "NotEmittedStatement", "NotEmittedTypeElement", "PartiallyEmittedExpression",
    "CommaListExpression", "SyntheticReferenceExpression", "Count",
];

// TS 6.0 spells these kinds differently.
const NATIVE_TO_TS6_KIND_NAME = {
    EndOfFile: "EndOfFileToken",
    JSDocUnknownTag: "JSDocTag",
};

// TS 6.0 kinds that tsc-rs parses the same source text to another kind: unknown
// JSDoc tags and `#member` names in JSDoc links.
const TS6_TO_NATIVE_KIND_NAME = {
    EndOfFileToken: "EndOfFile",
    JSDocTag: "JSDocUnknownTag",
    JSDocAuthorTag: "JSDocUnknownTag",
    JSDocClassTag: "JSDocUnknownTag",
    JSDocEnumTag: "JSDocUnknownTag",
    JSDocMemberName: "QualifiedName",
};

// Kinds with no counterpart map to undefined:
// - tsc-rs ImmediateKeyword (scanner-only contextual keyword, an Identifier in TS 6.0),
//   SourceKeyword (only the phase of `import source x`), JSTypeAliasDeclaration and
//   JSImportDeclaration (reparsed JSDoc, not in TS 6.0 trees).
// - TS 6.0 ShebangTrivia, ImportTypeAssertionContainer, Bundle, JSDocUnknownType,
//   JSDocFunctionType, JSDocNamepathType and CommaListExpression.
const syntaxKindTs6ByNative = mapKindsByName(NATIVE_SYNTAX_KIND_NAMES, TS6_SYNTAX_KIND_NAMES, NATIVE_TO_TS6_KIND_NAME);
const syntaxKindNativeByTs6 = mapKindsByName(TS6_SYNTAX_KIND_NAMES, NATIVE_SYNTAX_KIND_NAMES, TS6_TO_NATIVE_KIND_NAME);

// tsc-rs SyntaxKind numbers by name, for code that reads native syntax trees.
const nativeSyntaxKind = Object.freeze(Object.fromEntries(NATIVE_SYNTAX_KIND_NAMES.map((name, kind) => [name, kind])));

function mapKindsByName(fromNames, toNames, renames) {
    const toKinds = new Map(toNames.map((name, kind) => [name, kind]));
    return fromNames.map(name => toKinds.get(renames[name] ?? name));
}

function syntaxKindToTs6(kind) {
    return syntaxKindTs6ByNative[kind];
}

function syntaxKindToNative(kind) {
    return syntaxKindNativeByTs6[kind];
}

function nativeSyntaxKindName(kind) {
    return NATIVE_SYNTAX_KIND_NAMES[kind];
}

// A bit table maps each source bit to a target bit; bits it leaves out are dropped.
function bitTable(...pairs) {
    const table = new Int8Array(32).fill(-1);
    for (const [from, to] of pairs) {
        table[from] = to;
    }
    return table;
}

function sameBits(first, last) {
    const pairs = [];
    for (let bit = first; bit <= last; bit++) {
        pairs.push([bit, bit]);
    }
    return pairs;
}

function remapBits(flags, table) {
    let result = 0;
    for (let bit = 0; bit < 32; bit++) {
        if ((flags & (1 << bit)) !== 0 && table[bit] >= 0) {
            result |= 1 << table[bit];
        }
    }
    return result;
}

function invertTable(table) {
    const inverse = new Int8Array(32).fill(-1);
    table.forEach((to, from) => {
        if (to >= 0 && inverse[to] < 0) {
            inverse[to] = from;
        }
    });
    return inverse;
}

const TYPE_FLAGS_OBJECT = 1 << 20;

// tsc-rs adds MembersResolved (21) and FromTypeNode (29), which TS 6.0 does not have, and moves
// SingleSignatureType from the top of the object bits.
const OBJECT_TYPE_OBJECT_FLAGS = bitTable(
    ...sameBits(0, 20),
    [22, 21], // ContainsSpread
    [23, 22], // ObjectRestType
    [24, 23], // InstantiationExpressionType
    [25, 27], // SingleSignatureType
    [26, 24], // IsClassInstanceClone
    [27, 25], // IdenticalBaseTypeCalculated
    [28, 26], // IdenticalBaseTypeExists
);

// Unions, intersections and instantiable types reuse the high bits (IsGenericTypeComputed and up);
// tsc-rs IsUniformEnumComputed and IsUniformEnum (28, 29) have no TS 6.0 bit.
const OTHER_TYPE_OBJECT_FLAGS = bitTable(
    ...sameBits(0, 20),
    [22, 21], // IsGenericTypeComputed
    [23, 22], // IsGenericObjectType
    [24, 23], // IsGenericIndexType
    [25, 24], // ContainsIntersections | IsNeverIntersectionComputed
    [26, 25], // IsUnknownLikeUnionComputed | IsNeverIntersection
    [27, 26], // IsUnknownLikeUnion | IsConstrainedTypeVariable
);

// The meaning of the high object flag bits depends on the type kind, so pass the type's flags.
function objectFlagsToTs6(flags, typeFlags = TYPE_FLAGS_OBJECT) {
    return remapBits(flags, (typeFlags & TYPE_FLAGS_OBJECT) !== 0 ? OBJECT_TYPE_OBJECT_FLAGS : OTHER_TYPE_OBJECT_FLAGS);
}

// tsc-rs adds ContainsWritePublic/Protected/Private (11-13) and IsDiscriminantComputed,
// IsDiscriminant and IndexSymbol (24-26).
const CHECK_FLAGS = bitTable(
    ...sameBits(0, 10),
    [14, 11], // ContainsStatic
    [15, 12], // Late
    [16, 13], // ReverseMapped
    [17, 14], // OptionalParameter
    [18, 15], // RestParameter
    [19, 16], // DeferredType
    [20, 17], // HasNeverType
    [21, 18], // Mapped
    [22, 19], // StripOptional
    [23, 20], // Unresolved
);

function checkFlagsToTs6(flags) {
    return remapBits(flags, CHECK_FLAGS);
}

// tsc-rs adds Construct (2).
const SIGNATURE_FLAGS = bitTable(
    [0, 0], // HasRestParameter
    [1, 1], // HasLiteralTypes
    [3, 2], // Abstract
    [4, 3], // IsInnerCallChain
    [5, 4], // IsOuterCallChain
    [6, 5], // IsUntypedSignatureInJSFile
    [7, 6], // IsNonInferrable
    [8, 7], // IsSignatureCandidateForOverloadFailure
);

function signatureFlagsToTs6(flags) {
    return remapBits(flags, SIGNATURE_FLAGS);
}

// The single symbol flags are the same; tsc-rs adds ConstEnumOnlyModule, ReplaceableByMethod and
// GlobalLookup (28-30).
const SYMBOL_FLAGS_MASK = (1 << 28) - 1;

function symbolFlagsToTs6(flags) {
    return flags & SYMBOL_FLAGS_MASK;
}

// TS 6.0 SymbolFlags.All is -1.
function symbolFlagsToNative(flags) {
    return (flags & SYMBOL_FLAGS_MASK) >>> 0;
}

// TypeFlags and ElementFlags have the same bits.
function typeFlagsToTs6(flags) {
    return flags;
}

function elementFlagsToTs6(flags) {
    return flags;
}

// TS 6.0 NodeBuilderFlags without the bits tsc-rs gives new meanings (WriteCallStyleSignature 27,
// UseInstantiationExpressions 30).
const TS6_NODE_BUILDER_FLAGS = 0x37ffffff;

function nodeBuilderFlagsToNative(flags) {
    return (flags & TS6_NODE_BUILDER_FLAGS) >>> 0;
}

// Every TypeFormatFlags bit TS 6.0 defines; the others mean nothing in TS 6.0.
const TS6_TYPE_FORMAT_FLAGS = 0x32fe7d6f;

function typeFormatFlagsToNative(flags) {
    return (flags & TS6_TYPE_FORMAT_FLAGS) >>> 0;
}

// TS7 bits with no TS 6.0 bit: Reparsed (3), HasJSDoc (21), PossiblyContainsDeprecatedTag (26) and
// ReparserTransformedLiteral (28) have no TS 6.0 bit; TS 6.0 Namespace, GlobalAugmentation,
// HasAggregatedChildData, TypeCached and Deprecated are computed or cached in TS 6.0.
const NODE_FLAGS = bitTable(
    [0, 0], // Let
    [1, 1], // Const
    [2, 2], // Using
    [4, 4], // Synthesized
    [5, 6], // OptionalChain
    [6, 7], // ExportContext
    [7, 8], // ContainsThis, IdentifierHasExtendedUnicodeEscape
    [8, 9], // HasImplicitReturn
    [9, 10], // HasExplicitReturn
    [10, 13], // DisallowInContext
    [11, 14], // YieldContext
    [12, 15], // DecoratorContext
    [13, 16], // AwaitContext
    [14, 17], // DisallowConditionalTypesContext
    [15, 18], // ThisNodeHasError
    [16, 19], // JavaScriptFile
    [17, 20], // ThisNodeOrAnySubNodesHasError
    [18, 12], // HasAsyncFunctions, IdentifierIsInJSDocNamespace
    [19, 22], // PossiblyContainsDynamicImport
    [20, 23], // PossiblyContainsImportMeta
    [22, 24], // JSDoc
    [23, 25], // Ambient
    [24, 26], // InWithStatement
    [25, 27], // JsonFile
    [27, 30], // Unreachable
);
const NODE_FLAGS_TO_NATIVE = invertTable(NODE_FLAGS);
const NATIVE_NESTED_NAMESPACE = 1 << 5;
const TS6_NESTED_NAMESPACE = 1 << 3;
const TS6_DEPRECATED = 1 << 29;
const NATIVE_POSSIBLY_CONTAINS_DEPRECATED_TAG = 1 << 26;

// On a ModuleDeclaration the OptionalChain bit means NestedNamespace.
function nodeFlagsToTs6(flags, nativeKind) {
    if (nativeKind === nativeSyntaxKind.ModuleDeclaration && (flags & NATIVE_NESTED_NAMESPACE) !== 0) {
        return remapBits(flags & ~NATIVE_NESTED_NAMESPACE, NODE_FLAGS) | TS6_NESTED_NAMESPACE;
    }
    return remapBits(flags, NODE_FLAGS);
}

function nodeFlagsToNative(flags, ts6Kind) {
    let result = remapBits(flags, NODE_FLAGS_TO_NATIVE);
    if ((flags & TS6_NESTED_NAMESPACE) !== 0 && syntaxKindToNative(ts6Kind) === nativeSyntaxKind.ModuleDeclaration) {
        result |= NATIVE_NESTED_NAMESPACE;
    }
    if ((flags & TS6_DEPRECATED) !== 0) {
        result |= NATIVE_POSSIBLY_CONTAINS_DEPRECATED_TAG;
    }
    return result >>> 0;
}

// Option enums. 0 means "not set" in tsc-rs, so TS 6.0 ES3 and
// ModuleKind.None have no native value.
function scriptTargetToNative(target) {
    return target === 0 ? undefined : target;
}

function scriptTargetToTs6(target) {
    return target === 0 ? undefined : target;
}

function moduleKindToNative(kind) {
    return kind === 0 ? undefined : kind;
}

function moduleKindToTs6(kind) {
    return kind === 0 ? undefined : kind;
}

// TS 6.0 CarriageReturnLineFeed 0, LineFeed 1; tsc-rs None 0, CRLF 1, LF 2.
function newLineKindToNative(kind) {
    return kind === 0 || kind === 1 ? kind + 1 : undefined;
}

function newLineKindToTs6(kind) {
    return kind === 1 || kind === 2 ? kind - 1 : undefined;
}

// TS 6.0 Legacy 1, Auto 2, Force 3; tsc-rs Auto 1, Legacy 2, Force 3.
const MODULE_DETECTION_SWAP = [undefined, 2, 1, 3];

function moduleDetectionKindToNative(kind) {
    return MODULE_DETECTION_SWAP[kind];
}

function moduleDetectionKindToTs6(kind) {
    return MODULE_DETECTION_SWAP[kind];
}

// tsc-rs has no ScriptKind External (5) or Deferred (7); TS 6.0 parses both as TypeScript.
function scriptKindToNative(kind) {
    return kind === 5 || kind === 7 ? 3 : kind;
}

module.exports = {
    NATIVE_SYNTAX_KIND_NAMES,
    TS6_SYNTAX_KIND_NAMES,
    nativeSyntaxKind,
    nativeSyntaxKindName,
    syntaxKindToTs6,
    syntaxKindToNative,
    objectFlagsToTs6,
    checkFlagsToTs6,
    signatureFlagsToTs6,
    symbolFlagsToTs6,
    symbolFlagsToNative,
    typeFlagsToTs6,
    elementFlagsToTs6,
    nodeBuilderFlagsToNative,
    typeFormatFlagsToNative,
    nodeFlagsToTs6,
    nodeFlagsToNative,
    scriptTargetToNative,
    scriptTargetToTs6,
    moduleKindToNative,
    moduleKindToTs6,
    newLineKindToNative,
    newLineKindToTs6,
    moduleDetectionKindToNative,
    moduleDetectionKindToTs6,
    scriptKindToNative,
};
