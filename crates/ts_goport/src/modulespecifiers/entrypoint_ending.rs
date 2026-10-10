//! Port of modulespecifiers/util.go lines 398 to 502
//! (`ProcessEntrypointEnding`). Only `ls/autoimport` uses it.

use crate::prelude::*;

use crate::frontend::module::{Ending, ResolvedEntrypoint};

use super::deps;
use super::preferences::get_allowed_endings_in_preferred_order;
use super::tspath;
use super::types::*;
use super::util::get_js_extension_for_declaration_file_extension;

// Go: modulespecifiers/util.go:410 ProcessEntrypointEnding
// ProcessEntrypointEnding processes a pre-computed module specifier from a package.json exports
// entrypoint according to the entrypoint's Ending type and the user's preferred endings.
// PORT: Go `module.TryGetJSExtensionForFile` is the package copy
// `deps::try_get_js_extension_for_file`. Go `strings.TrimSuffix` is
// `strip_suffix` with the unchanged string as the fallback.
pub fn process_entrypoint_ending(
    entrypoint: &ResolvedEntrypoint,
    prefs: &UserPreferences,
    host: &dyn ModuleSpecifierGenerationHost,
    options: &CompilerOptions,
    importing_source_file: &dyn SourceFileForSpecifierGeneration,
    allowed_endings: &[ModuleSpecifierEnding],
) -> String {
    let mut specifier = entrypoint.module_specifier.clone();
    if entrypoint.ending == Ending::FIXED {
        return specifier;
    }

    let computed_endings: Vec<ModuleSpecifierEnding>;
    let mut allowed_endings = allowed_endings;
    if allowed_endings.is_empty() {
        computed_endings = get_allowed_endings_in_preferred_order(
            prefs,
            host,
            options,
            importing_source_file,
            "",
            host.get_default_resolution_mode_for_file(importing_source_file.node()),
        );
        allowed_endings = &computed_endings;
    }

    let preferred_ending = allowed_endings[0];

    // Handle declaration file extensions
    let dts_extension = tspath::get_declaration_file_extension(&specifier);
    if !dts_extension.is_empty() {
        match preferred_ending {
            ModuleSpecifierEnding::TsExtension | ModuleSpecifierEnding::JsExtension => {
                // Map .d.ts -> .js, .d.mts -> .mjs, .d.cts -> .cjs
                let js_extension = get_js_extension_for_declaration_file_extension(&dts_extension);
                return tspath::change_any_extension(
                    &specifier,
                    &js_extension,
                    &[dts_extension.as_str()],
                    false,
                );
            }
            ModuleSpecifierEnding::Minimal | ModuleSpecifierEnding::Index => {
                if entrypoint.ending == Ending::CHANGEABLE {
                    // .d.mts/.d.cts must keep an extension; rewrite to .mjs/.cjs instead of dropping
                    if dts_extension == tspath::EXTENSION_DTS {
                        specifier =
                            tspath::remove_extension(&specifier, &dts_extension).to_string();
                        if preferred_ending == ModuleSpecifierEnding::Minimal {
                            specifier = specifier
                                .strip_suffix("/index")
                                .unwrap_or(&specifier)
                                .to_string();
                        }
                        return specifier;
                    }
                    let js_extension =
                        get_js_extension_for_declaration_file_extension(&dts_extension);
                    return tspath::change_any_extension(
                        &specifier,
                        &js_extension,
                        &[dts_extension.as_str()],
                        false,
                    );
                }
                // EndingExtensionChangeable - can only change extension, not remove it
                let js_extension = get_js_extension_for_declaration_file_extension(&dts_extension);
                return tspath::change_any_extension(
                    &specifier,
                    &js_extension,
                    &[dts_extension.as_str()],
                    false,
                );
            }
        }
    }

    // Handle .ts/.tsx/.mts/.cts extensions
    if tspath::file_extension_is_one_of(
        &specifier,
        &[
            tspath::EXTENSION_TS,
            tspath::EXTENSION_TSX,
            tspath::EXTENSION_MTS,
            tspath::EXTENSION_CTS,
        ],
    ) {
        match preferred_ending {
            ModuleSpecifierEnding::TsExtension => {
                return specifier;
            }
            ModuleSpecifierEnding::JsExtension => {
                let js_extension = deps::try_get_js_extension_for_file(&specifier, options);
                if !js_extension.is_empty() {
                    return format!(
                        "{}{}",
                        tspath::remove_file_extension(&specifier),
                        js_extension
                    );
                }
                return specifier;
            }
            ModuleSpecifierEnding::Minimal | ModuleSpecifierEnding::Index => {
                if entrypoint.ending == Ending::CHANGEABLE {
                    specifier = tspath::remove_file_extension(&specifier).to_string();
                    if preferred_ending == ModuleSpecifierEnding::Minimal {
                        specifier = specifier
                            .strip_suffix("/index")
                            .unwrap_or(&specifier)
                            .to_string();
                    }
                    return specifier;
                }
                // EndingExtensionChangeable - can only change extension, not remove it
                let js_extension = deps::try_get_js_extension_for_file(&specifier, options);
                if !js_extension.is_empty() {
                    return format!(
                        "{}{}",
                        tspath::remove_file_extension(&specifier),
                        js_extension
                    );
                }
                return specifier;
            }
        }
    }

    // Handle .js/.jsx/.mjs/.cjs extensions
    if tspath::file_extension_is_one_of(
        &specifier,
        &[
            tspath::EXTENSION_JS,
            tspath::EXTENSION_JSX,
            tspath::EXTENSION_MJS,
            tspath::EXTENSION_CJS,
        ],
    ) {
        match preferred_ending {
            ModuleSpecifierEnding::TsExtension | ModuleSpecifierEnding::JsExtension => {
                return specifier;
            }
            ModuleSpecifierEnding::Minimal | ModuleSpecifierEnding::Index => {
                if entrypoint.ending == Ending::CHANGEABLE {
                    specifier = tspath::remove_file_extension(&specifier).to_string();
                    if preferred_ending == ModuleSpecifierEnding::Minimal {
                        specifier = specifier
                            .strip_suffix("/index")
                            .unwrap_or(&specifier)
                            .to_string();
                    }
                    return specifier;
                }
                // EndingExtensionChangeable - keep the extension
                return specifier;
            }
        }
    }

    // For other extensions (like .json), return as-is
    specifier
}
