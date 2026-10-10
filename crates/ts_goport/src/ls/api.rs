//! Port of Go `ls/api.go`.

use crate::ls::prelude::*;

use std::sync::LazyLock;

// Go: ls/api.go:15 ErrNoSourceFile
pub static ERR_NO_SOURCE_FILE: LazyLock<GoError> =
    LazyLock::new(|| gostd::errors::new("source file not found"));

// Go: ls/api.go:16 ErrNoTokenAtPosition
pub static ERR_NO_TOKEN_AT_POSITION: LazyLock<GoError> =
    LazyLock::new(|| gostd::errors::new("no token found at position"));

impl LanguageService {
    // Go: ls/api.go:19 GetSymbolAtPosition
    pub fn get_symbol_at_position(
        &self,
        ctx: &Context,
        file_name: &str,
        position: i32,
    ) -> Result<SymbolId, GoError> {
        let (program, file) = self.try_get_program_and_file(file_name);
        if file.is_nil() {
            // Go: fmt.Errorf("%w: %s", ErrNoSourceFile, fileName)
            return Err(gostd::errors::errorf(
                format!("{}: {}", ERR_NO_SOURCE_FILE.error(), file_name),
                vec![(*ERR_NO_SOURCE_FILE).clone()],
            ));
        }
        let node = astnav::get_token_at_position(file, position);
        if node.is_nil() {
            // Go: fmt.Errorf("%w: %s:%d", ErrNoTokenAtPosition, fileName, position)
            return Err(gostd::errors::errorf(
                format!(
                    "{}: {}:{}",
                    ERR_NO_TOKEN_AT_POSITION.error(),
                    file_name,
                    position
                ),
                vec![(*ERR_NO_TOKEN_AT_POSITION).clone()],
            ));
        }
        let (checker, _done) = ls_program::get_type_checker_for_file(program, ctx, file);
        let c = &mut *checker.borrow_mut();
        let result = c.get_symbol_at_location_exported(node);
        Ok(result)
    }

    // Go: ls/api.go:33 GetSymbolAtLocation
    pub fn get_symbol_at_location(&self, ctx: &Context, node: Node) -> SymbolId {
        let program = self.get_program();
        let (checker, _done) =
            ls_program::get_type_checker_for_file(program, ctx, get_source_file_of_node(node));
        let c = &mut *checker.borrow_mut();
        let result = c.get_symbol_at_location_exported(node);
        result
    }

    // Go: ls/api.go:40 GetTypeOfSymbol
    pub fn get_type_of_symbol(&self, ctx: &Context, symbol: SymbolId) -> TypeId {
        let program = self.get_program();
        let (checker, _done) = ls_program::get_type_checker(program, ctx);
        let c = &mut *checker.borrow_mut();
        let result = c.get_type_of_symbol_at_location(symbol, Node::NIL);
        result
    }
}
