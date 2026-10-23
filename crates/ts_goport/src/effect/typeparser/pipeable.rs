//! Port of Effect-TS/tsgo `internal/typeparser/pipeable.go`.

use crate::effect::typeparser::*;
use crate::prelude::*;

impl TypeParser<'_> {
    // Go: typeparser/pipeable.go TypeParser.IsPipeableType
    /// IsPipeableType returns true if the type has a callable "pipe" property,
    /// indicating it supports the pipeable pattern (e.g., value.pipe(f1, f2, ...)).
    pub fn is_pipeable_type(&mut self, t: TypeId) -> bool {
        if t.is_nil() {
            return false;
        }
        cached!(self, is_pipeable_type, t, {
            let pipe_type = self.get_type_of_property_by_name(t, "pipe");
            if pipe_type.is_nil() {
                false
            } else {
                let signatures = self
                    .checker
                    .get_signatures_of_type_exported(pipe_type, SignatureKind::CALL);
                !signatures.is_empty()
            }
        })
    }

    // Go: typeparser/pipeable.go TypeParser.IsSafelyPipeableCallee
    /// IsSafelyPipeableCallee returns true if a callee expression can be safely
    /// extracted into a pipe argument without losing `this` context.
    /// This is used by the missedPipeableOpportunity rule to determine which
    /// call expressions can be converted to pipe style.
    pub fn is_safely_pipeable_callee(&mut self, callee: Node) -> bool {
        if callee.is_nil() {
            return false;
        }

        // Call expressions are safe - they return a value
        if is_call_expression(callee) {
            return true;
        }

        // Arrow functions are safe - no `this` binding
        if callee.kind() == SyntaxKind::ArrowFunction {
            return true;
        }

        // Function expressions are safe
        if callee.kind() == SyntaxKind::FunctionExpression {
            return true;
        }

        // Parenthesized expressions - check inner
        if callee.kind() == SyntaxKind::ParenthesizedExpression {
            return self.is_safely_pipeable_callee(callee.expression());
        }

        // Simple identifiers - check if it's a module/namespace or standalone function
        if is_identifier(callee) {
            let sym = self.get_symbol_at_location(callee);
            if sym.is_nil() {
                return false;
            }

            // Module/namespace imports are safe
            if self.checker.sym_flags(sym).intersects(
                SymbolFlags::MODULE | SymbolFlags::NAMESPACE | SymbolFlags::VALUE_MODULE,
            ) {
                return true;
            }

            // Check if the symbol's declaration is a function, variable, or import (not a method)
            if !self.checker.sym(sym).declarations.is_empty() {
                let decl = self.checker.sym(sym).declarations[0];
                match decl.kind() {
                    SyntaxKind::FunctionDeclaration
                    | SyntaxKind::VariableDeclaration
                    | SyntaxKind::ImportSpecifier
                    | SyntaxKind::ImportClause
                    | SyntaxKind::NamespaceImport => return true,
                    _ => {}
                }
            }

            return false;
        }

        // Property access - check if subject is a module/namespace
        if is_property_access_expression(callee) {
            let subject = callee.expression();
            let sym = self.get_symbol_at_location(subject);
            if sym.is_nil() {
                return false;
            }

            // Check if subject is a module/namespace
            if self.checker.sym_flags(sym).intersects(
                SymbolFlags::MODULE | SymbolFlags::NAMESPACE | SymbolFlags::VALUE_MODULE,
            ) {
                return true;
            }

            // Check if the symbol's declaration indicates it's a module import
            if !self.checker.sym(sym).declarations.is_empty() {
                let decl = self.checker.sym(sym).declarations[0];
                match decl.kind() {
                    SyntaxKind::NamespaceImport
                    | SyntaxKind::SourceFile
                    | SyntaxKind::ModuleDeclaration => return true,
                    _ => {}
                }
            }

            return false;
        }

        false
    }
}
