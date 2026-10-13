"use strict";

// The TS 6.0 object model of a checker: Type, Symbol, Signature, IndexInfo and TypePredicate
// objects built with TS 6.0's own constructors (ts.objectAllocator), so that their prototype
// methods (type.getProperties(), symbol.getDocumentationComment(checker), signature.getReturnType()
// ...) run through `this.checker`. Fields that need a tsc-rs request are lazy accessors that cache
// the answer as an own property. One object per native type id, symbol reference and signature id.

const { checkFlagsToTs6, elementFlagsToTs6, objectFlagsToTs6, signatureFlagsToTs6, symbolFlagsToTs6, syntaxKindToTs6 } = require("./flags.js");
const { escapedNameToTs6 } = require("./names.js");

const symbolOwnerFile = 0;

// The base constructors assign every field; the lazy setters ignore those first assignments.
let constructing = false;

const classesByTs = new WeakMap();
const modelsByChecker = new WeakMap();

function classesFor(ts) {
    let classes = classesByTs.get(ts);
    if (classes === undefined) {
        classes = createClasses(ts);
        classesByTs.set(ts, classes);
    }
    return classes;
}

function own(object, name, value) {
    Object.defineProperty(object, name, { value, writable: true, enumerable: true, configurable: true });
}

// A list answered as a circularity near the end of the JS stack (checker.js hasStackRoom); a field
// does not keep it.
const circularList = Object.freeze([]);

function lazy(prototype, name, load) {
    Object.defineProperty(prototype, name, {
        configurable: true,
        get() {
            const value = load(this);
            if (value !== circularList) {
                own(this, name, value);
            }
            return value;
        },
        set(value) {
            if (!constructing) {
                own(this, name, value);
            }
        },
    });
}

function createClasses(ts) {
    const TypeObject = ts.objectAllocator.getTypeConstructor();
    const SymbolObject = ts.objectAllocator.getSymbolConstructor();
    const SignatureObject = ts.objectAllocator.getSignatureConstructor();

    class NativeType extends TypeObject {
        #model;
        #data;

        constructor(model, data) {
            super(model.checker, data.flags);
            this.#model = model;
            this.#data = data;
        }

        static native(type) {
            return typeof type === "object" && type !== null && #data in type ? { model: type.#model, data: type.#data } : undefined;
        }
    }

    class NativeSymbol extends SymbolObject {
        #model;
        #data;

        constructor(model, data) {
            constructing = true;
            try {
                super(symbolFlagsToTs6(data.flags), escapedNameToTs6(data.name));
            }
            finally {
                constructing = false;
            }
            this.#model = model;
            this.#data = data;
        }

        static native(symbol) {
            return typeof symbol === "object" && symbol !== null && #data in symbol ? { model: symbol.#model, data: symbol.#data } : undefined;
        }
    }

    class NativeSignature extends SignatureObject {
        #model;
        #data;

        constructor(model, data) {
            constructing = true;
            try {
                super(model.checker, signatureFlagsToTs6(data.flags));
            }
            finally {
                constructing = false;
            }
            this.#model = model;
            this.#data = data;
        }

        static native(signature) {
            return typeof signature === "object" && signature !== null && #data in signature ? { model: signature.#model, data: signature.#data } : undefined;
        }
    }

    for (const [Class, name] of [[NativeType, "TypeObject"], [NativeSymbol, "SymbolObject"], [NativeSignature, "SignatureObject"]]) {
        Object.defineProperty(Class, "name", { value: name });
    }
    defineSymbolFields(ts, NativeSymbol);
    defineSignatureFields(ts, NativeSignature);
    return { NativeType, NativeSymbol, NativeSignature, typeShapes: new Map(), nodeSymbols: installNodeSymbols(ts) };
}

function programOfSourceFile(sourceFile) {
    return require("./program.js").programOfSourceFile(sourceFile);
}

// Node symbols

// TS 6.0: src/compiler/binder.ts bindWorker gives these kinds a symbol (and bindSourceFileIfExternalModule
// the SourceFile of a module); JS expando assignments are answered when a checker hands them out.
const boundKindNames = [
    "TypeParameter", "Parameter", "VariableDeclaration", "BindingElement", "PropertyDeclaration", "PropertySignature", "PropertyAssignment",
    "ShorthandPropertyAssignment", "EnumMember", "CallSignature", "ConstructSignature", "IndexSignature", "MethodDeclaration", "MethodSignature",
    "FunctionDeclaration", "Constructor", "GetAccessor", "SetAccessor", "FunctionType", "JSDocFunctionType", "JSDocSignature", "ConstructorType",
    "TypeLiteral", "JSDocTypeLiteral", "MappedType", "ObjectLiteralExpression", "FunctionExpression", "ArrowFunction", "ClassExpression",
    "ClassDeclaration", "InterfaceDeclaration", "TypeAliasDeclaration", "EnumDeclaration", "ModuleDeclaration", "JsxAttributes", "JsxAttribute",
    "ImportEqualsDeclaration", "NamespaceImport", "ImportSpecifier", "ExportSpecifier", "NamespaceExportDeclaration", "ImportClause",
    "ExportDeclaration", "ExportAssignment", "NamespaceExport", "JSDocParameterTag", "JSDocPropertyTag", "JSDocTypedefTag", "JSDocCallbackTag",
    "JSDocEnumTag", "SourceFile",
];

const nodeSymbolsByTs = new WeakMap();

// Declarations a checker handed out (ObjectModel withSymbolField): answered whatever their kind.
const handedOutDeclarations = new WeakSet();

// TS 6.0's binder sets `symbol` on every declaration of a program file, and services and tools read
// it (computeSuggestionDiagnostics reads `node.symbol.members`, findBaseOfDeclaration). TS 6.0's
// factory starts each declaration with `symbol = undefined` (createBaseDeclaration); with this
// accessor on the node prototype, that first assignment makes `symbol` an own field of a bound kind
// (so own enumerable properties stay those of TS 6.0's nodes) that asks the newest live program of
// the node's file when it is read, and so keeps no program alive. Other kinds get no own field; a
// SourceFile answers through the prototype. Nodes parsed before this runs (src/index.js installs it
// at load) get the field when a checker hands them out.
function installNodeSymbols(ts) {
    let nodeSymbols = nodeSymbolsByTs.get(ts);
    if (nodeSymbols !== undefined) {
        return nodeSymbols;
    }
    const kinds = new Set(boundKindNames.map(name => ts.SyntaxKind[name]));
    const get = function getSymbol() {
        return symbolOfProgramNode(ts, kinds, this);
    };
    const set = function setSymbol(value) {
        own(this, "symbol", value);
    };
    const field = { configurable: true, enumerable: true, get, set };
    nodeSymbols = { field };
    nodeSymbolsByTs.set(ts, nodeSymbols);
    Object.defineProperty(ts.objectAllocator.getNodeConstructor().prototype, "symbol", {
        configurable: true,
        get,
        set(value) {
            if (value !== undefined) {
                own(this, "symbol", value);
            }
            else if (kinds.has(this.kind)) {
                Object.defineProperty(this, "symbol", field);
            }
        },
    });
    return nodeSymbols;
}

function symbolOfProgramNode(ts, kinds, node) {
    if (!kinds.has(node.kind) && !handedOutDeclarations.has(node)) {
        return undefined;
    }
    const sourceFile = ts.getSourceFileOfNode(node);
    const program = sourceFile === undefined ? undefined : programOfSourceFile(sourceFile);
    if (program === undefined) {
        return undefined;
    }
    return modelsByChecker.get(program.getTypeChecker())?.symbolOfNode(node);
}

// Symbol

const privateName = /^__#\d+@(#.*)$/s;

function defineSymbolFields(ts, NativeSymbol) {
    const { prototype } = NativeSymbol;
    const native = symbol => NativeSymbol.native(symbol);
    // TS 6.0: src/compiler/utilities.ts symbolName reads the value declaration only for private
    // names, which the escaped name spells too; reading the name must not resolve declarations.
    Object.defineProperty(prototype, "name", {
        configurable: true,
        get() {
            const match = privateName.exec(this.escapedName);
            return match === null ? ts.unescapeLeadingUnderscores(this.escapedName) : match[1];
        },
    });
    lazy(prototype, "declarations", symbol => {
        const { model, data } = native(symbol);
        return data.declarations?.map(handle => model.declaration(handle, symbol)).filter(node => node !== undefined);
    });
    lazy(prototype, "valueDeclaration", symbol => {
        const { model, data } = native(symbol);
        return data.valueDeclaration === undefined ? undefined : model.declaration(data.valueDeclaration, symbol);
    });
    lazy(prototype, "parent", symbol => {
        const { model, data } = native(symbol);
        return model.symbolProperty(data, data.parent, "getParentOfSymbol");
    });
    lazy(prototype, "exportSymbol", symbol => {
        const { model, data } = native(symbol);
        return model.symbolProperty(data, data.exportSymbol, "getExportSymbolOfSymbol");
    });
    // TS 6.0: src/compiler/binder.ts:641 addDeclarationToSymbol creates these tables.
    lazy(prototype, "members", symbol => {
        const flags = ts.SymbolFlags.Class | ts.SymbolFlags.Interface | ts.SymbolFlags.TypeLiteral | ts.SymbolFlags.ObjectLiteral;
        return native(symbol).model.symbolTable(symbol, "getMembersOfSymbol", flags);
    });
    lazy(prototype, "exports", symbol => {
        const flags = ts.SymbolFlags.Class | ts.SymbolFlags.Enum | ts.SymbolFlags.Module | ts.SymbolFlags.Variable;
        return native(symbol).model.symbolTable(symbol, "getExportsOfSymbol", flags);
    });
    // TS 6.0 keeps `links` on transient symbols only (src/compiler/checker.ts createSymbol).
    lazy(prototype, "links", symbol => {
        if ((symbol.flags & ts.SymbolFlags.Transient) === 0) {
            return undefined;
        }
        const { model, data } = native(symbol);
        const links = { checkFlags: checkFlagsToTs6(data.checkFlags) };
        lazy(links, "type", () => model.checker.getTypeOfSymbol(symbol));
        return links;
    });
}

// Signature

function defineSignatureFields(ts, NativeSignature) {
    const { prototype } = NativeSignature;
    const native = signature => NativeSignature.native(signature);
    lazy(prototype, "declaration", signature => {
        const { model, data } = native(signature);
        return data.declaration === undefined ? undefined : model.signatureDeclaration(data.declaration);
    });
    lazy(prototype, "typeParameters", signature => {
        const { model, data } = native(signature);
        return data.typeParameters === undefined ? undefined : model.typeList(data.typeParameters, "getTypeParametersOfSignature", data.id);
    });
    lazy(prototype, "parameters", signature => {
        const { model, data } = native(signature);
        const parameters = model.symbolList(data.parameters ?? [], "getParametersOfSignature", data.id, "list");
        model.noteParameters(signature, parameters);
        return parameters;
    });
    lazy(prototype, "thisParameter", signature => {
        const { model, data } = native(signature);
        if (data.thisParameter === undefined) {
            return undefined;
        }
        const thisParameter = model.cachedSymbol(data.thisParameter) ?? model.symbol(model.request("getThisParameterOfSignature", { objectId: data.id }));
        model.noteParameters(signature, [thisParameter]);
        return thisParameter;
    });
    lazy(prototype, "target", signature => {
        const { model, data } = native(signature);
        if (!data.target) {
            return undefined;
        }
        return model.signatures.get(data.target) ?? model.signature(model.request("getTargetOfSignature", { objectId: data.id }));
    });
    lazy(prototype, "resolvedReturnType", signature => native(signature).model.checker.getReturnTypeOfSignature(signature));
    lazy(prototype, "resolvedTypePredicate", signature => native(signature).model.checker.getTypePredicateOfSignature(signature));
    lazy(prototype, "minArgumentCount", signature => minArgumentCount(ts, signature));
    lazy(prototype, "minTypeArgumentCount", signature => {
        const { model } = native(signature);
        const typeParameters = signature.typeParameters ?? [];
        let count = 0;
        for (let index = 0; index < typeParameters.length; index++) {
            if (model.checker.getDefaultFromTypeParameter(typeParameters[index]) === undefined) {
                count = index + 1;
            }
        }
        return count;
    });
    lazy(prototype, "mapper", () => undefined);
}

// TS 6.0: src/compiler/checker.ts getSignatureFromDeclaration computes minArgumentCount from the
// parameters; signatures without a declaration count the parameters that are not optional.
function minArgumentCount(ts, signature) {
    const declaration = signature.declaration;
    if (declaration !== undefined && ts.isFunctionLike(declaration)) {
        let count = 0;
        let index = 0;
        for (const parameter of declaration.parameters) {
            if (index === 0 && ts.isIdentifier(parameter.name) && parameter.name.escapedText === "this") {
                continue;
            }
            index++;
            const optional = parameter.questionToken !== undefined || parameter.initializer !== undefined || parameter.dotDotDotToken !== undefined || ts.isJSDocOptionalParameter(parameter);
            if (!optional) {
                count = index;
            }
        }
        return count;
    }
    let count = 0;
    signature.parameters.forEach((parameter, index) => {
        const checkFlags = parameter.links?.checkFlags ?? 0;
        const optional = (checkFlags & (ts.CheckFlags.OptionalParameter | ts.CheckFlags.RestParameter)) !== 0 || (parameter.flags & ts.SymbolFlags.Optional) !== 0;
        if (!optional) {
            count = index + 1;
        }
    });
    return count;
}

// Type

// Each shape gets its own class so that `"types" in type`, `"typeParameter" in type` and the like
// answer as for TS 6.0's objects.
function typeShape(ts, data) {
    const { TypeFlags, ObjectFlags } = ts;
    const groups = [];
    const flags = data.flags;
    if (flags & TypeFlags.Object) {
        const objectFlags = objectFlagsToTs6(data.objectFlags ?? 0, flags);
        if (objectFlags & ObjectFlags.Reference) {
            groups.push("reference");
        }
        if (objectFlags & ObjectFlags.ClassOrInterface) {
            groups.push("interface");
        }
        if (data.elementFlags !== undefined) {
            groups.push("tuple");
        }
        if (objectFlags & ObjectFlags.Mapped) {
            groups.push("mapped");
        }
    }
    if (flags & (TypeFlags.UnionOrIntersection | TypeFlags.TemplateLiteral)) {
        groups.push("types");
    }
    if (flags & TypeFlags.Freshable) {
        groups.push("freshable");
    }
    if (flags & TypeFlags.UniqueESSymbol) {
        groups.push("uniqueSymbol");
    }
    if (flags & (TypeFlags.Index | TypeFlags.StringMapping)) {
        groups.push("target");
    }
    if (flags & TypeFlags.IndexedAccess) {
        groups.push("indexedAccess");
    }
    if (flags & TypeFlags.Conditional) {
        groups.push("conditional");
    }
    if (flags & TypeFlags.Substitution) {
        groups.push("substitution");
    }
    return groups.join(",");
}

function typeClass(ts, classes, shape) {
    let Class = classes.typeShapes.get(shape);
    if (Class !== undefined) {
        return Class;
    }
    const { NativeType } = classes;
    Class = class extends NativeType {};
    Object.defineProperty(Class, "name", { value: "TypeObject" });
    const { prototype } = Class;
    const native = type => NativeType.native(type);
    lazy(prototype, "symbol", type => {
        const { model, data } = native(type);
        return model.symbolProperty(data, data.symbol, "getSymbolOfType", true);
    });
    lazy(prototype, "aliasSymbol", type => {
        const { model, data } = native(type);
        return model.symbolProperty(data, data.aliasSymbol, "getAliasSymbolOfType", true);
    });
    lazy(prototype, "aliasTypeArguments", type => {
        const { model, data } = native(type);
        return data.aliasTypeArguments === undefined ? undefined : model.typeList(data.aliasTypeArguments, "getAliasTypeArgumentsOfType", data.id, "list");
    });
    for (const group of shape.split(",")) {
        typeGroupFields[group]?.(ts, prototype, native);
    }
    classes.typeShapes.set(shape, Class);
    return Class;
}

function typeField(prototype, native, name, idField, method) {
    lazy(prototype, name, type => {
        const { model, data } = native(type);
        return model.typeProperty(type, data, data[idField], method);
    });
}

const typeGroupFields = {
    reference(ts, prototype, native) {
        typeField(prototype, native, "target", "target", "getTargetOfType");
        lazy(prototype, "resolvedTypeArguments", type => native(type).model.checker.getTypeArguments(type));
    },
    interface(ts, prototype, native) {
        for (const [name, method] of [["typeParameters", "getTypeParametersOfType"], ["outerTypeParameters", "getOuterTypeParametersOfType"], ["localTypeParameters", "getLocalTypeParametersOfType"]]) {
            lazy(prototype, name, type => {
                const { model, data } = native(type);
                return data[name] === undefined ? undefined : model.typeList(data[name], method, data.id);
            });
        }
        typeField(prototype, native, "thisType", "thisType", "getThisTypeOfType");
    },
    tuple(ts, prototype, native) {
        lazy(prototype, "labeledElementDeclarations", type => {
            const { model, data } = native(type);
            return data.labeledElementDeclarations?.map(handle => handle === "" ? undefined : model.node(handle));
        });
    },
    mapped(ts, prototype, native) {
        typeField(prototype, native, "typeParameter", "typeParameter", "getTypeParameterOfMappedType");
        typeField(prototype, native, "constraintType", "constraintType", "getConstraintTypeOfMappedType");
        typeField(prototype, native, "nameType", "nameType", "getNameTypeOfMappedType");
        typeField(prototype, native, "templateType", "templateType", "getTemplateTypeOfMappedType");
        lazy(prototype, "declaration", type => type.symbol?.declarations?.find(ts.isMappedTypeNode));
        lazy(prototype, "modifiersType", type => modifiersType(ts, native(type).model, type));
    },
    types(ts, prototype, native) {
        lazy(prototype, "types", type => {
            const { model, data } = native(type);
            return model.typeList(undefined, "getTypesOfType", data.id, "list");
        });
    },
    freshable(ts, prototype, native) {
        typeField(prototype, native, "freshType", "freshType", "getFreshTypeOfType");
        typeField(prototype, native, "regularType", "regularType", "getRegularTypeOfType");
    },
    uniqueSymbol(ts, prototype, native) {
        // TS 6.0: src/compiler/checker.ts getESSymbolLikeTypeForNode names it `__@<name>@<symbol id>`;
        // tsc-rs uses the same form with its own symbol id, which property names carry too.
        lazy(prototype, "escapedName", type => {
            const symbolData = type.symbol === undefined ? undefined : native(type).model.symbolData(type.symbol);
            if (symbolData === undefined) {
                return undefined;
            }
            const name = symbolData.name.startsWith("___") ? symbolData.name.slice(1) : symbolData.name;
            return escapedNameToTs6(`__@${name}@${symbolData.reference.id}`);
        });
    },
    target(ts, prototype, native) {
        typeField(prototype, native, "type", "target", "getTargetOfType");
    },
    indexedAccess(ts, prototype, native) {
        typeField(prototype, native, "objectType", "objectType", "getObjectTypeOfType");
        typeField(prototype, native, "indexType", "indexType", "getIndexTypeOfType");
    },
    conditional(ts, prototype, native) {
        typeField(prototype, native, "checkType", "checkType", "getCheckTypeOfType");
        typeField(prototype, native, "extendsType", "extendsType", "getExtendsTypeOfType");
        lazy(prototype, "resolvedTrueType", type => native(type).model.typeProperty(type, native(type).data, -1, "getTrueTypeOfConditionalType"));
        lazy(prototype, "resolvedFalseType", type => native(type).model.typeProperty(type, native(type).data, -1, "getFalseTypeOfConditionalType"));
        lazy(prototype, "root", type => conditionalRoot(ts, native(type).model, type));
    },
    substitution(ts, prototype, native) {
        typeField(prototype, native, "baseType", "baseType", "getBaseTypeOfType");
        typeField(prototype, native, "constraint", "substConstraint", "getConstraintOfType");
    },
};

// TS 6.0: src/compiler/checker.ts getModifiersTypeFromMappedType. The declared `keyof T` constraint
// is read from the mapped type node when the constraint no longer shows it; tsc-rs sends no mapper,
// so an instantiation maps T only through its alias's type arguments, else it has no answer.
function modifiersType(ts, model, type) {
    const { checker } = model;
    const constraint = type.constraintType;
    if (constraint !== undefined && constraint.flags & ts.TypeFlags.Index) {
        return constraint.type;
    }
    const declared = type.declaration?.typeParameter.constraint;
    if (declared !== undefined && ts.isTypeOperatorNode(declared) && declared.operator === ts.SyntaxKind.KeyOfKeyword) {
        return instantiateThroughAlias(ts, model, type, checker.getTypeFromTypeNode(declared.type));
    }
    const parameterConstraint = constraint !== undefined && constraint.flags & ts.TypeFlags.TypeParameter ? checker.getConstraintOfTypeParameter(constraint) : constraint;
    if (parameterConstraint !== undefined && parameterConstraint.flags & ts.TypeFlags.Index) {
        return parameterConstraint.type;
    }
    return checker.getUnknownType();
}

function instantiateThroughAlias(ts, model, type, declared) {
    if (!(type.objectFlags & ts.ObjectFlags.Instantiated)) {
        return declared;
    }
    if (declared.flags & ts.TypeFlags.TypeParameter && type.aliasSymbol !== undefined && type.aliasTypeArguments !== undefined) {
        const parameters = model.checker.getDeclaredTypeOfSymbol(type.aliasSymbol).aliasTypeArguments ?? [];
        const index = parameters.indexOf(declared);
        if (index >= 0) {
            return type.aliasTypeArguments[index];
        }
    }
    return undefined;
}

// TS 6.0 ConditionalRoot (src/compiler/checker.ts getTypeFromConditionalTypeNode). tsc-rs does
// not send the root; its node is the conditional type node of the alias or of the declaration that
// holds the check type, and the declared check and extends types come from that node.
function conditionalRoot(ts, model, type) {
    const { checker } = model;
    const root = {
        inferTypeParameters: undefined,
        outerTypeParameters: undefined,
        instantiations: undefined,
        aliasSymbol: type.aliasSymbol,
        aliasTypeArguments: type.aliasTypeArguments,
    };
    lazy(root, "node", () => findConditionalTypeNode(ts, model, type));
    lazy(root, "checkType", () => root.node === undefined ? type.checkType : checker.getTypeFromTypeNode(root.node.checkType));
    lazy(root, "extendsType", () => root.node === undefined ? type.extendsType : checker.getTypeFromTypeNode(root.node.extendsType));
    lazy(root, "isDistributive", () => (root.checkType.flags & ts.TypeFlags.TypeParameter) !== 0);
    return root;
}

function findConditionalTypeNode(ts, model, type) {
    const containers = [
        ...type.aliasSymbol?.declarations ?? [],
        ...type.checkType.symbol?.declarations?.map(declaration => declaration.parent) ?? [],
    ];
    const candidates = [];
    const visit = node => {
        if (ts.isConditionalTypeNode(node)) {
            candidates.push(node);
        }
        ts.forEachChild(node, visit);
    };
    for (const container of containers) {
        if (container !== undefined) {
            visit(container);
        }
    }
    return candidates.find(node => model.checker.getTypeFromTypeNode(node) === type)
        ?? candidates.find(node => model.checker.getTypeFromTypeNode(node.extendsType) === type.extendsType)
        ?? candidates[0];
}

// The model

class ObjectModel {
    constructor(checker, program) {
        this.checker = checker;
        this.program = program;
        this.ts = program.ts;
        this.classes = classesFor(program.ts);
        this.types = new Map();
        this.symbols = new Map();
        this.signatures = new Map();
        this.nodes = new Map();
        this.declarationSymbols = new WeakMap();
        this.sharedSymbols = new WeakMap();
        this.parameterSignatures = new WeakMap();
        modelsByChecker.set(checker, this);
    }

    request(method, params) {
        return this.program.request(method, params);
    }

    requestBinary(method, params) {
        return this.program.requestBinary(method, params);
    }

    node(handle) {
        if (handle === undefined || handle === "") {
            return undefined;
        }
        let node = this.nodes.get(handle);
        if (node === undefined) {
            node = this.program.handleToNode(handle);
            this.nodes.set(handle, node);
        }
        return node;
    }

    // A declaration of `symbol` (one of its declarations): a symbol that is not transient is the
    // declaration's own, so the node needs no request for it.
    declaration(handle, symbol) {
        const node = this.node(handle);
        if (node !== undefined && (symbol.flags & this.ts.SymbolFlags.Transient) === 0 && !this.declarationSymbols.has(node)) {
            this.declarationSymbols.set(node, symbol);
        }
        return this.withSymbolField(node);
    }

    signatureDeclaration(handle) {
        return this.withSymbolField(this.node(handle));
    }

    // A declaration this checker hands out answers `symbol` (installNodeSymbols) whatever its kind
    // and also when it was parsed before the node prototype had the field. A language service or a
    // ts.server project shares unchanged SourceFiles between its programs, so the field asks the
    // newest live program of the node's file: a field bound to this program would keep it, and its
    // tsc-rs snapshot, alive and answer from it after newer programs. The checkers of the other
    // programs that have the file take that symbol as their own (symbolOfSharedDeclaration). A file
    // no program shares (shadowSourceFile) gets this one's.
    withSymbolField(node) {
        if (node === undefined) {
            return undefined;
        }
        const { field } = this.classes.nodeSymbols;
        const descriptor = Object.getOwnPropertyDescriptor(node, "symbol");
        if (descriptor !== undefined && descriptor.get !== field.get && (descriptor.get !== undefined || descriptor.value !== undefined)) {
            return node;
        }
        if (programOfSourceFile(this.ts.getSourceFileOfNode(node)) !== undefined) {
            handedOutDeclarations.add(node);
            if (descriptor !== undefined && descriptor.get === undefined) {
                Object.defineProperty(node, "symbol", field);
            }
            return node;
        }
        const model = this;
        Object.defineProperty(node, "symbol", {
            configurable: true,
            enumerable: true,
            get() {
                return model.symbolOfNode(node);
            },
            set(value) {
                own(node, "symbol", value);
            },
        });
        return node;
    }

    // The symbol of a declaration node of this program, asked once per node.
    symbolOfNode(node) {
        if (this.declarationSymbols.has(node)) {
            return this.declarationSymbols.get(node);
        }
        const symbol = this.symbolOfDeclaration(node);
        this.declarationSymbols.set(node, symbol);
        return symbol;
    }

    // Set by the checker: the symbol of a declaration node.
    symbolOfDeclaration() {
        return undefined;
    }

    // Set by the checker: with `interrupted`, a resolution that answers as a circularity near the
    // end of the JS stack (checker.js hasStackRoom).
    interruptible(compute) {
        return compute();
    }

    // Types

    type(data) {
        if (data === undefined || data === null) {
            return undefined;
        }
        let type = this.types.get(data.id);
        if (type !== undefined) {
            return type;
        }
        const { ts, classes } = this;
        const Class = typeClass(ts, classes, typeShape(ts, data));
        type = new Class(this, data);
        type.id = data.id;
        setTypeFields(ts, type, data);
        this.types.set(data.id, type);
        return type;
    }

    // A type field: the type itself, a type this checker already has, or one more request.
    typeProperty(type, data, id, method) {
        if (id === 0 || id === undefined) {
            return undefined;
        }
        if (id === data.id) {
            return type;
        }
        return this.types.get(id) ?? this.type(this.request(method, { objectId: data.id }));
    }

    // A list of types: from the known ids when all are known, else from the server.
    typeList(ids, method, objectId, interrupted) {
        if (ids !== undefined) {
            const known = ids.map(id => this.types.get(id));
            if (known.every(type => type !== undefined)) {
                return known;
            }
        }
        return this.interruptible(() => (this.request(method, { objectId }) ?? []).map(data => this.type(data)), interrupted);
    }

    // Symbols

    symbol(data) {
        if (data === undefined || data === null) {
            return undefined;
        }
        const key = referenceKey(data.reference);
        let symbol = this.symbols.get(key);
        if (symbol !== undefined) {
            return symbol;
        }
        symbol = new this.classes.NativeSymbol(this, data);
        this.ts.getSymbolId(symbol);
        this.symbols.set(key, symbol);
        return symbol;
    }

    symbolData(symbol) {
        return this.classes.NativeSymbol.native(symbol)?.data;
    }

    cachedSymbol(compact) {
        return this.symbols.get(compactKey(compact));
    }

    // A symbol field of a symbol (`source` is its response) or of a type (`ofType`).
    symbolProperty(source, compact, method, ofType = false) {
        if (compact === undefined) {
            return undefined;
        }
        const cached = this.cachedSymbol(compact);
        if (cached !== undefined) {
            return cached;
        }
        const params = ofType ? { objectId: source.id } : { symbol: source.reference };
        return this.symbol(this.request(method, params));
    }

    symbolList(compacts, method, objectId, interrupted) {
        const known = compacts.map(compact => this.cachedSymbol(compact));
        if (known.every(symbol => symbol !== undefined)) {
            return known;
        }
        return this.interruptible(() => (this.request(method, { objectId }) ?? []).map(data => this.symbol(data)), interrupted);
    }

    symbolTable(symbol, method, binderFlags) {
        const data = this.symbolData(symbol);
        const members = (this.request(method, { symbol: data.reference }) ?? []).map(member => this.symbol(member)).filter(member => member !== undefined);
        if (members.length === 0 && (symbol.flags & binderFlags) === 0) {
            return undefined;
        }
        return new Map(members.map(member => [member.escapedName, member]));
    }

    // Signatures

    // The signature whose parameter list first had each parameter symbol: an instantiated
    // signature's parameters are its own transient symbols (TS 6.0 instantiateSignature).
    noteParameters(signature, parameters) {
        for (const parameter of parameters) {
            if (parameter !== undefined && !this.parameterSignatures.has(parameter)) {
                this.parameterSignatures.set(parameter, signature);
            }
        }
    }

    signatureOfParameter(parameter) {
        return this.parameterSignatures.get(parameter);
    }

    signature(data) {
        if (data === undefined || data === null) {
            return undefined;
        }
        let signature = this.signatures.get(data.id);
        if (signature === undefined) {
            signature = new this.classes.NativeSignature(this, data);
            this.signatures.set(data.id, signature);
        }
        return signature;
    }

    // TS 6.0: src/compiler/checker.ts createIndexInfo
    indexInfo(data) {
        return {
            keyType: this.type(data.keyType),
            type: this.type(data.valueType),
            isReadonly: data.isReadonly ?? false,
            declaration: data.declaration === undefined ? undefined : this.signatureDeclaration(data.declaration),
            components: undefined,
        };
    }

    // TS 6.0: src/compiler/checker.ts createTypePredicate
    typePredicate(data) {
        const { TypePredicateKind } = this.ts;
        const isThis = data.kind === TypePredicateKind.This || data.kind === TypePredicateKind.AssertsThis;
        return {
            kind: data.kind,
            parameterName: isThis ? undefined : data.parameterName,
            parameterIndex: isThis ? undefined : data.parameterIndex,
            type: this.type(data.type),
        };
    }

    // Native identities of objects this model made.

    typeId(type) {
        const native = this.classes.NativeType.native(type);
        if (native === undefined || native.model !== this) {
            throw new Error("tsc-rs: the type does not come from this type checker");
        }
        return native.data.id;
    }

    symbolReference(symbol) {
        const native = this.classes.NativeSymbol.native(symbol);
        if (native !== undefined && native.model === this) {
            return native.data.reference;
        }
        const own = native === undefined ? undefined : this.symbolOfSharedDeclaration(symbol);
        if (own === undefined) {
            throw new Error("tsc-rs: the symbol does not come from this type checker");
        }
        return this.symbolReference(own);
    }

    // A symbol of another checker whose declaration is in a SourceFile this program has too. The
    // `symbol` of a declaration in a file that programs share comes from the newest of them
    // (installNodeSymbols), while TS 6.0's binder symbol is one object that every program's checker
    // takes (getMergedSymbol(sourceFile.symbol) in forEachExternalModule): it stands for this
    // checker's symbol of the declaration.
    symbolOfSharedDeclaration(symbol) {
        if (this.sharedSymbols.has(symbol)) {
            return this.sharedSymbols.get(symbol);
        }
        let own;
        if ((symbol.flags & this.ts.SymbolFlags.Transient) === 0) {
            for (const declaration of symbol.declarations ?? []) {
                const candidate = this.program.nodeToHandle(declaration) === undefined ? undefined : this.symbolOfNode(declaration);
                if (candidate !== undefined && candidate.escapedName === symbol.escapedName) {
                    own = candidate;
                    break;
                }
            }
        }
        this.sharedSymbols.set(symbol, own);
        return own;
    }

    signatureId(signature) {
        const native = this.classes.NativeSignature.native(signature);
        if (native === undefined || native.model !== this) {
            throw new Error("tsc-rs: the signature does not come from this type checker");
        }
        return native.data.id;
    }

    typeData(type) {
        return this.classes.NativeType.native(type)?.data ?? {};
    }
}

function referenceKey(reference) {
    return reference.kind === symbolOwnerFile ? `f${reference.file.nodeId}:${reference.id}` : `s:${reference.id}`;
}

function compactKey(compact) {
    return compact.file ? `f${compact.file}:${compact.id}` : `s:${compact.id}`;
}

function setTypeFields(ts, type, data) {
    const { TypeFlags } = ts;
    const flags = data.flags;
    if (flags & (TypeFlags.Object | TypeFlags.UnionOrIntersection | TypeFlags.Any | TypeFlags.Nullable | TypeFlags.Never) || data.intrinsicName !== undefined) {
        type.objectFlags = objectFlagsToTs6(data.objectFlags ?? 0, flags);
    }
    if (data.intrinsicName !== undefined) {
        type.intrinsicName = data.intrinsicName;
    }
    // TS 6.0: src/compiler/checker.ts createUnionType names the union of the two boolean literals.
    if (flags & TypeFlags.Union && flags & TypeFlags.Boolean) {
        type.intrinsicName = "boolean";
    }
    if (flags & TypeFlags.BooleanLiteral) {
        // TS 6.0's true and false types are intrinsic types without a value.
        type.intrinsicName = data.value === true ? "true" : "false";
    }
    else if (flags & TypeFlags.Literal) {
        type.value = literalValue(ts, flags, data.value);
    }
    if (data.isThisType) {
        type.isThisType = true;
    }
    if (data.texts !== undefined) {
        type.texts = data.texts;
    }
    if (data.elementFlags !== undefined) {
        setTupleFields(type, data);
    }
}

// TS 6.0: src/compiler/checker.ts createTupleTargetType
function setTupleFields(type, data) {
    const elementFlags = data.elementFlags.map(elementFlagsToTs6);
    let combinedFlags = 0;
    let minLength = 0;
    for (const flags of elementFlags) {
        combinedFlags |= flags;
        if (flags & (1 | 8)) {
            minLength++;
        }
    }
    type.elementFlags = elementFlags;
    type.minLength = minLength;
    type.fixedLength = data.fixedLength ?? 0;
    type.hasRestElement = (combinedFlags & 12) !== 0;
    type.combinedFlags = combinedFlags;
    type.readonly = data.readonly ?? false;
}

function literalValue(ts, flags, value) {
    if (flags & ts.TypeFlags.BigIntLiteral) {
        const text = String(value);
        const negative = text.startsWith("-");
        return { negative, base10Value: negative ? text.slice(1) : text };
    }
    if (flags & ts.TypeFlags.NumberLiteral && typeof value === "string") {
        return value === "NaN" ? NaN : value === "-Infinity" ? -Infinity : Infinity;
    }
    return value;
}

function createObjectModel(checker, program) {
    return new ObjectModel(checker, program);
}

// Synthetic nodes: tsc-rs answers typeToTypeNode and signatureToSignatureDeclaration with a binary
// syntax tree (protocol 9, the layout of crates/ts_goport/src/api/encoder/encoder.rs). It is
// rebuilt here with TS 6.0's factory, as TS 6.0's node builder makes it: positions -1 and the emit
// flags of src/compiler/checker.ts createNodeBuilder.

const headerStringTableOffsets = 24;
const headerStringTable = 28;
const headerExtendedData = 32;
const headerNodes = 40;
const nodeSize = 28;
const dataTypeMask = 0xC0000000;
const dataTypeChildren = 0;
const dataIndexMask = 0x00FFFFFF;

// Child property order per kind (packages/typescript/src/api/node/protocol.generated.ts at the pin).
const childProperties = {
    QualifiedName: ["left", "right"],
    ComputedPropertyName: ["expression"],
    TypeParameter: ["modifiers", "name", "constraint", "expression", "default"],
    Parameter: ["modifiers", "dotDotDotToken", "name", "questionToken", "type", "initializer"],
    PropertySignature: ["modifiers", "name", "questionToken", "type", "initializer"],
    PropertyDeclaration: ["modifiers", "name", "questionToken", "type", "initializer"],
    MethodSignature: ["modifiers", "name", "questionToken", "typeParameters", "parameters", "type"],
    MethodDeclaration: ["modifiers", "asteriskToken", "name", "questionToken", "typeParameters", "parameters", "type", "body"],
    Constructor: ["modifiers", "typeParameters", "parameters", "type", "body"],
    GetAccessor: ["modifiers", "name", "typeParameters", "parameters", "type", "body"],
    SetAccessor: ["modifiers", "name", "typeParameters", "parameters", "type", "body"],
    CallSignature: ["typeParameters", "parameters", "type"],
    ConstructSignature: ["typeParameters", "parameters", "type"],
    IndexSignature: ["modifiers", "parameters", "type"],
    TypePredicate: ["assertsModifier", "parameterName", "type"],
    TypeReference: ["typeName", "typeArguments"],
    FunctionType: ["typeParameters", "parameters", "type"],
    ConstructorType: ["modifiers", "typeParameters", "parameters", "type"],
    TypeQuery: ["exprName", "typeArguments"],
    TypeLiteral: ["members"],
    ArrayType: ["elementType"],
    TupleType: ["elements"],
    OptionalType: ["type"],
    RestType: ["type"],
    UnionType: ["types"],
    IntersectionType: ["types"],
    ConditionalType: ["checkType", "extendsType", "trueType", "falseType"],
    InferType: ["typeParameter"],
    ParenthesizedType: ["type"],
    TypeOperator: ["type"],
    IndexedAccessType: ["objectType", "indexType"],
    MappedType: ["readonlyToken", "typeParameter", "nameType", "questionToken", "type", "members"],
    LiteralType: ["literal"],
    NamedTupleMember: ["dotDotDotToken", "name", "questionToken", "type"],
    TemplateLiteralType: ["head", "templateSpans"],
    TemplateLiteralTypeSpan: ["type", "literal"],
    ImportType: ["argument", "attributes", "qualifier", "typeArguments"],
    ImportAttributes: ["elements"],
    ImportAttribute: ["name", "value"],
    ObjectBindingPattern: ["elements"],
    ArrayBindingPattern: ["elements"],
    BindingElement: ["dotDotDotToken", "propertyName", "name", "initializer"],
    PropertyAccessExpression: ["expression", "questionDotToken", "name"],
    ElementAccessExpression: ["expression", "questionDotToken", "argumentExpression"],
    PrefixUnaryExpression: ["operand"],
    ArrowFunction: ["modifiers", "typeParameters", "parameters", "type", "equalsGreaterThanToken", "body"],
    FunctionExpression: ["modifiers", "asteriskToken", "name", "typeParameters", "parameters", "type", "body"],
    FunctionDeclaration: ["modifiers", "asteriskToken", "name", "typeParameters", "parameters", "type", "body"],
    ExpressionWithTypeArguments: ["expression", "typeArguments"],
    Block: ["statements"],
};

const prefixOperators = ["PlusToken", "MinusToken", "TildeToken", "ExclamationToken", "PlusPlusToken", "MinusMinusToken"];
const typeOperators = ["KeyOfKeyword", "ReadonlyKeyword", "UniqueKeyword"];

class SyntheticTree {
    constructor(ts, bytes, multilineObjectLiterals, singleQuote) {
        this.ts = ts;
        this.view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
        this.bytes = bytes;
        this.stringOffsets = this.view.getUint32(headerStringTableOffsets, true);
        this.strings = this.view.getUint32(headerStringTable, true);
        this.extendedData = this.view.getUint32(headerExtendedData, true);
        this.nodes = this.view.getUint32(headerNodes, true);
        this.nodeCount = (bytes.byteLength - this.nodes) / nodeSize;
        this.multilineObjectLiterals = multilineObjectLiterals;
        this.singleQuote = singleQuote;
        this.decoder = new TextDecoder("utf-8");
    }

    field(index, offset) {
        return this.view.getUint32(this.nodes + index * nodeSize + offset, true);
    }

    string(index) {
        const start = this.view.getUint32(this.stringOffsets + index * 4, true);
        const end = this.view.getUint32(this.stringOffsets + (index + 1) * 4, true);
        return this.decoder.decode(this.bytes.subarray(this.strings + start, this.strings + end));
    }

    extended(index, slot) {
        return this.view.getUint32(this.extendedData + (this.field(index, 20) & dataIndexMask) + slot * 4, true);
    }

    children(index) {
        const children = [];
        let child = index + 1 < this.nodeCount && this.field(index + 1, 16) === index ? index + 1 : 0;
        while (child !== 0) {
            children.push(child);
            child = this.field(child, 12);
        }
        return children;
    }

    // The named children of a node, from the child mask of its data.
    named(index, kindName) {
        const names = childProperties[kindName];
        if (names === undefined) {
            throw new Error(`tsc-rs: cannot rebuild a synthesized ${kindName} node`);
        }
        const data = this.field(index, 20);
        const mask = (data & dataTypeMask) === dataTypeChildren ? data & 0xFF : 0;
        const children = this.children(index);
        const result = {};
        let next = 0;
        names.forEach((name, order) => {
            if (mask & (1 << order)) {
                result[name] = children[next++];
            }
        });
        return result;
    }

    kind6(index) {
        return syntaxKindToTs6(this.field(index, 0));
    }

    list(index) {
        if (index === undefined) {
            return undefined;
        }
        return this.ts.factory.createNodeArray(this.children(index).map(child => this.build(child)));
    }

    opt(index) {
        return index === undefined ? undefined : this.build(index);
    }

    text(index) {
        return this.string(this.field(index, 20) & dataIndexMask);
    }

    build(index) {
        const { ts } = this;
        const { factory, SyntaxKind } = ts;
        const kind = this.kind6(index);
        if (kind === undefined) {
            throw new Error(`tsc-rs: cannot rebuild a synthesized node of native kind ${this.field(index, 0)}`);
        }
        if (ts.isKeyword(kind) || ts.isPunctuation(kind) || kind === SyntaxKind.ThisType) {
            return this.token(kind);
        }
        const name = kindNames(ts)[kind];
        const c = name in childProperties ? this.named(index, name) : {};
        switch (kind) {
            case SyntaxKind.Identifier:
                return ts.setEmitFlags(factory.createIdentifier(this.text(index)), ts.EmitFlags.NoAsciiEscaping);
            case SyntaxKind.PrivateIdentifier:
                return factory.createPrivateIdentifier(this.text(index));
            case SyntaxKind.StringLiteral:
                return ts.setEmitFlags(factory.createStringLiteral(this.string(this.extended(index, 0)), this.singleQuote), ts.EmitFlags.NoAsciiEscaping);
            case SyntaxKind.NumericLiteral:
                return factory.createNumericLiteral(this.string(this.extended(index, 0)), this.extended(index, 1));
            case SyntaxKind.BigIntLiteral:
                return factory.createBigIntLiteral(this.string(this.extended(index, 0)));
            case SyntaxKind.NoSubstitutionTemplateLiteral:
                return this.template(factory.createNoSubstitutionTemplateLiteral, index);
            case SyntaxKind.TemplateHead:
                return this.template(factory.createTemplateHead, index);
            case SyntaxKind.TemplateMiddle:
                return this.template(factory.createTemplateMiddle, index);
            case SyntaxKind.TemplateTail:
                return this.template(factory.createTemplateTail, index);
            case SyntaxKind.QualifiedName:
                return factory.createQualifiedName(this.build(c.left), this.build(c.right));
            case SyntaxKind.ComputedPropertyName:
                return factory.createComputedPropertyName(this.build(c.expression));
            case SyntaxKind.PropertyAccessExpression:
                return factory.createPropertyAccessExpression(this.build(c.expression), this.build(c.name));
            case SyntaxKind.ElementAccessExpression:
                return factory.createElementAccessExpression(this.build(c.expression), this.build(c.argumentExpression));
            case SyntaxKind.PrefixUnaryExpression:
                return factory.createPrefixUnaryExpression(SyntaxKind[prefixOperators[(this.field(index, 20) >>> 24) & 0x7]], this.build(c.operand));
            case SyntaxKind.ExpressionWithTypeArguments:
                return factory.createExpressionWithTypeArguments(this.build(c.expression), this.list(c.typeArguments));
            case SyntaxKind.TypeParameter:
                return factory.createTypeParameterDeclaration(this.list(c.modifiers), this.build(c.name), this.opt(c.constraint), this.opt(c.default));
            case SyntaxKind.Parameter:
                return factory.createParameterDeclaration(this.list(c.modifiers), this.opt(c.dotDotDotToken), this.build(c.name), this.opt(c.questionToken), this.opt(c.type), this.opt(c.initializer));
            case SyntaxKind.ObjectBindingPattern:
                return ts.setEmitFlags(factory.createObjectBindingPattern(this.list(c.elements)), ts.EmitFlags.SingleLine | ts.EmitFlags.NoAsciiEscaping);
            case SyntaxKind.ArrayBindingPattern:
                return ts.setEmitFlags(factory.createArrayBindingPattern(this.list(c.elements)), ts.EmitFlags.SingleLine | ts.EmitFlags.NoAsciiEscaping);
            case SyntaxKind.BindingElement:
                return factory.createBindingElement(this.opt(c.dotDotDotToken), this.opt(c.propertyName), this.build(c.name), this.opt(c.initializer));
            case SyntaxKind.PropertySignature:
                return factory.createPropertySignature(this.list(c.modifiers), this.build(c.name), this.opt(c.questionToken), this.opt(c.type));
            case SyntaxKind.PropertyDeclaration:
                return factory.createPropertyDeclaration(this.list(c.modifiers), this.build(c.name), this.opt(c.questionToken), this.opt(c.type), this.opt(c.initializer));
            case SyntaxKind.MethodSignature:
                return factory.createMethodSignature(this.list(c.modifiers), this.build(c.name), this.opt(c.questionToken), this.list(c.typeParameters), this.list(c.parameters) ?? [], this.opt(c.type));
            case SyntaxKind.MethodDeclaration:
                return factory.createMethodDeclaration(this.list(c.modifiers), this.opt(c.asteriskToken), this.build(c.name), this.opt(c.questionToken), this.list(c.typeParameters), this.list(c.parameters) ?? [], this.opt(c.type), this.opt(c.body));
            case SyntaxKind.Constructor:
                return factory.createConstructorDeclaration(this.list(c.modifiers), this.list(c.parameters) ?? [], this.opt(c.body));
            case SyntaxKind.GetAccessor:
                return factory.createGetAccessorDeclaration(this.list(c.modifiers), this.build(c.name), this.list(c.parameters) ?? [], this.opt(c.type), this.opt(c.body));
            case SyntaxKind.SetAccessor:
                return factory.createSetAccessorDeclaration(this.list(c.modifiers), this.build(c.name), this.list(c.parameters) ?? [], this.opt(c.body));
            case SyntaxKind.CallSignature:
                return factory.createCallSignature(this.list(c.typeParameters), this.list(c.parameters) ?? [], this.opt(c.type));
            case SyntaxKind.ConstructSignature:
                return factory.createConstructSignature(this.list(c.typeParameters), this.list(c.parameters) ?? [], this.opt(c.type));
            case SyntaxKind.IndexSignature:
                return factory.createIndexSignature(this.list(c.modifiers), this.list(c.parameters) ?? [], this.build(c.type));
            case SyntaxKind.FunctionDeclaration:
                return factory.createFunctionDeclaration(this.list(c.modifiers), this.opt(c.asteriskToken), this.opt(c.name), this.list(c.typeParameters), this.list(c.parameters) ?? [], this.opt(c.type), this.opt(c.body));
            case SyntaxKind.FunctionExpression:
                return factory.createFunctionExpression(this.list(c.modifiers), this.opt(c.asteriskToken), this.opt(c.name), this.list(c.typeParameters), this.list(c.parameters) ?? [], this.opt(c.type), this.opt(c.body) ?? factory.createBlock([]));
            case SyntaxKind.ArrowFunction:
                return factory.createArrowFunction(this.list(c.modifiers), this.list(c.typeParameters), this.list(c.parameters) ?? [], this.opt(c.type), this.opt(c.equalsGreaterThanToken), this.opt(c.body) ?? factory.createBlock([]));
            case SyntaxKind.Block:
                return factory.createBlock(this.list(c.statements) ?? []);
            case SyntaxKind.TypePredicate:
                return factory.createTypePredicateNode(this.opt(c.assertsModifier), this.build(c.parameterName), this.opt(c.type));
            case SyntaxKind.TypeReference:
                return factory.createTypeReferenceNode(this.build(c.typeName), this.list(c.typeArguments));
            case SyntaxKind.FunctionType:
                return factory.createFunctionTypeNode(this.list(c.typeParameters), this.list(c.parameters) ?? [], this.build(c.type));
            case SyntaxKind.ConstructorType:
                return factory.createConstructorTypeNode(this.list(c.modifiers), this.list(c.typeParameters), this.list(c.parameters) ?? [], this.build(c.type));
            case SyntaxKind.TypeQuery:
                return factory.createTypeQueryNode(this.build(c.exprName), this.list(c.typeArguments));
            case SyntaxKind.TypeLiteral: {
                const literal = factory.createTypeLiteralNode(this.list(c.members));
                return this.multilineObjectLiterals ? literal : ts.setEmitFlags(literal, ts.EmitFlags.SingleLine);
            }
            case SyntaxKind.ArrayType:
                return factory.createArrayTypeNode(this.build(c.elementType));
            case SyntaxKind.TupleType:
                return ts.setEmitFlags(factory.createTupleTypeNode(this.list(c.elements) ?? []), ts.EmitFlags.SingleLine);
            case SyntaxKind.NamedTupleMember:
                return factory.createNamedTupleMember(this.opt(c.dotDotDotToken), this.build(c.name), this.opt(c.questionToken), this.build(c.type));
            case SyntaxKind.OptionalType:
                return factory.createOptionalTypeNode(this.build(c.type));
            case SyntaxKind.RestType:
                return factory.createRestTypeNode(this.build(c.type));
            case SyntaxKind.UnionType:
                return factory.createUnionTypeNode(this.list(c.types) ?? []);
            case SyntaxKind.IntersectionType:
                return factory.createIntersectionTypeNode(this.list(c.types) ?? []);
            case SyntaxKind.ConditionalType:
                return factory.createConditionalTypeNode(this.build(c.checkType), this.build(c.extendsType), this.build(c.trueType), this.build(c.falseType));
            case SyntaxKind.InferType:
                return factory.createInferTypeNode(this.build(c.typeParameter));
            case SyntaxKind.ParenthesizedType:
                return factory.createParenthesizedType(this.build(c.type));
            case SyntaxKind.TypeOperator:
                return factory.createTypeOperatorNode(SyntaxKind[typeOperators[(this.field(index, 20) >>> 24) & 0x3]], this.build(c.type));
            case SyntaxKind.IndexedAccessType:
                return factory.createIndexedAccessTypeNode(this.build(c.objectType), this.build(c.indexType));
            case SyntaxKind.MappedType:
                return ts.setEmitFlags(factory.createMappedTypeNode(this.opt(c.readonlyToken), this.build(c.typeParameter), this.opt(c.nameType), this.opt(c.questionToken), this.opt(c.type), this.list(c.members)), ts.EmitFlags.SingleLine);
            case SyntaxKind.LiteralType:
                return factory.createLiteralTypeNode(this.build(c.literal));
            case SyntaxKind.TemplateLiteralType:
                return factory.createTemplateLiteralType(this.build(c.head), this.list(c.templateSpans) ?? []);
            case SyntaxKind.TemplateLiteralTypeSpan:
                return factory.createTemplateLiteralTypeSpan(this.build(c.type), this.build(c.literal));
            case SyntaxKind.ImportType: {
                const isTypeOf = ((this.field(index, 20) >>> 24) & 1) !== 0;
                return factory.createImportTypeNode(this.build(c.argument), this.opt(c.attributes), this.opt(c.qualifier), this.list(c.typeArguments), isTypeOf);
            }
            case SyntaxKind.ImportAttributes: {
                const data = this.field(index, 20) >>> 24;
                return factory.createImportAttributes(this.list(c.elements) ?? [], (data & 1) !== 0, (data & 2) !== 0 ? SyntaxKind.AssertKeyword : SyntaxKind.WithKeyword);
            }
            case SyntaxKind.ImportAttribute:
                return factory.createImportAttribute(this.build(c.name), this.build(c.value));
            default:
                throw new Error(`tsc-rs: cannot rebuild a synthesized ${name} node`);
        }
    }

    // Synthesized template parts have no raw text; TS 6.0's factory then derives it.
    template(create, index) {
        const text = this.string(this.extended(index, 0));
        const rawText = this.string(this.extended(index, 1));
        return rawText === "" ? create(text) : create(text, rawText, this.extended(index, 2));
    }

    token(kind) {
        const { factory, SyntaxKind } = this.ts;
        switch (kind) {
            case SyntaxKind.ThisType:
                return factory.createThisTypeNode();
            case SyntaxKind.TrueKeyword:
                return factory.createTrue();
            case SyntaxKind.FalseKeyword:
                return factory.createFalse();
            case SyntaxKind.NullKeyword:
                return factory.createNull();
            case SyntaxKind.ThisKeyword:
                return factory.createThis();
            case SyntaxKind.SuperKeyword:
                return factory.createSuper();
            default:
                return this.ts.isModifierKind(kind) ? factory.createModifier(kind) : this.ts.isTypeNodeKind(kind) ? factory.createKeywordTypeNode(kind) : factory.createToken(kind);
        }
    }
}

const kindNamesByTs = new WeakMap();

// SyntaxKind names by value without TS 6.0's First*/Last* aliases.
function kindNames(ts) {
    let names = kindNamesByTs.get(ts);
    if (names === undefined) {
        names = [];
        for (const [name, value] of Object.entries(ts.SyntaxKind)) {
            if (typeof value === "number" && names[value] === undefined && !/^(First|Last)/.test(name)) {
                names[value] = name;
            }
        }
        kindNamesByTs.set(ts, names);
    }
    return names;
}

// The TS 6.0 node for a synthesized tsc-rs syntax tree (its root is node 1).
function decodeSyntheticNode(ts, bytes, { multilineObjectLiterals = false, singleQuote = false } = {}) {
    if (bytes === undefined) {
        return undefined;
    }
    return new SyntheticTree(ts, bytes, multilineObjectLiterals, singleQuote).build(1);
}

// Called by src/index.js when lib/typescript.js loads, before anything is parsed.
function install(ts) {
    installNodeSymbols(ts);
}

module.exports = { circularList, createObjectModel, decodeSyntheticNode, install };
