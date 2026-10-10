//! Go `internal/project/ata/validatepackagename.go`.

use crate::project::ata::prelude::*;

use crate::frontend::scanner::scanner_p1::utf8_decode_rune_in_string;
use crate::gostd;

// Go: project/ata/validatepackagename.go:10 NameValidationResult
// PORT: Go `type NameValidationResult int` with `iota` consts. The consts
// keep their Go names as package-level consts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct NameValidationResult(pub i32);

// Go: project/ata/validatepackagename.go:13 NameOk .. NameContainsNonURISafeCharacters
pub const NAME_OK: NameValidationResult = NameValidationResult(0);
pub const EMPTY_NAME: NameValidationResult = NameValidationResult(1);
pub const NAME_TOO_LONG: NameValidationResult = NameValidationResult(2);
pub const NAME_STARTS_WITH_DOT: NameValidationResult = NameValidationResult(3);
pub const NAME_STARTS_WITH_UNDERSCORE: NameValidationResult = NameValidationResult(4);
pub const NAME_CONTAINS_NON_URI_SAFE_CHARACTERS: NameValidationResult = NameValidationResult(5);

// Go: project/ata/validatepackagename.go:21 maxPackageNameLength
// PORT: Go untyped const compared with `len`, so `usize`.
pub const MAX_PACKAGE_NAME_LENGTH: usize = 214;

// Go: project/ata/validatepackagename.go:28 ValidatePackageName
// Validates package name using rules defined at https://docs.npmjs.com/files/package.json
//
// @internal
pub fn validate_package_name(package_name: &str) -> (NameValidationResult, String, bool) {
    validate_package_name_worker(package_name /*supportScopedPackage*/, true)
}

// Go: project/ata/validatepackagename.go:32 validatePackageNameWorker
pub fn validate_package_name_worker(
    package_name: &str,
    support_scoped_package: bool,
) -> (NameValidationResult, String, bool) {
    let package_name_len = package_name.len();
    if package_name_len == 0 {
        return (EMPTY_NAME, String::new(), false);
    }
    if package_name_len > MAX_PACKAGE_NAME_LENGTH {
        return (NAME_TOO_LONG, String::new(), false);
    }
    let (first_char, _) = utf8_decode_rune_in_string(package_name, 0);
    if first_char == '.' as i32 {
        return (NAME_STARTS_WITH_DOT, String::new(), false);
    }
    if first_char == '_' as i32 {
        return (NAME_STARTS_WITH_UNDERSCORE, String::new(), false);
    }
    // check if name is scope package like: starts with @ and has one '/' in the middle
    // scoped packages are not currently supported
    if support_scoped_package {
        if let Some(without_scope) = package_name.strip_prefix('@') {
            // Go: strings.Cut returns (s, "", false) when "/" is missing.
            let (scope, scoped_package_name, found) = match without_scope.split_once('/') {
                Some((scope, scoped_package_name)) => (scope, scoped_package_name, true),
                None => (without_scope, "", false),
            };
            if found
                && !scope.is_empty()
                && !scoped_package_name.is_empty()
                && !scoped_package_name.contains('/')
            {
                let (scope_result, _, _) =
                    validate_package_name_worker(scope /*supportScopedPackage*/, false);
                if scope_result != NAME_OK {
                    return (scope_result, scope.to_string(), true);
                }
                let (package_result, _, _) = validate_package_name_worker(
                    scoped_package_name, /*supportScopedPackage*/
                    false,
                );
                if package_result != NAME_OK {
                    return (package_result, scoped_package_name.to_string(), false);
                }
                return (NAME_OK, String::new(), false);
            }
        }
    }
    if gostd::url::query_escape(package_name) != package_name {
        return (NAME_CONTAINS_NON_URI_SAFE_CHARACTERS, String::new(), false);
    }
    (NAME_OK, String::new(), false)
}

// Go: project/ata/validatepackagename.go:72 renderPackageNameValidationFailure
// @internal
pub fn render_package_name_validation_failure(
    typing: &str,
    result: NameValidationResult,
    name: &str,
    is_scope_name: bool,
) -> String {
    let kind = if is_scope_name { "Scope" } else { "Package" };
    let name = if name.is_empty() { typing } else { name };
    match result {
        EMPTY_NAME => format!("'{typing}':: {kind} name '{name}' cannot be empty"),
        NAME_TOO_LONG => format!(
            "'{typing}':: {kind} name '{name}' should be less than {MAX_PACKAGE_NAME_LENGTH} characters"
        ),
        NAME_STARTS_WITH_DOT => format!("'{typing}':: {kind} name '{name}' cannot start with '.'"),
        NAME_STARTS_WITH_UNDERSCORE => {
            format!("'{typing}':: {kind} name '{name}' cannot start with '_'")
        }
        NAME_CONTAINS_NON_URI_SAFE_CHARACTERS => {
            format!("'{typing}':: {kind} name '{name}' contains non URI safe characters")
        }
        NAME_OK => crate::core::go_panic("Unexpected Ok result".to_string()),
        _ => crate::core::go_panic("Unknown package name validation result".to_string()),
    }
}
