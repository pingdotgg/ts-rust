"use strict";

// Symbol names (`__String`). The tsc-rs API sends TS 6.0's escaped form already: Go
// ast.EscapeSymbolName (crates/ts_goport/src/ast/misc.rs escape_symbol_name) writes the binder's
// internal prefix (the byte 0xFE) as "__" and adds "_" to a user name that starts with "__",
// which is TS 6.0's escapeLeadingUnderscores. Late-bound names (`__@iterator@N`) and private names
// (`__#N@#x`) carry tsc-rs symbol ids, not TS 6.0's; tools read them as opaque. Names with lone
// surrogates do not survive the tsc-rs JSON encoding (each WTF-8 byte becomes U+FFFD).

function escapedNameToTs6(escapedName) {
    return escapedName;
}

// TS 6.0 escaped names of user symbols start with "___"; other names that start with "__" are
// internal (`__call`, `__index`, `__type`, `__export`, late-bound and private names).
function isInternalEscapedName(escapedName) {
    return escapedName.startsWith("__") && !escapedName.startsWith("___");
}

// The name tsc-rs requests that look a symbol up by name take (getPropertyOfType,
// getTypeOfPropertyOfType, getMemberInModuleExports): the binder name, which is the user name
// without the escape. Internal names have no form a request can carry, because the binder spells
// them with the byte 0xFE and JSON cannot hold it; they map to undefined.
function escapedNameToNative(escapedName) {
    if (escapedName.startsWith("___")) {
        return escapedName.slice(1);
    }
    return isInternalEscapedName(escapedName) ? undefined : escapedName;
}

const PRIVATE_NAME = /^__#\d+@(#.*)$/s;

// TS 6.0 Symbol.name: the private identifier's text for a private name, else the unescaped name.
// TS 6.0: src/compiler/utilities.ts symbolName
function symbolNameOfEscapedName(escapedName) {
    const privateName = PRIVATE_NAME.exec(escapedName);
    if (privateName !== null) {
        return privateName[1];
    }
    return escapedName.startsWith("___") ? escapedName.slice(1) : escapedName;
}

module.exports = {
    escapedNameToTs6,
    escapedNameToNative,
    isInternalEscapedName,
    symbolNameOfEscapedName,
};
