#!/usr/bin/env node

// Usage: node generate.mts

import fs from "node:fs";
import path from "node:path";
import url from "node:url";
import type {
    Enumeration,
    MetaModel,
    Notification,
    OrType,
    Property,
    ReferenceType,
    Request,
    Structure,
    Type,
    TypeAlias,
} from "./metaModelSchema.mts";

const __filename = url.fileURLToPath(new URL(import.meta.url));
const __dirname = path.dirname(__filename);
const repoRoot = path.resolve(__dirname, "../../../..");

const out = path.resolve(__dirname, "../lsp_generated.go");
const metaModelPath = path.resolve(__dirname, "metaModel.json");

if (!fs.existsSync(metaModelPath)) {
    console.error("Meta model file not found; did you forget to run fetchModel.mjs?");
    process.exit(1);
}

const model: MetaModel = JSON.parse(fs.readFileSync(metaModelPath, "utf-8"));

// Custom structures to add to the model
const customStructures: Structure[] = [
    {
        name: "InitializationOptions",
        properties: [
            {
                name: "disablePushDiagnostics",
                type: { kind: "base", name: "boolean" },
                optional: true,
                documentation: "DisablePushDiagnostics disables automatic pushing of diagnostics to the client.",
            },
            {
                name: "codeLensShowLocationsCommandName",
                type: { kind: "base", name: "string" },
                optional: true,
                documentation: "The client-side command name that resolved references/implementations `CodeLens` should trigger. Arguments passed will be `(DocumentUri, Position, Location[])`.",
            },
            {
                name: "userPreferences",
                type: { kind: "reference", name: "any" },
                optional: true,
                documentation: "userPreferences and/or formatting options if provided at initialization.",
            },
            {
                name: "enableTelemetry",
                type: { kind: "base", name: "boolean" },
                optional: true,
                documentation: "EnableTelemetry enables sending telemetry events from the server to the client.",
            },
            {
                name: "logVerbosity",
                type: { kind: "reference", name: "LogVerbosity" },
                optional: true,
                documentation: "The initial log verbosity level, matching the client's output channel log level at startup. Subsequent changes are sent via custom/setLogVerbosity.",
            },
            {
                name: "runExternalCode",
                type: { kind: "base", name: "boolean" },
                optional: true,
                documentation: "RunExternalCode allows configured content mappers to launch external plugin processes. The client should set this only for trusted workspaces. It mirrors the --runExternalCode CLI flag.",
            },
            {
                name: "trackFlakyDiagnostics",
                type: { kind: "reference", name: "DiagnosticFlakeLogLevel" },
                optional: true,
                documentation: "The level at which we track flaky diagnostics, if at all.",
            },
        ],
        documentation: "InitializationOptions contains user-provided initialization options.",
    },
    {
        name: "AutoImportFix",
        properties: [
            {
                name: "kind",
                type: { kind: "reference", name: "AutoImportFixKind" },
                omitzeroValue: true,
            },
            {
                name: "name",
                type: { kind: "base", name: "string" },
                omitzeroValue: true,
            },
            {
                name: "importKind",
                type: { kind: "reference", name: "ImportKind" },
            },
            {
                name: "useRequire",
                type: { kind: "base", name: "boolean" },
                omitzeroValue: true,
            },
            {
                name: "addAsTypeOnly",
                type: { kind: "reference", name: "AddAsTypeOnly" },
            },
            {
                name: "moduleSpecifier",
                type: { kind: "base", name: "string" },
                documentation: "The module specifier for this auto-import.",
                omitzeroValue: true,
            },
            {
                name: "importIndex",
                type: { kind: "base", name: "integer" },
                documentation: "Index of the import to modify when adding to an existing import declaration.",
            },
            {
                name: "usagePosition",
                type: { kind: "reference", name: "Position" },
                optional: true,
            },
            {
                name: "namespacePrefix",
                type: { kind: "base", name: "string" },
                omitzeroValue: true,
            },
        ],
        documentation: "AutoImportFix contains information about an auto-import suggestion.",
    },
    {
        name: "CompletionItemData",
        properties: [
            {
                name: "fileName",
                type: { kind: "base", name: "string" },
                documentation: "The file name where the completion was requested.",
                omitzeroValue: true,
            },
            {
                name: "position",
                type: { kind: "base", name: "integer" },
                documentation: "The position where the completion was requested.",
                omitzeroValue: true,
            },
            {
                name: "supplementalFileIndex",
                type: { kind: "base", name: "integer" },
                optional: true,
                documentation: "Zero-based index into the canonical file's supplemental source files. Absent when the completion was requested in the canonical file.",
            },
            {
                name: "source",
                type: { kind: "base", name: "string" },
                documentation: "Special source value for disambiguation.",
                omitzeroValue: true,
            },
            {
                name: "name",
                type: { kind: "base", name: "string" },
                documentation: "The name of the completion item.",
                omitzeroValue: true,
            },
            {
                name: "autoImport",
                type: { kind: "reference", name: "AutoImportFix" },
                optional: true,
                documentation: "Auto-import data for this completion item.",
            },
            {
                name: "isImportStatementCompletion",
                type: { kind: "base", name: "boolean" },
                omitzeroValue: true,
            },
        ],
        documentation: "CompletionItemData is preserved on a CompletionItem between CompletionRequest and CompletionResolveRequest.",
    },
    {
        name: "CodeLensData",
        properties: [
            {
                name: "kind",
                type: { kind: "reference", name: "CodeLensKind" },
                documentation: `The kind of the code lens ("references" or "implementations").`,
            },
            {
                name: "uri",
                type: { kind: "base", name: "DocumentUri" },
                documentation: `The document in which the code lens and its range are located.`,
            },
            {
                name: "position",
                type: { kind: "base", name: "integer" },
                documentation: `The position of the code lens declaration in its virtual source file.`,
            },
            {
                name: "supplementalFileIndex",
                type: { kind: "base", name: "integer" },
                optional: true,
                documentation: `Zero-based index into the canonical file's supplemental source files. Absent for the canonical source file.`,
            },
        ],
    },
    {
        name: "ExperimentalServerCapabilities",
        properties: [
            {
                name: "customSourceDefinitionProvider",
                type: { kind: "base", name: "boolean" },
                optional: true,
                documentation: "The server provides source definition support via custom/textDocument/sourceDefinition.",
            },
            {
                name: "customMultiDocumentHighlightProvider",
                type: { kind: "base", name: "boolean" },
                optional: true,
                documentation: "The server provides multi-document highlight support via custom/textDocument/multiDocumentHighlight.",
            },
        ],
        documentation: "ExperimentalServerCapabilities contains experimental capabilities under development.",
    },
    {
        name: "ExperimentalClientCapabilities",
        properties: [
            {
                name: "hoverVerbosityLevel",
                type: { kind: "base", name: "boolean" },
                optional: true,
                documentation: "The client supports hover verbosityLevel requests and canIncreaseVerbosity responses.",
            },
        ],
        documentation: "ExperimentalClientCapabilities contains experimental capabilities under development.",
    },
    {
        name: "VSOnAutoInsertOptions",
        properties: [
            {
                name: "_vs_triggerCharacters",
                type: { kind: "array", element: { kind: "base", name: "string" } },
                documentation: "List of trigger characters that trigger auto-insert.",
            },
        ],
        documentation: "Options for the textDocument/_vs_onAutoInsert provider capability.",
    },
    {
        name: "VSReferenceItem",
        properties: [
            {
                name: "_vs_id",
                type: { kind: "base", name: "integer" },
                documentation: "Unique identifier for this reference item.",
            },
            {
                name: "_vs_definitionId",
                type: { kind: "base", name: "integer" },
                optional: true,
                documentation: "The ID of the definition item this reference belongs to. Absent for definition items themselves.",
            },
            {
                name: "_vs_kind",
                type: { kind: "array", element: { kind: "reference", name: "VSReferenceKind" } },
                optional: true,
                documentation: "The kind(s) of this reference (read, write, etc.).",
            },
            {
                name: "_vs_location",
                type: { kind: "reference", name: "Location" },
                documentation: "The location of this reference.",
            },
            {
                name: "_vs_definitionText",
                type: { kind: "reference", name: "VSClassifiedTextElement" },
                optional: true,
                documentation: "Classified display text for the definition (used for grouping headers in the UI).",
            },
            {
                name: "_vs_projectName",
                type: { kind: "base", name: "string" },
                optional: true,
                documentation: "The project name for this reference.",
            },
            {
                name: "_vs_containingType",
                type: { kind: "base", name: "string" },
                optional: true,
                documentation: "The containing type for this reference.",
            },
        ],
        documentation: "A VS-specific reference item with grouping support for Find All References.",
    },
    {
        name: "VSOnAutoInsertParams",
        properties: [
            {
                name: "_vs_textDocument",
                type: { kind: "reference", name: "TextDocumentIdentifier" },
                documentation: "The text document.",
            },
            {
                name: "_vs_position",
                type: { kind: "reference", name: "Position" },
                documentation: "The position inside the text document.",
            },
            {
                name: "_vs_ch",
                type: { kind: "base", name: "string" },
                documentation: "The character that triggered the auto-insert.",
            },
        ],
        documentation: "Parameters for the textDocument/_vs_onAutoInsert request.",
    },
    {
        name: "VSOnAutoInsertResponseItem",
        properties: [
            {
                name: "_vs_textEditFormat",
                type: { kind: "reference", name: "InsertTextFormat" },
                documentation: "The format of the text edit (plaintext or snippet).",
            },
            {
                name: "_vs_textEdit",
                type: { kind: "reference", name: "TextEdit" },
                documentation: "The text edit to apply for the auto-insertion.",
            },
        ],
        documentation: "Response item for the textDocument/_vs_onAutoInsert request.",
    },
    {
        name: "RequestFailureTelemetryEvent",
        properties: [
            {
                name: "eventName",
                type: { kind: "stringLiteral", value: "languageServer.errorResponse" },
                documentation: "The name of the telemetry event.",
            },
            {
                name: "telemetryPurpose",
                type: { kind: "stringLiteral", value: "error" },
                documentation: "Indicates whether the reason for generating the event (e.g. general usage telemetry or errors).",
            },
            {
                name: "properties",
                type: { kind: "reference", name: "RequestFailureTelemetryProperties" },
                documentation: "The properties associated with the event.",
            },
        ],
        documentation: "A RequestFailureTelemetryEvent is sent when a request fails and the server recovers.",
    },
    {
        name: "RequestFailureTelemetryProperties",
        properties: [
            {
                name: "errorCode",
                type: { kind: "base", name: "string" },
                documentation: "The error code associated with the event.",
            },
            {
                name: "requestMethod",
                type: { kind: "base", name: "string" },
                documentation: "The method of the request that caused the event.",
            },
            {
                name: "stack",
                type: { kind: "base", name: "string" },
                documentation: "The stack trace associated with the event.",
            },
        ],
        documentation: "RequestFailureTelemetryProperties contains failure information when an LSP request manages to recover.",
    },
    {
        name: "ProfileParams",
        properties: [
            {
                name: "dir",
                type: { kind: "base", name: "string" },
                documentation: "The directory path where the profile should be saved.",
            },
        ],
        documentation: "Parameters for profiling requests.",
    },
    {
        name: "ProfileResult",
        properties: [
            {
                name: "file",
                type: { kind: "base", name: "string" },
                documentation: "The file path where the profile was saved.",
            },
        ],
        documentation: "Result of a profiling request.",
    },
    {
        name: "InitializeAPISessionParams",
        properties: [
            {
                name: "pipe",
                type: { kind: "base", name: "string" },
                optional: true,
                documentation: "Optional named pipe path on Windows. On Unix, the path must not exist and will be created as a private directory containing a socket named 'socket'. If not provided, a unique path will be generated.",
            },
        ],
        documentation: "Parameters for the initializeAPISession request.",
    },
    {
        name: "InitializeAPISessionResult",
        properties: [
            {
                name: "sessionId",
                type: { kind: "base", name: "string" },
                documentation: "The unique identifier for this API session.",
            },
            {
                name: "pipe",
                type: { kind: "base", name: "string" },
                documentation: "The path to the named pipe or Unix domain socket for API communication.",
            },
        ],
        documentation: "Result for the initializeAPISession request.",
    },
    {
        name: "ProjectInfoParams",
        properties: [
            {
                name: "textDocument",
                type: { kind: "reference", name: "TextDocumentIdentifier" },
                documentation: "The text document to get project info for.",
            },
        ],
        documentation: "Parameters for the custom/projectInfo request.",
    },
    {
        name: "ProjectInfoResult",
        properties: [
            {
                name: "configFilePath",
                type: { kind: "base", name: "string" },
                documentation: "The absolute path to the config file (e.g. /path/to/tsconfig.json) for the project that contains this file, or an empty string if the file is in an inferred project.",
            },
        ],
        documentation: "Result for the custom/projectInfo request.",
    },
    {
        name: "ContentMapperManifest",
        properties: [
            { name: "name", type: { kind: "base", name: "string" }, documentation: "Human-readable mapper name." },
            { name: "version", type: { kind: "base", name: "string" }, optional: true, documentation: "Mapper version." },
            { name: "exec", type: { kind: "array", element: { kind: "base", name: "string" } }, documentation: "Executable and arguments used to start the mapper." },
            { name: "cwd", type: { kind: "base", name: "string" }, optional: true, documentation: "Absolute working directory for the mapper process." },
            { name: "compilerOptions", type: { kind: "array", element: { kind: "base", name: "string" } }, optional: true, documentation: "Compiler option names forwarded to the mapper." },
            { name: "dynamicConfig", type: { kind: "base", name: "boolean" }, optional: true, documentation: "Whether the mapper uses project-scoped dynamic configuration." },
            { name: "outputExtensions", type: { kind: "map", key: { kind: "base", name: "string" }, value: { kind: "base", name: "string" } }, optional: true, documentation: "Default source-to-runtime output extension mappings." },
        ],
        documentation: "Inline content mapper manifest supplied by a contributing extension.",
    },
    {
        name: "InferredProjectContentMapperContribution",
        properties: [
            { name: "options", type: { kind: "reference", name: "LSPObject" }, optional: true, documentation: "Options supplied to transforms in inferred projects." },
            { name: "manifest", type: { kind: "reference", name: "ContentMapperManifest" }, documentation: "Inline manifest for the mapper contributed to inferred projects." },
        ],
        documentation: "Content mapper configuration contributed to inferred projects.",
    },
    {
        name: "ContentMapperContribution",
        properties: [
            { name: "contributorId", type: { kind: "base", name: "string" }, documentation: "Unique identifier of the contributor extension." },
            { name: "extensions", type: { kind: "array", element: { kind: "base", name: "string" } }, documentation: "File extensions handled by this content mapper." },
            { name: "inferredProjectContribution", type: { kind: "reference", name: "InferredProjectContentMapperContribution" }, optional: true, documentation: "When present, contributes this mapper to inferred projects." },
        ],
        documentation: "One extension-provided content mapper contribution.",
    },
    {
        name: "SetContentMapperContributionsParams",
        properties: [
            { name: "contributions", type: { kind: "array", element: { kind: "reference", name: "ContentMapperContribution" } }, documentation: "Complete replacement set of active extension contributions." },
            { name: "openDocuments", type: { kind: "array", element: { kind: "reference", name: "TextDocumentIdentifier" } }, documentation: "Currently open documents matching contributed extensions." },
        ],
        documentation: "Parameters for the custom/setContentMapperContributions request.",
    },
    {
        name: "SetLogVerbosityParams",
        properties: [
            {
                name: "verbosity",
                type: { kind: "reference", name: "LogVerbosity" },
                documentation: "The log verbosity level.",
            },
        ],
        documentation: "Parameters for the custom/setLogVerbosity notification.",
    },
    {
        name: "PerformanceStatsTelemetryEvent",
        properties: [
            {
                name: "eventName",
                type: { kind: "stringLiteral", value: "languageServer.performanceStats" },
                documentation: "The name of the telemetry event.",
            },
            {
                name: "telemetryPurpose",
                type: { kind: "stringLiteral", value: "usage" },
                documentation: "Indicates this is a usage telemetry event.",
            },
            {
                name: "measurements",
                type: { kind: "reference", name: "PerformanceStatsTelemetryMeasurements" },
                documentation: "Numeric measurements for this telemetry event.",
            },
        ],
        documentation: "A PerformanceStatsTelemetryEvent is sent periodically with performance and resource usage statistics.",
    },
    {
        name: "PerformanceStatsTelemetryMeasurements",
        properties: [
            { name: "openFileCount", type: { kind: "base", name: "decimal" }, omitzeroValue: true, documentation: "Number of files currently open in the editor." },
            { name: "uptimeSeconds", type: { kind: "base", name: "decimal" }, omitzeroValue: true, documentation: "Seconds since the session was initialized." },
            { name: "projectCount", type: { kind: "base", name: "decimal" }, omitzeroValue: true, documentation: "Number of loaded projects." },
            { name: "configCount", type: { kind: "base", name: "decimal" }, omitzeroValue: true, documentation: "Number of loaded config files." },
            { name: "cachedDiskFileCount", type: { kind: "base", name: "decimal" }, omitzeroValue: true, documentation: "Number of files cached from disk." },
            { name: "memoryUsedBytes", type: { kind: "base", name: "decimal" }, omitzeroValue: true, documentation: "Total memory mapped by the Go runtime in bytes." },
            { name: "goMemLimit", type: { kind: "base", name: "decimal" }, omitzeroValue: true, documentation: "GOMEMLIMIT value in bytes, or 0 if not set." },
            { name: "goGCPercent", type: { kind: "base", name: "decimal" }, omitzeroValue: true, documentation: "GOGC percentage value configured for the GC." },
            { name: "heapGoalBytes", type: { kind: "base", name: "decimal" }, omitzeroValue: true, documentation: "Heap size target the GC is working toward in bytes." },
            { name: "heapLiveBytes", type: { kind: "base", name: "decimal" }, omitzeroValue: true, documentation: "Bytes of live (reachable) heap objects." },
            { name: "heapObjectCount", type: { kind: "base", name: "decimal" }, omitzeroValue: true, documentation: "Number of live or unswept objects occupying heap memory." },
            { name: "heapStackBytes", type: { kind: "base", name: "decimal" }, omitzeroValue: true, documentation: "Heap memory reserved for goroutine stacks." },
            { name: "heapReleasedBytes", type: { kind: "base", name: "decimal" }, omitzeroValue: true, documentation: "Heap memory returned to the OS." },
            { name: "heapFreeBytes", type: { kind: "base", name: "decimal" }, omitzeroValue: true, documentation: "Heap memory that is free and eligible to be returned to the OS." },
            { name: "gcScanHeapBytes", type: { kind: "base", name: "decimal" }, omitzeroValue: true, documentation: "Total scannable heap bytes — how much the GC must traverse." },
            { name: "goMaxProcs", type: { kind: "base", name: "decimal" }, omitzeroValue: true, documentation: "The current GOMAXPROCS value." },
            { name: "goroutineCount", type: { kind: "base", name: "decimal" }, omitzeroValue: true, documentation: "Current number of goroutines." },
            { name: "gcCyclesTotal", type: { kind: "base", name: "decimal" }, omitzeroValue: true, documentation: "Total completed GC cycles." },
            { name: "gcCPUSeconds", type: { kind: "base", name: "decimal" }, omitzeroValue: true, documentation: "Cumulative CPU time spent in GC in seconds." },
            { name: "userCPUSeconds", type: { kind: "base", name: "decimal" }, omitzeroValue: true, documentation: "Cumulative CPU time spent in user Go code in seconds." },
            { name: "systemMemTotal", type: { kind: "base", name: "decimal" }, omitzeroValue: true, documentation: "Total physical memory on the system in bytes." },
            { name: "systemMemUsed", type: { kind: "base", name: "decimal" }, omitzeroValue: true, documentation: "Used physical memory on the system in bytes." },
            { name: "autoImportProjectBucketCount", type: { kind: "base", name: "decimal" }, omitzeroValue: true, documentation: "Number of auto-import project buckets." },
            { name: "autoImportNodeModulesBucketCount", type: { kind: "base", name: "decimal" }, omitzeroValue: true, documentation: "Number of auto-import node_modules buckets." },
            { name: "autoImportUniquePackageCount", type: { kind: "base", name: "decimal" }, omitzeroValue: true, documentation: "Unique packages across all node_modules buckets." },
            { name: "autoImportProjectExportCount", type: { kind: "base", name: "decimal" }, omitzeroValue: true, documentation: "Total indexed exports from project files." },
            { name: "autoImportNodeModulesExportCount", type: { kind: "base", name: "decimal" }, omitzeroValue: true, documentation: "Total indexed exports from node_modules." },
            { name: "autoImportProjectFileCount", type: { kind: "base", name: "decimal" }, omitzeroValue: true, documentation: "Total files tracked across project buckets." },
            { name: "autoImportNodeModulesFileCount", type: { kind: "base", name: "decimal" }, omitzeroValue: true, documentation: "Total files tracked across node_modules buckets." },
            { name: "autoImportNodeModulesUnfilteredBucketCount", type: { kind: "base", name: "decimal" }, omitzeroValue: true, documentation: "Number of node_modules buckets with no package.json filter." },
        ],
        documentation: "Numeric measurements for PerformanceStatsTelemetryEvent.",
    },
    {
        name: "ProjectInfoTelemetryEvent",
        properties: [
            {
                name: "eventName",
                type: { kind: "stringLiteral", value: "languageServer.projectInfo" },
                documentation: "The name of the telemetry event.",
            },
            {
                name: "telemetryPurpose",
                type: { kind: "stringLiteral", value: "usage" },
                documentation: "Indicates this is a usage telemetry event.",
            },
            {
                name: "properties",
                type: { kind: "map", key: { kind: "base", name: "string" }, value: { kind: "base", name: "string" } },
                documentation: "String properties for this telemetry event. Complex values (compilerOptions, fileStats) are JSON-stringified.",
            },
            {
                name: "measurements",
                type: { kind: "reference", name: "ProjectInfoTelemetryMeasurements" },
                documentation: "Numeric measurements for this telemetry event.",
            },
        ],
        documentation: "A ProjectInfoTelemetryEvent is sent once per project when it is first loaded.",
    },
    {
        name: "ProjectInfoTelemetryMeasurements",
        properties: [
            { name: "jsFileCount", type: { kind: "base", name: "decimal" }, omitzeroValue: true },
            { name: "jsFileSize", type: { kind: "base", name: "decimal" }, omitzeroValue: true },
            { name: "jsxFileCount", type: { kind: "base", name: "decimal" }, omitzeroValue: true },
            { name: "jsxFileSize", type: { kind: "base", name: "decimal" }, omitzeroValue: true },
            { name: "tsFileCount", type: { kind: "base", name: "decimal" }, omitzeroValue: true },
            { name: "tsFileSize", type: { kind: "base", name: "decimal" }, omitzeroValue: true },
            { name: "tsxFileCount", type: { kind: "base", name: "decimal" }, omitzeroValue: true },
            { name: "tsxFileSize", type: { kind: "base", name: "decimal" }, omitzeroValue: true },
            { name: "dtsFileCount", type: { kind: "base", name: "decimal" }, omitzeroValue: true },
            { name: "dtsFileSize", type: { kind: "base", name: "decimal" }, omitzeroValue: true },
        ],
        documentation: "Numeric measurements for ProjectInfoTelemetryEvent.",
    },
    {
        name: "MultiDocumentHighlight",
        properties: [
            {
                name: "uri",
                type: { kind: "base", name: "DocumentUri" },
                documentation: "The URI of the document containing the highlights.",
            },
            {
                name: "highlights",
                type: { kind: "array", element: { kind: "reference", name: "DocumentHighlight" } },
                documentation: "The highlights for the document.",
            },
        ],
        documentation: "Represents a collection of document highlights from a single document, used in multi-document highlight responses.",
    },
    {
        name: "MultiDocumentHighlightParams",
        properties: [
            {
                name: "textDocument",
                type: { kind: "reference", name: "TextDocumentIdentifier" },
                documentation: "The text document.",
            },
            {
                name: "position",
                type: { kind: "reference", name: "Position" },
                documentation: "The position inside the text document.",
            },
            {
                name: "filesToSearch",
                type: { kind: "array", element: { kind: "base", name: "DocumentUri" } },
                documentation: "The list of file URIs to search for highlights across.",
            },
        ],
        documentation: "Parameters for the custom/textDocument/multiDocumentHighlight request.",
    },
    {
        name: "VSClassifiedTextRun",
        properties: [
            {
                name: "ClassificationTypeName",
                type: { kind: "base", name: "string" },
                documentation: "The classification type name (e.g. 'keyword', 'class name', 'parameter name').",
            },
            {
                name: "Text",
                type: { kind: "base", name: "string" },
                documentation: "The text content of this run.",
            },
            {
                name: "MarkerTagType",
                type: { kind: "base", name: "string" },
                optional: true,
                documentation: "Optional marker tag type.",
            },
            {
                name: "Style",
                type: { kind: "base", name: "integer" },
                optional: true,
                omitzeroValue: true,
                documentation: "The style of this text run.",
            },
            {
                name: "_vs_type",
                type: { kind: "stringLiteral", value: "ClassifiedTextRun" },
                documentation: "VS type discriminator required by ObjectContentConverter for deserialization.",
            },
        ],
        documentation: "A classified text run with text and classification type, used for colorized display in VS.",
    },
    {
        name: "VSClassifiedTextElement",
        properties: [
            {
                name: "Runs",
                type: { kind: "array", element: { kind: "reference", name: "VSClassifiedTextRun" } },
                documentation: "The classified text runs that make up this element.",
            },
            {
                name: "_vs_type",
                type: { kind: "stringLiteral", value: "ClassifiedTextElement" },
                documentation: "VS type discriminator required by ObjectContentConverter for deserialization.",
            },
        ],
        documentation: "A classified text element containing an array of classified text runs, used for colorized labels in VS.",
    },
    {
        name: "VSImageId",
        properties: [
            {
                name: "Guid",
                type: { kind: "base", name: "string" },
                documentation: "The GUID of the image catalog containing this image.",
            },
            {
                name: "Id",
                type: { kind: "base", name: "integer" },
                documentation: "The numeric identifier of the image within its catalog.",
            },
            {
                name: "_vs_type",
                type: { kind: "stringLiteral", value: "ImageId" },
                documentation: "VS type discriminator required by ObjectContentConverter for deserialization.",
            },
        ],
        documentation: "Identifies an image in a VS image catalog. Used to render symbol-kind icons (e.g. in hover tooltips).",
    },
    {
        name: "VSImageElement",
        properties: [
            {
                name: "ImageId",
                type: { kind: "reference", name: "VSImageId" },
                documentation: "The image to display.",
            },
            {
                name: "_vs_type",
                type: { kind: "stringLiteral", value: "ImageElement" },
                documentation: "VS type discriminator required by ObjectContentConverter for deserialization.",
            },
        ],
        documentation: "An image element (e.g. a symbol-kind icon) for use in VS rich content such as hover tooltips.",
    },
    {
        name: "VSContainerElement",
        properties: [
            {
                name: "Style",
                type: { kind: "reference", name: "VSContainerElementStyle" },
                documentation: "Layout style for the child elements.",
            },
            {
                name: "Elements",
                type: {
                    kind: "array",
                    element: {
                        kind: "or",
                        items: [
                            { kind: "reference", name: "VSImageElement" },
                            { kind: "reference", name: "VSClassifiedTextElement" },
                            { kind: "reference", name: "VSContainerElement" },
                        ],
                    },
                },
                documentation: "The child elements contained within this container.",
            },
            {
                name: "_vs_type",
                type: { kind: "stringLiteral", value: "ContainerElement" },
                documentation: "VS type discriminator required by ObjectContentConverter for deserialization.",
            },
        ],
        documentation: "A container element that groups other VS rich-content elements (images, classified text, or nested containers). Used to build the VS hover raw content that combines a symbol icon with colorized text.",
    },
];

const customEnumerations: Enumeration[] = [
    {
        name: "VSContainerElementStyle",
        type: { kind: "base", name: "integer" },
        values: [
            { name: "Wrapped", value: 0, documentation: "Child elements are laid out inline, wrapping as needed (e.g. an icon next to a signature line)." },
            { name: "Stacked", value: 1, documentation: "Child elements are stacked vertically, each on its own line (e.g. a signature line followed by documentation)." },
        ],
        documentation: "Layout style for a VSContainerElement's children, mirroring VS's Microsoft.VisualStudio.Text.Adornments.ContainerElementStyle.",
    },
    {
        name: "LogVerbosity",
        type: { kind: "base", name: "integer" },
        values: [
            { name: "Off", value: 0, documentation: "All logging disabled." },
            { name: "Trace", value: 1, documentation: "Most verbose; includes LSP request/response traces." },
            { name: "Debug", value: 2, documentation: "Verbose server logs." },
            { name: "Info", value: 3, documentation: "Normal server logs." },
            { name: "Warning", value: 4, documentation: "Warnings only." },
            { name: "Error", value: 5, documentation: "Errors only." },
        ],
        documentation: "Log verbosity level, mirroring the VS Code LogLevel enum values.",
    },
    {
        name: "DiagnosticFlakeLogLevel",
        type: { kind: "base", name: "integer" },
        values: [
            { name: "Off", value: 0, documentation: "All flake logging disabled." },
            { name: "Log", value: 1, documentation: "Log flaky diagnostics to the error log." },
            { name: "Panic", value: 2, documentation: "Panic on flaky diagnostics." },
        ],
        documentation: "Behavior for tracking and logging flaky diagnostics.",
    },
    {
        name: "VSReferenceKind",
        type: { kind: "base", name: "integer" },
        values: [
            { name: "Inactive", value: 0 },
            { name: "Comment", value: 1 },
            { name: "String", value: 2 },
            { name: "Read", value: 3 },
            { name: "Write", value: 4 },
            { name: "Reference", value: 5 },
            { name: "Name", value: 6 },
            { name: "Qualified", value: 7 },
            { name: "TypeArgument", value: 8 },
            { name: "TypeConstraint", value: 9 },
            { name: "BaseType", value: 10 },
            { name: "Constructor", value: 11 },
            { name: "Destructor", value: 12 },
            { name: "Import", value: 13 },
            { name: "Declaration", value: 14 },
            { name: "AddressOf", value: 15 },
            { name: "NotReference", value: 16 },
            { name: "Unknown", value: 17 },
        ],
    },
    {
        name: "CodeLensKind",
        type: {
            kind: "base",
            name: "string",
        },
        values: [
            {
                name: "References",
                value: "references",
            },
            {
                name: "Implementations",
                value: "implementations",
            },
        ],
    },
    {
        name: "AutoImportFixKind",
        type: { kind: "base", name: "integer" },
        values: [
            { name: "UseNamespace", value: 0, documentation: "Augment an existing namespace import." },
            { name: "JsdocTypeImport", value: 1, documentation: "Add a JSDoc-only type import." },
            { name: "AddToExisting", value: 2, documentation: "Insert into an existing import declaration." },
            { name: "AddNew", value: 3, documentation: "Create a fresh import statement." },
            { name: "PromoteTypeOnly", value: 4, documentation: "Promote a type-only import when necessary." },
        ],
    },
    {
        name: "ImportKind",
        type: { kind: "base", name: "integer" },
        values: [
            { name: "Named", value: 0, documentation: "Adds a named import." },
            { name: "Default", value: 1, documentation: "Adds a default import." },
            { name: "Namespace", value: 2, documentation: "Adds a namespace import." },
            { name: "CommonJS", value: 3, documentation: "Adds a CommonJS import assignment." },
        ],
    },
    {
        name: "AddAsTypeOnly",
        type: { kind: "base", name: "integer" },
        values: [
            { name: "Allowed", value: 1, documentation: "Import may be marked type-only if needed." },
            { name: "Required", value: 2, documentation: "Import must be marked type-only." },
            { name: "NotAllowed", value: 4, documentation: "Import cannot be marked type-only." },
        ],
    },
    {
        name: "ClassificationTypeName",
        type: { kind: "base", name: "string" },
        values: [
            { name: "Keyword", value: "keyword", documentation: "Language keyword (e.g., function, const, class)." },
            { name: "Punctuation", value: "punctuation", documentation: "Punctuation characters (e.g., parentheses, commas, semicolons)." },
            { name: "Operator", value: "operator", documentation: "Operators (e.g., =, +, ?)." },
            { name: "WhiteSpace", value: "whitespace", documentation: "Whitespace including spaces and line breaks." },
            { name: "Text", value: "text", documentation: "Plain text with no special classification." },
            { name: "String", value: "string", documentation: "String and literal values." },
            { name: "Number", value: "number", documentation: "Numeric literal values." },
            { name: "Comment", value: "comment", documentation: "Comment text." },
            { name: "ClassName", value: "class name", documentation: "Class names." },
            { name: "InterfaceName", value: "interface name", documentation: "Interface names." },
            { name: "EnumName", value: "enum name", documentation: "Enum names." },
            { name: "ModuleName", value: "module name", documentation: "Module/namespace names." },
            { name: "MethodName", value: "method name", documentation: "Method and function names." },
            { name: "ParameterName", value: "parameter name", documentation: "Parameter names." },
            { name: "PropertyName", value: "property name", documentation: "Property and accessor names." },
            { name: "FieldName", value: "field name", documentation: "Field names (e.g., enum members)." },
            { name: "LocalName", value: "local name", documentation: "Local variable names." },
            { name: "TypeParameterName", value: "type parameter name", documentation: "Type parameter names." },
            { name: "Identifier", value: "identifier", documentation: "General identifiers (e.g., type aliases, imports)." },
        ],
        documentation: "Roslyn classification type names used by VS for syntax coloring in tooltips and other UI elements.",
    },
];
const customRequests: Request[] = [
    {
        method: "custom/runGC",
        typeName: "RunGCRequest",
        messageDirection: "clientToServer",
        result: { kind: "base", name: "null" },
        documentation: "Triggers garbage collection in the language server.",
    },
    {
        method: "custom/saveHeapProfile",
        typeName: "SaveHeapProfileRequest",
        params: { kind: "reference", name: "ProfileParams" },
        messageDirection: "clientToServer",
        result: { kind: "reference", name: "ProfileResult" },
        documentation: "Saves a heap profile to the specified directory.",
    },
    {
        method: "custom/saveAllocProfile",
        typeName: "SaveAllocProfileRequest",
        params: { kind: "reference", name: "ProfileParams" },
        messageDirection: "clientToServer",
        result: { kind: "reference", name: "ProfileResult" },
        documentation: "Saves an allocation profile to the specified directory.",
    },
    {
        method: "custom/startCPUProfile",
        typeName: "StartCPUProfileRequest",
        params: { kind: "reference", name: "ProfileParams" },
        messageDirection: "clientToServer",
        result: { kind: "base", name: "null" },
        documentation: "Starts CPU profiling, writing to the specified directory when stopped.",
    },
    {
        method: "custom/stopCPUProfile",
        typeName: "StopCPUProfileRequest",
        messageDirection: "clientToServer",
        result: { kind: "reference", name: "ProfileResult" },
        documentation: "Stops CPU profiling and saves the profile.",
    },
    {
        method: "custom/initializeAPISession",
        typeName: "CustomInitializeAPISessionRequest",
        params: { kind: "reference", name: "InitializeAPISessionParams" },
        result: { kind: "reference", name: "InitializeAPISessionResult" },
        messageDirection: "clientToServer",
        documentation: "Custom request to initialize an API session.",
    },
    {
        method: "custom/projectInfo",
        typeName: "CustomProjectInfoRequest",
        params: { kind: "reference", name: "ProjectInfoParams" },
        result: { kind: "reference", name: "ProjectInfoResult" },
        messageDirection: "clientToServer",
        documentation: "Returns project information (e.g. the tsconfig.json path) for a given text document.",
    },
    {
        method: "custom/setContentMapperContributions",
        typeName: "CustomSetContentMapperContributionsRequest",
        params: { kind: "reference", name: "SetContentMapperContributionsParams" },
        result: { kind: "base", name: "null" },
        messageDirection: "clientToServer",
        documentation: "Replaces extension content mapper contributions and discovers configured mappers for matching open documents.",
    },
    {
        method: "custom/textDocument/sourceDefinition",
        typeName: "CustomTextDocumentSourceDefinitionRequest",
        params: { kind: "reference", name: "TextDocumentPositionParams" },
        result: { kind: "reference", name: "LocationOrLocationsOrDefinitionLinksOrNull" },
        messageDirection: "clientToServer",
        documentation: "Request to get source definitions for a position.",
    },
    {
        method: "custom/textDocument/multiDocumentHighlight",
        typeName: "CustomMultiDocumentHighlightRequest",
        params: { kind: "reference", name: "MultiDocumentHighlightParams" },
        result: {
            kind: "or",
            items: [
                { kind: "array", element: { kind: "reference", name: "MultiDocumentHighlight" } },
                { kind: "base", name: "null" },
            ],
        },
        messageDirection: "clientToServer",
        documentation: "Request to get document highlights across multiple files.",
    },
    {
        method: "textDocument/_vs_onAutoInsert",
        typeName: "VSOnAutoInsertRequest",
        params: { kind: "reference", name: "VSOnAutoInsertParams" },
        result: {
            kind: "or",
            items: [
                { kind: "reference", name: "VSOnAutoInsertResponseItem" },
                { kind: "base", name: "null" },
            ],
        },
        messageDirection: "clientToServer",
        documentation: "Request for auto-insert when a trigger character is typed (VS-specific).",
    },
    {
        method: "textDocument/_vs_references",
        typeName: "VSReferencesRequest",
        params: { kind: "reference", name: "ReferenceParams" },
        result: {
            kind: "or",
            items: [
                { kind: "array", element: { kind: "reference", name: "VSReferenceItem" } },
                { kind: "base", name: "null" },
            ],
        },
        messageDirection: "clientToServer",
        documentation: "VS-specific request for Find All References with grouped reference items.",
    },
];

const customNotifications: Notification[] = [
    {
        method: "custom/setLogVerbosity",
        typeName: "CustomSetLogVerbosityNotification",
        params: { kind: "reference", name: "SetLogVerbosityParams" },
        messageDirection: "clientToServer",
        documentation: "Notification to set the server's log verbosity level based on the output channel's log level.",
    },
];

// compareStructures is the set of generated structures for which a Compare method should be emitted.
// The Compare method defines a total ordering by comparing fields in declaration order.
// All listed structures (and any structure-typed fields they reference) must contain only
// comparable fields: base scalar types, or other structures that are themselves in this set.
const compareStructures = new Set<string>([
    "Position",
    "Range",
    "TextEdit",
]);

const customTypeAliases: TypeAlias[] = [
    {
        name: "TelemetryEvent",
        type: {
            kind: "or",
            items: [
                { kind: "reference", name: "RequestFailureTelemetryEvent" },
                { kind: "reference", name: "PerformanceStatsTelemetryEvent" },
                { kind: "reference", name: "ProjectInfoTelemetryEvent" },
                { kind: "base", name: "null" },
            ],
        },
    },
];

// Track which custom Data structures were declared explicitly
const explicitDataStructures = new Set(customStructures.map(s => s.name));

// Map from registration method → { fieldName, optionsTypeName }
// Built during patchAndPreprocessModel, used during code generation.
interface RegistrationMethodInfo {
    registrationMethod: string;
    fieldName: string;
    optionsTypeName: string;
    isRegistrationOnly?: boolean;
}
let registrationMethods: RegistrationMethodInfo[] = [];

// Patch and preprocess the model
function patchAndPreprocessModel() {
    // Track which Data types we need to create as placeholders
    const neededDataStructures = new Set<string>();

    // Collect all registration option types from requests and notifications
    const registrationOptionTypes: Type[] = [];
    for (const request of [...model.requests, ...model.notifications]) {
        if (request.registrationOptions) {
            registrationOptionTypes.push(request.registrationOptions);
        }
    }

    // Create synthetic structures for "and" types in registration options
    const syntheticStructures: Structure[] = [];
    for (let i = 0; i < registrationOptionTypes.length; i++) {
        const regOptType = registrationOptionTypes[i];
        if (regOptType.kind === "and") {
            // Find which request/notification this registration option belongs to
            const owner = [...model.requests, ...model.notifications].find(r => r.registrationOptions === regOptType);
            if (!owner) {
                throw new Error("Could not find owner for 'and' type registration option");
            }

            // Determine the proper name based on the typeName or method
            let structureName: string;
            if (owner.typeName) {
                // Use typeName as base: "ColorPresentationRequest" -> "ColorPresentationRegistrationOptions"
                structureName = owner.typeName.replace(/Request$/, "").replace(/Notification$/, "") + "RegistrationOptions";
            }
            else {
                // Fall back to method: "textDocument/colorPresentation" -> "ColorPresentationRegistrationOptions"
                const methodParts = owner.method.split("/");
                const lastPart = methodParts[methodParts.length - 1];
                structureName = titleCase(lastPart) + "RegistrationOptions";
            }

            // Extract all reference types from the "and"
            const refTypes = regOptType.items.filter((item): item is ReferenceType => item.kind === "reference");

            // Create a synthetic structure that combines all the referenced structures
            syntheticStructures.push({
                name: structureName,
                properties: [],
                extends: refTypes,
                documentation: `Registration options for ${owner.method}.`,
            });

            // Replace the "and" type with a reference to the synthetic structure
            registrationOptionTypes[i] = { kind: "reference", name: structureName };
            // Also update the model so the request/notification has the resolved type
            owner.registrationOptions = registrationOptionTypes[i];
        }
    }

    for (const structure of model.structures) {
        // Patch ServerCapabilities to add custom tsgo capability flags
        if (structure.name === "ServerCapabilities") {
            structure.properties.push({
                name: "_vs_onAutoInsertProvider",
                type: { kind: "reference", name: "VSOnAutoInsertOptions" },
                optional: true,
                documentation: "Provider options for the VS auto-insert feature via textDocument/_vs_onAutoInsert.",
            });
            structure.properties.push({
                name: "_vs_referencesProvider",
                type: { kind: "base", name: "boolean" },
                optional: true,
                documentation: "The server provides VS-specific grouped references via textDocument/_vs_references.",
            });
        }

        // Patch HoverParams to add verbosityLevel
        if (structure.name === "HoverParams") {
            structure.properties.push({
                name: "verbosityLevel",
                type: { kind: "base", name: "integer" },
                optional: true,
                documentation: "Controls how many levels of type definitions will be expanded. Default is 0.",
            });
        }

        // Patch WorkspaceSymbolParams to optionally scope the search to projects
        // containing a document, matching Strada's currentProject mode.
        if (structure.name === "WorkspaceSymbolParams") {
            structure.properties.push({
                name: "textDocument",
                type: { kind: "reference", name: "TextDocumentIdentifier" },
                optional: true,
                documentation: "Scopes the workspace symbol search to projects containing this document.",
            });
        }

        // Patch Hover to add canIncreaseVerbosity
        if (structure.name === "Hover") {
            structure.properties.push(
                {
                    name: "canIncreaseVerbosity",
                    type: { kind: "base", name: "boolean" },
                    omitzeroValue: true,
                    documentation: "Whether the verbosity level can be increased for this hover.",
                },
                {
                    name: "_vs_rawContent",
                    type: { kind: "reference", name: "VSContainerElement" },
                    optional: true,
                    documentation: "VS-specific rich content (symbol icon + colorized/classified text) rendered by clients that support Visual Studio extensions, in place of `contents`.",
                },
            );
        }

        // Patch ClientCapabilities to add VS-specific client capabilities
        if (structure.name === "ClientCapabilities") {
            structure.properties.push(
                {
                    name: "_vs_supportsVisualStudioExtensions",
                    type: { kind: "base", name: "boolean" },
                    optional: true,
                    documentation: "Whether the client supports Visual Studio extensions.",
                },
                {
                    name: "_vs_supportedSnippetVersion",
                    type: { kind: "base", name: "integer" },
                    optional: true,
                    documentation: "The snippet version supported by the client.",
                },
                {
                    name: "_vs_supportsNotIncludingTextInTextDocumentDidOpen",
                    type: { kind: "base", name: "boolean" },
                    optional: true,
                    documentation: "Whether the client supports not including text in textDocument/didOpen notifications.",
                },
                {
                    name: "_vs_supportsIconExtensions",
                    type: { kind: "base", name: "boolean" },
                    optional: true,
                    documentation: "Whether the client supports icon extensions.",
                },
                {
                    name: "_vs_supportsDiagnosticRequests",
                    type: { kind: "base", name: "boolean" },
                    optional: true,
                    documentation: "Whether the client supports diagnostic requests.",
                },
            );
        }

        // Patch SignatureInformation to add VS-specific colorized label
        if (structure.name === "SignatureInformation") {
            structure.properties.push({
                name: "_vs_colorizedLabel",
                type: { kind: "reference", name: "VSClassifiedTextElement" },
                optional: true,
                documentation: "A colorized label for the signature, providing classified text runs for VS syntax coloring.",
            });
        }

        for (const prop of structure.properties) {
            // Replace initializationOptions type with custom InitializationOptions.
            // The spec types this field as LSPAny?, which includes null, so keep
            // it nullable so a null value sent by loose clients is accepted.
            if (prop.name === "initializationOptions" && prop.type.kind === "reference" && prop.type.name === "LSPAny") {
                prop.type = {
                    kind: "or",
                    items: [
                        { kind: "reference", name: "InitializationOptions" },
                        { kind: "base", name: "null" },
                    ],
                };
            }

            // Replace Data *any fields with custom typed Data fields
            if (prop.name === "data" && prop.type.kind === "reference" && prop.type.name === "LSPAny") {
                const customDataType = `${structure.name}Data`;
                prop.type = { kind: "reference", name: customDataType };

                // If we haven't explicitly declared this Data structure, we'll need a placeholder
                if (!explicitDataStructures.has(customDataType)) {
                    neededDataStructures.add(customDataType);
                }
            }

            // Registration.registerOptions and Registration.method are handled specially:
            // registerOptions becomes a custom struct, and method is derived from it.
            // Remove both from the structure so the normal generator skips them.
            if (structure.name === "Registration" && (prop.name === "registerOptions" || prop.name === "method")) {
                // Will be filtered out below
            }

            // Replace ProgressParams.value with a proper union type
            if (structure.name === "ProgressParams" && prop.name === "value" && prop.type.kind === "reference" && prop.type.name === "LSPAny") {
                prop.type = {
                    kind: "or",
                    items: [
                        { kind: "reference", name: "WorkDoneProgressBegin" },
                        { kind: "reference", name: "WorkDoneProgressReport" },
                        { kind: "reference", name: "WorkDoneProgressEnd" },
                    ],
                };
            }
        }
    }

    for (const notification of model.notifications) {
        if (notification.typeName === "TelemetryEventNotification") {
            notification.params = {
                kind: "reference",
                name: "TelemetryEvent",
            };
        }
    }

    // Create placeholder structures for Data types that weren't explicitly declared
    for (const dataTypeName of neededDataStructures) {
        const baseName = dataTypeName.replace(/Data$/, "");
        customStructures.push({
            name: dataTypeName,
            properties: [],
            documentation: `${dataTypeName} is a placeholder for custom data preserved on a ${baseName}.`,
        });
    }

    // Add custom enumerations, custom structures, custom requests, and synthetic structures to the model
    model.enumerations.push(...customEnumerations);
    model.structures.push(...customStructures, ...syntheticStructures);
    model.requests.push(...customRequests);
    model.notifications.push(...customNotifications);

    // Build structure map for preprocessing
    const structureMap = new Map<string, Structure>();
    for (const structure of model.structures) {
        structureMap.set(structure.name, structure);
    }

    function collectInheritedProperties(structure: Structure, visited = new Set<string>()): Property[] {
        if (visited.has(structure.name)) {
            return []; // Avoid circular dependencies
        }
        visited.add(structure.name);

        const properties: Property[] = [];
        const inheritanceTypes = [...(structure.extends || []), ...(structure.mixins || [])];

        for (const type of inheritanceTypes) {
            if (type.kind === "reference") {
                const inheritedStructure = structureMap.get(type.name);
                if (inheritedStructure) {
                    properties.push(
                        ...collectInheritedProperties(inheritedStructure, new Set(visited)),
                        ...inheritedStructure.properties,
                    );
                }
            }
        }

        return properties;
    }

    // Inline inheritance for each structure
    for (const structure of model.structures) {
        const inheritedProperties = collectInheritedProperties(structure);

        // Merge properties with structure's own properties taking precedence
        const propertyMap = new Map<string, Property>();

        inheritedProperties.forEach(prop => propertyMap.set(prop.name, prop));
        structure.properties.forEach(prop => propertyMap.set(prop.name, prop));

        structure.properties = Array.from(propertyMap.values());
        structure.extends = undefined;
        structure.mixins = undefined;

        // Replace experimental LSPAny with typed ExperimentalClientCapabilities in ClientCapabilities
        if (structure.name === "ClientCapabilities") {
            const expProp = structure.properties.find(p => p.name === "experimental");
            if (expProp) {
                expProp.type = { kind: "reference", name: "ExperimentalClientCapabilities" };
                expProp.optional = true;
            }
        }

        // Replace experimental LSPAny with typed ExperimentalServerCapabilities in ServerCapabilities
        if (structure.name === "ServerCapabilities") {
            const expProp = structure.properties.find(p => p.name === "experimental");
            if (expProp) {
                expProp.type = { kind: "reference", name: "ExperimentalServerCapabilities" };
                expProp.optional = true;
            }
        }

        // Remove method and registerOptions from Registration (handled by custom codegen)
        if (structure.name === "Registration") {
            structure.properties = structure.properties.filter(p => p.name !== "method" && p.name !== "registerOptions");
        }
    }

    // Remove _InitializeParams structure after flattening (it was only needed for inheritance)
    model.structures = model.structures.filter(s => s.name !== "_InitializeParams");

    // Remove all notebook-related features from the model
    function isNotebookRelatedName(name: string): boolean {
        const lower = name.toLowerCase();
        return lower.includes("notebook");
    }

    function isNotebookRelatedMethod(method: string): boolean {
        return method.toLowerCase().startsWith("notebookdocument/");
    }

    function typeReferencesNotebook(type: Type): boolean {
        if (type.kind === "reference") return isNotebookRelatedName(type.name);
        if (type.kind === "array") return typeReferencesNotebook(type.element);
        if (type.kind === "or" || type.kind === "and") return type.items.some(typeReferencesNotebook);
        if (type.kind === "map") return typeReferencesNotebook(type.key) || typeReferencesNotebook(type.value);
        return false;
    }

    function isEntirelyNotebookType(type: Type): boolean {
        if (type.kind === "reference") return isNotebookRelatedName(type.name);
        if (type.kind === "array") return isEntirelyNotebookType(type.element);
        if (type.kind === "or" || type.kind === "and") return type.items.every(isEntirelyNotebookType);
        return false;
    }

    function removeNotebookFromType(type: Type): Type {
        if (type.kind === "or") {
            const filtered = type.items.filter(item => !typeReferencesNotebook(item)).map(removeNotebookFromType);
            if (filtered.length === 1) return filtered[0];
            if (filtered.length < type.items.length) {
                return { ...type, items: filtered };
            }
        }
        if (type.kind === "and") {
            const filtered = type.items.filter(item => !typeReferencesNotebook(item)).map(removeNotebookFromType);
            if (filtered.length === 1) return filtered[0];
            if (filtered.length < type.items.length) {
                return { ...type, items: filtered };
            }
        }
        return type;
    }

    // Filter out notebook structures (and notebook-only structures like ExecutionSummary)
    const notebookOnlyStructures = new Set(["ExecutionSummary"]);
    model.structures = model.structures.filter(s => !isNotebookRelatedName(s.name) && !notebookOnlyStructures.has(s.name));

    // Remove notebook properties from remaining structures
    for (const structure of model.structures) {
        structure.properties = structure.properties.filter(p => {
            if (isNotebookRelatedName(p.name)) return false;
            // Only remove properties whose type is entirely notebook-related
            if (isEntirelyNotebookType(p.type)) return false;
            return true;
        });
        // Clean up union types in remaining properties to remove notebook members
        for (const prop of structure.properties) {
            prop.type = removeNotebookFromType(prop.type);
        }
    }

    // Filter out notebook notifications and requests
    model.notifications = model.notifications.filter(n => !isNotebookRelatedMethod(n.method));
    model.requests = model.requests.filter(r => !isNotebookRelatedMethod(r.method));

    // Filter out notebook enumerations
    model.enumerations = model.enumerations.filter(e => !isNotebookRelatedName(e.name));

    // Remove notebook-related values from remaining enumerations
    for (const enumeration of model.enumerations) {
        enumeration.values = enumeration.values.filter(v => !isNotebookRelatedName(v.name));
    }

    // Filter out notebook type aliases
    model.typeAliases = model.typeAliases.filter(ta => !isNotebookRelatedName(ta.name));

    // Clean up type aliases that reference notebook types (e.g., DocumentFilter)
    for (const ta of model.typeAliases) {
        if (ta.type.kind === "or") {
            ta.type.items = ta.type.items.filter(item => !typeReferencesNotebook(item));
            // If only one item remains, unwrap the union
            if (ta.type.items.length === 1) {
                ta.type = ta.type.items[0];
            }
        }
    }

    // Build the registration method map (after notebook filtering).
    // Each unique registration method gets a field in the generated RegisterOptions struct.
    const regMethodSeen = new Set<string>();
    for (const request of [...model.requests, ...model.notifications]) {
        if (!request.registrationOptions) continue;
        const regMethod = (request as any).registrationMethod || request.method;

        if (regMethodSeen.has(regMethod)) continue;
        regMethodSeen.add(regMethod);

        // Resolve the options type name
        const ro = request.registrationOptions;
        let optionsTypeName: string;
        if (ro.kind === "reference") {
            optionsTypeName = ro.name;
        }
        else {
            throw new Error(`Unexpected registrationOptions kind '${ro.kind}' for ${request.method}; expected all to be resolved to references`);
        }

        registrationMethods.push({
            registrationMethod: regMethod,
            fieldName: methodNameIdentifier(regMethod),
            optionsTypeName,
        });
    }

    // Identify registration-only methods (not also a request/notification method).
    // These need their own Method constant emitted.
    const allRequestMethods = new Set([...model.requests, ...model.notifications].map(r => r.method));
    for (const reg of registrationMethods) {
        (reg as any).isRegistrationOnly = !allRequestMethods.has(reg.registrationMethod);
    }

    // Merge LSPErrorCodes into ErrorCodes and remove LSPErrorCodes
    const errorCodesEnum = model.enumerations.find(e => e.name === "ErrorCodes");
    const lspErrorCodesEnum = model.enumerations.find(e => e.name === "LSPErrorCodes");
    if (errorCodesEnum && lspErrorCodesEnum) {
        // Merge LSPErrorCodes values into ErrorCodes
        errorCodesEnum.values.push(...lspErrorCodesEnum.values);
        // Remove LSPErrorCodes from the model
        model.enumerations = model.enumerations.filter(e => e.name !== "LSPErrorCodes");
    }

    // Singularize plural enum names (e.g., "ErrorCodes" -> "ErrorCode")
    for (const enumeration of model.enumerations) {
        if (enumeration.name.endsWith("Codes")) {
            enumeration.name = enumeration.name.slice(0, -1); // "Codes" -> "Code"
        }
        else if (enumeration.name.endsWith("Modifiers")) {
            enumeration.name = enumeration.name.slice(0, -1); // "Modifiers" -> "Modifier"
        }
        else if (enumeration.name.endsWith("Types")) {
            enumeration.name = enumeration.name.slice(0, -1); // "Types" -> "Type"
        }
    }
}

patchAndPreprocessModel();

// Validate that telemetry events in the TelemetryEvent union have properly shaped
// measurements and properties fields. measurements struct fields must only contain
// numeric types (decimal/integer/uinteger).
function validateTelemetryEvents() {
    const telemetryAlias = customTypeAliases.find(a => a.name === "TelemetryEvent");
    if (!telemetryAlias || telemetryAlias.type.kind !== "or") return;

    const structureMap = new Map(model.structures.map(s => [s.name, s]));

    for (const item of telemetryAlias.type.items) {
        if (item.kind !== "reference") continue;
        const eventStruct = structureMap.get(item.name);
        if (!eventStruct) continue;

        for (const prop of eventStruct.properties) {
            if (prop.name === "measurements" && prop.type.kind === "reference") {
                const measurementsStruct = structureMap.get(prop.type.name);
                if (!measurementsStruct) continue;
                for (const mp of measurementsStruct.properties) {
                    if (mp.type.kind !== "base" || !["decimal", "integer", "uinteger"].includes(mp.type.name)) {
                        throw new Error(
                            `Telemetry measurements struct ${prop.type.name}.${mp.name} must be a numeric type ` +
                                `(decimal/integer/uinteger), got ${mp.type.kind === "base" ? mp.type.name : mp.type.kind}`,
                        );
                    }
                }
            }
        }
    }
}

validateTelemetryEvents();

interface GoType {
    name: string;
    needsPointer: boolean;
}

interface TypeInfo {
    types: Map<string, GoType>;
    literalTypes: Map<string, string>;
    unionTypes: Map<string, { name: string; type: Type; containedNull: boolean; }[]>;
    typeAliasMap: Map<string, Type>;
}

const typeInfo: TypeInfo = {
    types: new Map(),
    literalTypes: new Map(),
    unionTypes: new Map(),
    typeAliasMap: new Map(),
};

function titleCase(s: string) {
    return s.charAt(0).toUpperCase() + s.slice(1);
}

function goFieldName(prop: Property): string {
    if (prop.name.startsWith("_vs_")) {
        return "VS" + titleCase(prop.name.slice(4));
    }
    return titleCase(prop.name);
}

function resolveType(type: Type): GoType {
    switch (type.kind) {
        case "base":
            switch (type.name) {
                case "integer":
                    return { name: "int32", needsPointer: false };
                case "uinteger":
                    return { name: "uint32", needsPointer: false };
                case "string":
                    return { name: "string", needsPointer: false };
                case "boolean":
                    return { name: "bool", needsPointer: false };
                case "URI":
                    return { name: "URI", needsPointer: false };
                case "DocumentUri":
                    return { name: "DocumentUri", needsPointer: false };
                case "decimal":
                    return { name: "float64", needsPointer: false };
                case "null":
                    return { name: "any", needsPointer: false };
                default:
                    throw new Error(`Unsupported base type: ${type.name}`);
            }

        case "reference":
            const typeAliasOverride = typeAliasOverrides.get(type.name);
            if (typeAliasOverride) {
                return typeAliasOverride;
            }

            const nonResolved = nonResolvedAliases.has(type.name);
            if (nonResolved) {
                return { name: type.name, needsPointer: false };
            }

            // Check if this is a type alias that resolves to a union type
            const aliasedType = typeInfo.typeAliasMap.get(type.name);
            if (aliasedType) {
                return resolveType(aliasedType);
            }

            let refType = typeInfo.types.get(type.name);
            if (!refType) {
                refType = { name: type.name, needsPointer: true };
                typeInfo.types.set(type.name, refType);
            }
            return refType;

        case "array": {
            const elementType = resolveType(type.element);
            const arrayTypeName = elementType.needsPointer
                ? `[]*${elementType.name}`
                : `[]${elementType.name}`;
            return {
                name: arrayTypeName,
                needsPointer: false,
            };
        }

        case "map": {
            const keyType = resolveType(type.key);
            const valueType = resolveType(type.value);
            const valueTypeName = valueType.needsPointer ? `*${valueType.name}` : valueType.name;

            return {
                name: `map[${keyType.name}]${valueTypeName}`,
                needsPointer: false,
            };
        }

        case "tuple": {
            if (
                type.items.length === 2 &&
                type.items[0].kind === "base" && type.items[0].name === "uinteger" &&
                type.items[1].kind === "base" && type.items[1].name === "uinteger"
            ) {
                return { name: "[2]uint32", needsPointer: false };
            }

            throw new Error("Unsupported tuple type: " + JSON.stringify(type));
        }

        case "stringLiteral": {
            const typeName = `StringLiteral${type.value.split(".").map(titleCase).join("")}`;
            typeInfo.literalTypes.set(String(type.value), typeName);
            return { name: typeName, needsPointer: false };
        }

        case "integerLiteral": {
            const typeName = `IntegerLiteral${type.value}`;
            typeInfo.literalTypes.set(String(type.value), typeName);
            return { name: typeName, needsPointer: false };
        }

        case "booleanLiteral": {
            const typeName = `BooleanLiteral${type.value ? "True" : "False"}`;
            typeInfo.literalTypes.set(String(type.value), typeName);
            return { name: typeName, needsPointer: false };
        }
        case "literal":
            if (type.value.properties.length === 0) {
                return { name: "struct{}", needsPointer: false };
            }

            throw new Error("Unexpected non-empty literal object: " + JSON.stringify(type.value));

        case "or": {
            return handleOrType(type);
        }

        default:
            throw new Error(`Unsupported type kind: ${type.kind}`);
    }
}

function flattenOrTypes(types: Type[]): Type[] {
    const flattened = new Set<Type>();

    for (const rawType of types) {
        let type = rawType;

        // Dereference reference types that point to OR types
        if (rawType.kind === "reference") {
            const aliasedType = typeInfo.typeAliasMap.get(rawType.name);
            if (aliasedType && aliasedType.kind === "or") {
                type = aliasedType;
            }
        }

        if (type.kind === "or") {
            // Recursively flatten OR types
            for (const subType of flattenOrTypes(type.items)) {
                flattened.add(subType);
            }
        }
        else {
            flattened.add(rawType);
        }
    }

    return Array.from(flattened);
}

function pluralize(name: string): string {
    // Handle common irregular plurals and special cases
    if (
        name.endsWith("s") || name.endsWith("x") || name.endsWith("z") ||
        name.endsWith("ch") || name.endsWith("sh")
    ) {
        return name + "es";
    }
    if (name.endsWith("y") && name.length > 1 && !"aeiou".includes(name[name.length - 2])) {
        return name.slice(0, -1) + "ies";
    }
    return name + "s";
}

function handleOrType(orType: OrType): GoType {
    // First, flatten any nested OR types
    const types = flattenOrTypes(orType.items);

    // Check for nullable types (OR with null)
    const nullIndex = types.findIndex(item => item.kind === "base" && item.name === "null");
    let containedNull = nullIndex !== -1;

    // If it's nullable, remove the null type from the list
    let nonNullTypes = types;
    if (containedNull) {
        nonNullTypes = types.filter((_, i) => i !== nullIndex);
    }

    // If no types remain after filtering null, this shouldn't happen
    if (nonNullTypes.length === 0) {
        throw new Error("Union type with only null is not supported: " + JSON.stringify(types));
    }

    // Even if only one type remains after filtering null, we still need to create a union type
    // to preserve the nullable behavior (all fields nil = null)

    let memberNames = nonNullTypes.map(type => {
        if (type.kind === "reference") {
            return type.name;
        }
        else if (type.kind === "base") {
            return titleCase(type.name);
        }
        else if (
            type.kind === "array" &&
            (type.element.kind === "reference" || type.element.kind === "base")
        ) {
            return pluralize(titleCase(type.element.name));
        }
        else if (type.kind === "array") {
            // Handle more complex array types
            const elementType = resolveType(type.element);
            return `${elementType.name}Array`;
        }
        else if (type.kind === "literal" && type.value.properties.length === 0) {
            return "EmptyObject";
        }
        else if (type.kind === "tuple") {
            return "Tuple";
        }
        else {
            throw new Error(`Unsupported type kind in union: ${type.kind}`);
        }
    });

    // Find longest common prefix of member names chunked by PascalCase
    function findLongestCommonPrefix(names: string[]): string {
        if (names.length === 0) return "";
        if (names.length === 1) return "";

        // Split each name into PascalCase chunks
        function splitPascalCase(name: string): string[] {
            const chunks: string[] = [];
            let currentChunk = "";

            for (let i = 0; i < name.length; i++) {
                const char = name[i];
                if (char >= "A" && char <= "Z" && currentChunk.length > 0) {
                    // Start of a new chunk
                    chunks.push(currentChunk);
                    currentChunk = char;
                }
                else {
                    currentChunk += char;
                }
            }

            if (currentChunk.length > 0) {
                chunks.push(currentChunk);
            }

            return chunks;
        }

        const allChunks = names.map(splitPascalCase);
        const minChunkLength = Math.min(...allChunks.map(chunks => chunks.length));

        // Find the longest common prefix of chunks
        let commonChunks: string[] = [];
        for (let i = 0; i < minChunkLength; i++) {
            const chunk = allChunks[0][i];
            if (allChunks.every(chunks => chunks[i] === chunk)) {
                commonChunks.push(chunk);
            }
            else {
                break;
            }
        }

        return commonChunks.join("");
    }

    const commonPrefix = findLongestCommonPrefix(memberNames);

    let unionTypeName = "";

    if (commonPrefix.length > 0) {
        const trimmedMemberNames = memberNames.map(name => name.slice(commonPrefix.length));
        if (trimmedMemberNames.every(name => name)) {
            unionTypeName = commonPrefix + trimmedMemberNames.join("Or");
            memberNames = trimmedMemberNames;
        }
        else {
            unionTypeName = memberNames.join("Or");
        }
    }
    else {
        unionTypeName = memberNames.join("Or");
    }

    if (containedNull) {
        unionTypeName += "OrNull";
    }
    else {
        containedNull = false;
    }

    const union = memberNames.map((name, i) => ({ name, type: nonNullTypes[i], containedNull }));

    typeInfo.unionTypes.set(unionTypeName, union);

    return {
        name: unionTypeName,
        needsPointer: false,
    };
}

const typeAliasOverrides = new Map([
    ["LSPAny", { name: "any", needsPointer: false }],
    ["LSPArray", { name: "[]any", needsPointer: false }],
    ["LSPObject", { name: "map[string]any", needsPointer: false }],
    ["uint64", { name: "uint64", needsPointer: false }],
]);

// These type aliases are intentionally not resolved to their underlying types.
// It means that we can end up with non-normalized union types in some places.
// Also, unlike other type aliases, these will get a type alias in the generated source code.
// We may want to eventually do this for all type aliases though.
const nonResolvedAliases = new Set(customTypeAliases.map(ta => ta.name));

/**
 * First pass: Resolve all type information
 */
function collectTypeDefinitions() {
    // Process all enumerations first to make them available for struct fields
    for (const enumeration of model.enumerations) {
        typeInfo.types.set(enumeration.name, {
            name: enumeration.name,
            needsPointer: false,
        });
    }

    const valueTypes = new Set([
        "Position",
        "Range",
        "Location",
        "Color",
        "TextDocumentIdentifier",
        "PreviousResultId",
        "VersionedTextDocumentIdentifier",
        "OptionalVersionedTextDocumentIdentifier",
        "ExportInfoMapKey",
    ]);

    // Process all structures
    for (const structure of model.structures) {
        typeInfo.types.set(structure.name, {
            name: structure.name,
            needsPointer: !valueTypes.has(structure.name),
        });
    }

    // Process all type aliases
    for (const typeAlias of model.typeAliases) {
        if (typeAliasOverrides.has(typeAlias.name)) {
            continue;
        }

        // Store the alias mapping so we can resolve it later
        typeInfo.typeAliasMap.set(typeAlias.name, typeAlias.type);
    }
}

function formatDocumentation(s: string | undefined): string {
    if (!s) return "";

    let lines: string[] = [];

    for (let line of s.split("\n")) {
        line = line.trimEnd();
        line = line.replace(/(\w ) +/g, "$1");
        // Some upstream docs include dangling block comment delimiters; remove them
        // so they don't leak into generated `//` comments.
        line = line.replace(/\s*\/\*+\s*/g, " ");
        line = line.replace(/\s*\*+\/\s*/g, " ");
        line = line.replace(/\s{2,}/g, " ").trimEnd();
        line = line.replace(/\{@link(?:code)?.*?([^} ]+)\}/g, "$1");
        line = line.replace(/^@(since|proposed|deprecated)(.*)/, (_, tag, rest) => {
            lines.push("");
            return `${titleCase(tag)}${rest ? ":" + rest : "."}`;
        });
        lines.push(line);
    }

    // filter out contiguous empty lines
    while (true) {
        const toRemove = lines.findIndex((line, index) => {
            if (line) return false;
            if (index === 0) return true;
            if (index === lines.length - 1) return true;
            return !(lines[index - 1] && lines[index + 1]);
        });
        if (toRemove === -1) break;
        lines.splice(toRemove, 1);
    }

    return lines.length > 0 ? "// " + lines.join("\n// ") + "\n" : "";
}

function methodNameIdentifier(name: string) {
    return name.split("/").map(v => {
        if (v === "$") return "";
        // Mirror goFieldName: "_vs_foo" -> "VSFoo".
        if (v.startsWith("_vs_")) return "VS" + titleCase(v.slice(4));
        return titleCase(v);
    }).join("");
}

/**
 * Returns the JSON token kind ("string", "number", "object", "array", "boolean")
 * for a given meta model Type, or undefined if the kind cannot be statically determined.
 */
function jsonKindForType(type: Type): string | undefined {
    switch (type.kind) {
        case "base":
            switch (type.name) {
                case "integer":
                case "uinteger":
                case "decimal":
                    return "number";
                case "string":
                case "URI":
                case "DocumentUri":
                    return "string";
                case "boolean":
                    return "boolean";
                default:
                    return undefined;
            }
        case "reference": {
            if (typeAliasOverrides.has(type.name)) {
                return undefined;
            }
            if (model.structures.some(s => s.name === type.name)) {
                return "object";
            }
            const enumeration = model.enumerations.find(e => e.name === type.name);
            if (enumeration) {
                switch (enumeration.type.name) {
                    case "string":
                        return "string";
                    case "integer":
                    case "uinteger":
                        return "number";
                    default:
                        return undefined;
                }
            }
            const aliasType = typeInfo.typeAliasMap.get(type.name);
            if (aliasType) return jsonKindForType(aliasType);
            return undefined;
        }
        case "array":
            return "array";
        case "map":
            return "object";
        case "tuple":
            return "array";
        case "stringLiteral":
            return "string";
        case "integerLiteral":
            return "number";
        case "booleanLiteral":
            return "boolean";
        case "literal":
            return "object";
        case "or": {
            const kinds = new Set(type.items.map(item => jsonKindForType(item)).filter(Boolean));
            return kinds.size === 1 ? kinds.values().next().value : undefined;
        }
        default:
            return undefined;
    }
}

function goKindCasesForJsonKind(kind: string): string {
    switch (kind) {
        case "string":
            return `case '"':`;
        case "number":
            return `case '0':`;
        case "object":
            return `case '{':`;
        case "array":
            return `case '[':`;
        case "boolean":
            return `case 't', 'f':`;
        default:
            return "";
    }
}

/**
 * Checks if a meta model Type can represent a JSON null value.
 * Used to determine whether to reject explicit JSON `null` for any field
 * that can otherwise decode `null` without a type error.
 */
function typeCanBeNull(type: Type): boolean {
    switch (type.kind) {
        case "base":
            return type.name === "null";
        case "reference": {
            const override = typeAliasOverrides.get(type.name);
            if (override) {
                return override.name === "any";
            }
            // A bare "any" reference resolves to Go's `any` (interface), which can hold null.
            if (type.name === "any") {
                return true;
            }
            if (nonResolvedAliases.has(type.name)) {
                const customAlias = customTypeAliases.find(t => t.name === type.name);
                if (customAlias) return typeCanBeNull(customAlias.type);
                return false;
            }
            const aliased = typeInfo.typeAliasMap.get(type.name);
            if (aliased) return typeCanBeNull(aliased);
            return false;
        }
        case "or":
            return type.items.some(item => typeCanBeNull(item));
        default:
            return false;
    }
}

/**
 * For a group of union entries that share the same JSON kind (e.g., all objects),
 * find a discriminator field — a JSON property whose string literal type differs
 * across variants — enabling efficient O(1) dispatch instead of try-each.
 */
function findDiscriminatorField(entries: { fieldName: string; typeName: string; originalType: Type; }[]): {
    fieldName: string;
    mapping: Map<string, { fieldName: string; typeName: string; originalType: Type; }>;
    unmapped: { fieldName: string; typeName: string; originalType: Type; }[];
} | null {
    // For each entry, find string literal fields and build candidate discriminators.
    // A valid discriminator is a field name that appears on multiple variants with
    // different string literal values.
    const fieldCandidates = new Map<string, Map<string, typeof entries[0] | undefined>>();

    for (const entry of entries) {
        if (entry.originalType.kind !== "reference") continue;
        const structure = model.structures.find(s => s.name === (entry.originalType as ReferenceType).name);
        if (!structure) continue;

        for (const prop of structure.properties) {
            if (prop.type.kind === "stringLiteral") {
                if (!fieldCandidates.has(prop.name)) {
                    fieldCandidates.set(prop.name, new Map());
                }

                const mapping = fieldCandidates.get(prop.name)!;
                if (!mapping.has(prop.type.value)) {
                    mapping.set(prop.type.value, entry);
                }
                else {
                    // Two entries share the same literal value; invalidate this candidate.
                    mapping.set(prop.type.value, undefined);
                }
            }
        }
    }

    // Pick the discriminator field that covers the most entries.
    let bestField: string | null = null;
    let bestMapping: Map<string, typeof entries[0]> | null = null;

    for (const [fieldName, mapping] of fieldCandidates) {
        const validMapping = new Map<string, typeof entries[0]>();
        for (const [value, entry] of mapping) {
            if (entry !== undefined) validMapping.set(value, entry);
        }
        if (validMapping.size >= 2 && (!bestMapping || validMapping.size > bestMapping.size)) {
            bestField = fieldName;
            bestMapping = validMapping;
        }
    }

    if (!bestField || !bestMapping) return null;

    const mappedEntries = new Set(bestMapping.values());
    const unmapped = entries.filter(e => !mappedEntries.has(e));

    return { fieldName: bestField, mapping: bestMapping, unmapped };
}

function hasCustomStructureCodec(name: string): boolean {
    return name === "Registration";
}

/**
 * For a group of union entries that share the same JSON kind, find fields whose
 * presence/absence in the JSON uniquely identifies a variant. A "presence discriminator"
 * for variant X is a required field on X that does not appear in any other variant's
 * property set at all.
 */
function findPresenceDiscriminator(entries: { fieldName: string; typeName: string; originalType: Type; }[]): {
    checks: { jsonFieldName: string; entry: { fieldName: string; typeName: string; originalType: Type; }; }[];
    unmapped: { fieldName: string; typeName: string; originalType: Type; }[];
} | null {
    // Collect all property names for each variant
    const variantProps = new Map<typeof entries[0], { required: Property[]; allNames: Set<string>; }>();
    for (const entry of entries) {
        if (entry.originalType.kind !== "reference") continue;
        const structure = model.structures.find(s => s.name === (entry.originalType as ReferenceType).name);
        if (!structure) continue;
        const required = structure.properties.filter(p => !p.optional && !p.omitzeroValue);
        const allNames = new Set(structure.properties.map(p => p.name));
        variantProps.set(entry, { required, allNames });
    }

    const checks: { jsonFieldName: string; entry: typeof entries[0]; }[] = [];
    const handled = new Set<typeof entries[0]>();

    for (const entry of entries) {
        const info = variantProps.get(entry);
        if (!info) continue;

        const otherEntries = entries.filter(e => e !== entry);
        for (const field of info.required) {
            const absentFromAllOthers = otherEntries.every(other => {
                const otherInfo = variantProps.get(other);
                if (!otherInfo) return false;
                return !otherInfo.allNames.has(field.name);
            });
            if (absentFromAllOthers) {
                checks.push({ jsonFieldName: field.name, entry });
                handled.add(entry);
                break;
            }
        }
    }

    if (checks.length === 0) return null;

    const unmapped = entries.filter(e => !handled.has(e));
    return { checks, unmapped };
}

/**
 * Generate the Go code
 *
 * PORT: this port of the pinned generator emits Rust instead of Go. Lines
 * 1-2176 above are a verbatim copy of the pinned generate.mts (microsoft/TypeScript
 * 673a5f17d713 tsc/internal/lsp/lsproto/_generate; only the tinyexec import is
 * dropped). Two custom structure entries follow 3e9236361fa9
 * tools/scripts/lsp/generate.mts: ContentMapperManifest.outputExtensions
 * (ts#64581) and the InitializeAPISessionParams.pipe text (ts#64818). So every
 * type name, union name, literal name,
 * discriminator and try order is the Go one. generateCode below is the
 * rewritten emitter. It keeps Go type strings from resolveType and converts
 * them to Rust types in one place (goTypeToRust). Go marshals structs by
 * reflection (JSON v2 default arshalers); the Rust output has an explicit
 * MarshalerTo per type that follows the v2 default rules, and an explicit
 * UnmarshalerFrom for the structs that Go decodes by reflection.
 */

// Output files, in Go section order. Structures are split 100 per file in
// model order (map-lsproto.md section 4). Root writes lsp_generated/mod.rs.
const outDir = out.replace(/\.go$/, "");
const structuresPerFile = 100;

// Rust keywords (2024 edition, strict and reserved). A field whose snake name
// is a keyword gets a trailing underscore (PORTING.md "Names").
const rustKeywords = new Set([
    "as", "break", "const", "continue", "crate", "else", "enum", "extern", "false", "fn", "for", "if", "impl", "in",
    "let", "loop", "match", "mod", "move", "mut", "pub", "ref", "return", "self", "Self", "static", "struct", "super",
    "trait", "true", "type", "unsafe", "use", "where", "while", "async", "await", "dyn", "abstract", "become", "box",
    "do", "final", "macro", "override", "priv", "typeof", "unsized", "virtual", "yield", "try", "gen",
]);

// snakeCase applies the PORTING.md name rule: insert `_` before each capital
// that follows a lowercase letter or digit, and before the last capital of an
// acronym run followed by a lowercase letter; then lowercase.
function snakeCase(name: string): string {
    const isUpper = (c: string | undefined) => c !== undefined && c >= "A" && c <= "Z";
    const isLower = (c: string | undefined) => c !== undefined && c >= "a" && c <= "z";
    const isDigit = (c: string | undefined) => c !== undefined && c >= "0" && c <= "9";
    let s = "";
    for (let i = 0; i < name.length; i++) {
        const c = name[i];
        if (i > 0 && isUpper(c)) {
            const prev = name[i - 1];
            const next = name[i + 1];
            if (isLower(prev) || isDigit(prev) || (isUpper(prev) && isLower(next))) {
                s += "_";
            }
        }
        s += c.toLowerCase();
    }
    return s;
}

function rustFieldName(goName: string): string {
    const s = snakeCase(goName);
    return rustKeywords.has(s) ? s + "_" : s;
}

function rustConstName(goName: string): string {
    return snakeCase(goName).toUpperCase();
}

// goReflectTypeName returns Go's reflect name of a generated Go type, as the v2
// errors print it: `any` is `interface {}` and lsproto types get the package.
function goReflectTypeName(name: string): string {
    return name.replace(/\bany\b/g, "interface {}").replace(/\b[A-Z]\w*/g, t => `lsproto.${t}`);
}

// rustStr returns a Rust string literal for s.
function rustStr(s: string): string {
    let r = '"';
    for (const ch of s) {
        const code = ch.codePointAt(0)!;
        if (ch === "\\") r += "\\\\";
        else if (ch === '"') r += '\\"';
        else if (ch === "\n") r += "\\n";
        else if (ch === "\r") r += "\\r";
        else if (ch === "\t") r += "\\t";
        else if (code < 0x20 || code === 0x7f) r += `\\u{${code.toString(16)}}`;
        else r += ch;
    }
    return r + '"';
}

// rustByteStr returns a Rust byte string literal for the UTF-8 bytes of s.
function rustByteStr(s: string): string {
    let r = 'b"';
    for (const b of Buffer.from(s, "utf-8")) {
        if (b === 0x5c) r += "\\\\";
        else if (b === 0x22) r += '\\"';
        else if (b >= 0x20 && b < 0x7f) r += String.fromCharCode(b);
        else r += `\\x${b.toString(16).padStart(2, "0")}`;
    }
    return r + '"';
}

// splitGoMapType splits "map[K]V" into K and V.
function splitGoMapType(goType: string): [string, string] {
    let depth = 0;
    for (let i = 3; i < goType.length; i++) {
        if (goType[i] === "[") depth++;
        else if (goType[i] === "]") {
            depth--;
            if (depth === 0) {
                return [goType.slice(4, i), goType.slice(i + 1)];
            }
        }
    }
    throw new Error(`Malformed Go map type: ${goType}`);
}

const goIdentifier = /^[A-Za-z_][A-Za-z0-9_]*$/;

/**
 * goTypeToRust converts a Go type string from resolveType (or a field type
 * built from it) to the Rust type (map-lsproto.md section 3, PORTING.md
 * "Protocol types"). `*T` is Option<T>, `[]T` and `[]*T` are Vec<T>
 * (`[]*T`, also as a map value, is Vec<Option<T>>, and a `*T` map value is
 * Option<T>, in a type the server decodes: fieldGoTypeToRust),
 * `map[K]V` and `map[K]*V` are IndexMap<K, V>, `any` is LspAny, `struct{}`
 * is EmptyObject and `[2]uint32` is [u32; 2]. Named types keep their name.
 * Box for type cycles is added per field by the caller (see boxedEdges).
 */
function goTypeToRust(goType: string): string {
    if (goType.startsWith("*")) {
        return `Option<${goTypeToRust(goType.slice(1))}>`;
    }
    if (goType.startsWith("[]")) {
        return `Vec<${goTypeToRust(goType.slice(2).replace(/^\*/, ""))}>`;
    }
    if (goType === "[2]uint32") {
        return "[u32; 2]";
    }
    if (goType.startsWith("map[")) {
        const [key, value] = splitGoMapType(goType);
        return `IndexMap<${goTypeToRust(key)}, ${goTypeToRust(value.replace(/^\*/, ""))}>`;
    }
    switch (goType) {
        case "int32":
            return "i32";
        case "uint32":
            return "u32";
        case "uint64":
            return "u64";
        case "float64":
            return "f64";
        case "string":
            return "String";
        case "bool":
            return "bool";
        case "any":
            return "LspAny";
        case "struct{}":
            return "EmptyObject";
    }
    if (!goIdentifier.test(goType)) {
        throw new Error(`Unsupported Go type in goTypeToRust: ${goType}`);
    }
    return goType;
}

/**
 * Generate the Rust code. Returns the content of each output file, in Go
 * section order.
 */
function generateCode(): Map<string, string> {
    const files = new Map<string, string[]>();
    let parts: string[] = [];

    function write(s: string) {
        parts.push(s);
    }

    function writeLine(s = "") {
        parts.push(s + "\n");
    }

    function startFile(name: string) {
        parts = [];
        files.set(name, parts);
        writeLine("// Code generated by generate.mts; DO NOT EDIT.");
        writeLine("//");
        writeLine("// Rust port of microsoft/TypeScript tsc/internal/lsp/lsproto/lsp_generated.go");
        writeLine("// (pinned 673a5f17d713), generated from the same meta model by the ported generator.");
        writeLine("// Meta model version " + model.metaData.version);
        writeLine("");
        writeLine("use crate::lsp::lsproto::prelude::*;");
        writeLine("");
    }

    // writeDocumentation writes formatDocumentation output with an indent.
    function writeDocumentation(s: string | undefined, indent: string) {
        const doc = formatDocumentation(s);
        for (const line of doc.split("\n").filter(l => l)) {
            writeLine(`${indent}${line}`);
        }
    }

    // ------------------------------------------------------------------
    // Resolve every type in the order the Go emitter first resolves it, so
    // typeInfo.unionTypes and typeInfo.literalTypes have Go's insertion
    // order before any Rust is written. resolveType is idempotent for types
    // it has already seen, so later calls do not change the order.
    // ------------------------------------------------------------------
    const requestsAndNotifications: (Request | Notification)[] = [...model.requests, ...model.notifications];
    for (const structure of model.structures) {
        for (const prop of structure.properties) {
            resolveType(prop.type);
        }
    }
    for (const request of requestsAndNotifications) {
        if (request.params && !Array.isArray(request.params)) {
            resolveType(request.params);
        }
    }
    for (const request of requestsAndNotifications) {
        if ("result" in request && !(request.result.kind === "base" && request.result.name === "null")) {
            resolveType(request.result);
        }
        if (request.params && !Array.isArray(request.params)) {
            resolveType(request.params);
        }
    }
    for (const alias of customTypeAliases) {
        resolveType(alias.type);
    }
    for (const [, members] of typeInfo.unionTypes.entries()) {
        for (const member of members) {
            resolveType(member.type);
        }
    }

    // ------------------------------------------------------------------
    // Field model shared by structures, RegisterOptions, unions and
    // Resolved structures.
    // ------------------------------------------------------------------
    interface GoField {
        goName: string; // Go field name
        jsonName: string; // JSON member name ("-" = not marshaled)
        goType: string; // Go field type
        omitzero: boolean; // json tag has omitzero
        prop?: Property;
    }

    function structureGoFields(structure: Structure): GoField[] {
        const fields: GoField[] = [];
        for (const prop of structure.properties) {
            const type = resolveType(prop.type);
            // For properties marked with omitzeroValue, use value type with omitzero instead of pointer
            const useOmitzero = !!(prop.optional || prop.omitzeroValue);
            const goType = (prop.optional || type.needsPointer) && !prop.omitzeroValue ? `*${type.name}` : type.name;
            fields.push({ goName: goFieldName(prop), jsonName: prop.name, goType, omitzero: useOmitzero, prop });
        }
        // Special: add RegisterOptions field to Registration
        if (structure.name === "Registration") {
            fields.push({ goName: "RegisterOptions", jsonName: "-", goType: "*RegisterOptions", omitzero: false });
        }
        return fields;
    }

    function registerOptionsGoFields(): GoField[] {
        // Go: no json tags, so the JSON v2 default name is the Go field name.
        return registrationMethods.map(reg => ({
            goName: reg.fieldName,
            jsonName: reg.fieldName,
            goType: `*${reg.optionsTypeName}`,
            omitzero: false,
        }));
    }

    interface UnionEntry {
        fieldName: string;
        typeName: string;
        originalType: Type;
    }

    // unionGoEntries returns the union fields in declaration order, one per
    // distinct Go member type (Go skips repeated member types).
    function unionGoEntries(members: { name: string; type: Type; containedNull: boolean; }[]): UnionEntry[] {
        const uniqueTypeFields = new Map<string, string>(); // Maps type name -> field name
        const uniqueTypeToOriginal = new Map<string, Type>();
        for (const member of members) {
            const type = resolveType(member.type);
            const memberType = type.name;
            if (!uniqueTypeFields.has(memberType)) {
                uniqueTypeFields.set(memberType, titleCase(member.name));
                uniqueTypeToOriginal.set(memberType, member.type);
            }
        }
        return Array.from(uniqueTypeFields.entries()).map(([typeName, fieldName]) => ({
            fieldName,
            typeName,
            originalType: uniqueTypeToOriginal.get(typeName)!,
        }));
    }

    function unionGoFields(members: { name: string; type: Type; containedNull: boolean; }[]): GoField[] {
        return unionGoEntries(members).map(e => ({
            goName: e.fieldName,
            jsonName: "-",
            goType: `*${e.typeName}`,
            omitzero: false,
        }));
    }

    // Resolved structures (generateResolvedStruct): every field is a value
    // with omitzero; structure references become Resolved<Name>.
    function resolvedGoFields(structure: Structure): GoField[] {
        const fields: GoField[] = [];
        for (const prop of structure.properties) {
            const type = resolveType(prop.type);
            let goType = type.name;
            if (prop.type.kind === "reference" && model.structures.find(s => s.name === type.name)) {
                goType = `Resolved${type.name}`;
            }
            fields.push({ goName: goFieldName(prop), jsonName: prop.name, goType, omitzero: true, prop });
        }
        return fields;
    }

    function collectStructureDependencies(structure: Structure, visited = new Set<string>()): Structure[] {
        if (visited.has(structure.name)) {
            return [];
        }
        visited.add(structure.name);

        const deps: Structure[] = [];

        for (const prop of structure.properties) {
            if (prop.type.kind === "reference") {
                const refStructure = model.structures.find(s => s.name === (prop.type as ReferenceType).name);
                if (refStructure) {
                    deps.push(...collectStructureDependencies(refStructure, new Set(visited)));
                    deps.push(refStructure);
                }
            }
        }

        return deps;
    }

    const clientCapsStructure = model.structures.find(s => s.name === "ClientCapabilities");
    const resolvedStructures: { structure: Structure; isMain: boolean; }[] = [];
    if (clientCapsStructure) {
        const deps = collectStructureDependencies(clientCapsStructure);
        const uniqueDeps = Array.from(new Map(deps.map(d => [d.name, d])).values());
        for (const dep of uniqueDeps) {
            resolvedStructures.push({ structure: dep, isMain: false });
        }
        resolvedStructures.push({ structure: clientCapsStructure, isMain: true });
    }

    // All struct-shaped Rust types and their Go fields.
    const typeFields = new Map<string, GoField[]>();
    for (const structure of model.structures) {
        typeFields.set(structure.name, structureGoFields(structure));
        if (structure.name === "Registration") {
            typeFields.set("RegisterOptions", registerOptionsGoFields());
        }
    }
    for (const [name, members] of typeInfo.unionTypes.entries()) {
        typeFields.set(name, unionGoFields(members));
    }
    for (const { structure } of resolvedStructures) {
        typeFields.set(`Resolved${structure.name}`, resolvedGoFields(structure));
    }

    // ------------------------------------------------------------------
    // Nil list elements. Go decodes a JSON null element of a `[]*T` as a
    // nil pointer, and code that reads it panics. The port keeps `[]*T` as
    // `Vec<T>`, because it never builds a nil element, except in the types
    // that the server decodes: the params of client-to-server methods and
    // the results of server-to-client requests, with the types they hold.
    // There a `[]*T` (also as a map value) is `Vec<Option<T>>`, so a null
    // element decodes as Go's nil, and a nil element encodes as null, as in
    // Go (a type such as CompletionItem goes back to the client). A list
    // that holds no null encodes and decodes as before (PORTING.md
    // "Protocol types").
    // ------------------------------------------------------------------
    function namedGoTypes(goType: string): string[] {
        if (goType.startsWith("*")) return namedGoTypes(goType.slice(1));
        if (goType.startsWith("[]")) return namedGoTypes(goType.slice(2));
        if (goType.startsWith("map[")) {
            const [key, value] = splitGoMapType(goType);
            return [...namedGoTypes(key), ...namedGoTypes(value)];
        }
        return typeFields.has(goType) ? [goType] : [];
    }

    function typeClosure(roots: Iterable<string>): Set<string> {
        const seen = new Set<string>();
        const todo = [...roots];
        while (todo.length > 0) {
            const name = todo.pop()!;
            if (seen.has(name)) continue;
            seen.add(name);
            for (const field of typeFields.get(name)!) {
                todo.push(...namedGoTypes(field.goType));
            }
        }
        return seen;
    }

    const decodedTypes = (() => {
        const decoded: string[] = [];
        for (const method of requestsAndNotifications) {
            const toServer = method.messageDirection !== "serverToClient";
            const toClient = method.messageDirection !== "clientToServer";
            if (toServer && method.params && !Array.isArray(method.params)) {
                decoded.push(...namedGoTypes(resolveType(method.params).name));
            }
            if (toClient && "result" in method) {
                decoded.push(...namedGoTypes(resolveType(method.result).name));
            }
        }
        return typeClosure(decoded);
    })();

    // fieldGoTypeToRust is goTypeToRust for a field of `owner`, with the
    // nil elements of a type the server decodes: a `[]*T` field, or a map
    // field whose values are `[]*T` (WorkspaceEdit.changes) or `*T`
    // (WorkspaceEdit.changeAnnotations: Go's pointer arshaler takes a null
    // value as nil).
    function fieldGoTypeToRust(owner: string, goType: string): string {
        const field = /^(\*?)(.*)$/.exec(goType)!;
        const nilElements = (list: string) => {
            const element = /^\[\]\*(.*)$/.exec(list);
            return element ? `Vec<Option<${goTypeToRust(element[1])}>>` : undefined;
        };
        let rust = decodedTypes.has(owner) ? nilElements(field[2]) : undefined;
        if (!rust && decodedTypes.has(owner) && field[2].startsWith("map[")) {
            const [key, value] = splitGoMapType(field[2]);
            const values = nilElements(value) ?? (value.startsWith("*") ? goTypeToRust(value) : undefined);
            if (values) rust = `IndexMap<${goTypeToRust(key)}, ${values}>`;
        }
        if (!rust) return goTypeToRust(goType);
        return field[1] ? `Option<${rust}>` : rust;
    }

    const enumKinds = new Map(model.enumerations.map(e => [e.name, e.type.name === "string" ? "string" : "int"]));
    const literalNames = new Set(typeInfo.literalTypes.values());

    // ------------------------------------------------------------------
    // Type cycles. A Rust struct cannot hold itself by value, so a field
    // that closes a cycle of by-value containment (Option<T> or T, not
    // behind Vec or IndexMap) gets Box (map-lsproto.md section 3).
    // ------------------------------------------------------------------
    function byValueTarget(goType: string): string | undefined {
        const t = goType.replace(/^\*/, "");
        return typeFields.has(t) ? t : undefined;
    }

    const boxedEdges = new Set<string>(); // "Type.GoField"
    {
        const state = new Map<string, "visiting" | "done">();
        const visit = (name: string) => {
            state.set(name, "visiting");
            for (const field of typeFields.get(name)!) {
                const target = byValueTarget(field.goType);
                if (!target) continue;
                const s = state.get(target);
                if (s === "visiting") {
                    boxedEdges.add(`${name}.${field.goName}`);
                }
                else if (s === undefined) {
                    visit(target);
                }
            }
            state.set(name, "done");
        };
        for (const name of typeFields.keys()) {
            if (!state.has(name)) visit(name);
        }
    }

    function isBoxed(owner: string, field: GoField): boolean {
        return boxedEdges.has(`${owner}.${field.goName}`);
    }

    function rustFieldType(owner: string, field: GoField): string {
        const rust = fieldGoTypeToRust(owner, field.goType);
        if (!isBoxed(owner, field)) return rust;
        if (field.goType.startsWith("*")) return `Option<Box<${goTypeToRust(field.goType.slice(1))}>>`;
        return `Box<${rust}>`;
    }

    // ------------------------------------------------------------------
    // Derives. Every type derives Clone, Debug, Default and PartialEq.
    // PORT: Go structs, enums and literals are comparable (and usable as map
    // keys) when all their fields are; ls code keys maps by Location, Range
    // and enum values. So a type also derives Copy, Eq and Hash when every
    // field allows it, and enums also derive PartialOrd and Ord.
    // ------------------------------------------------------------------
    interface Traits {
        copy: boolean;
        eq: boolean;
        hash: boolean;
    }
    const allTraits: Traits = { copy: true, eq: true, hash: true };
    const namedTraits = new Map<string, Traits>();
    for (const name of typeFields.keys()) namedTraits.set(name, { ...allTraits });

    function goTypeTraits(goType: string): Traits {
        if (goType.startsWith("*")) return goTypeTraits(goType.slice(1));
        if (goType.startsWith("[]")) {
            const t = goTypeTraits(goType.slice(2).replace(/^\*/, ""));
            return { copy: false, eq: t.eq, hash: t.hash };
        }
        if (goType === "[2]uint32" || goType === "struct{}") return { ...allTraits };
        if (goType.startsWith("map[")) {
            const [key, value] = splitGoMapType(goType);
            const k = goTypeTraits(key);
            const v = goTypeTraits(value.replace(/^\*/, ""));
            return { copy: false, eq: k.eq && k.hash && v.eq, hash: false };
        }
        switch (goType) {
            case "int32":
            case "uint32":
            case "uint64":
            case "bool":
                return { ...allTraits };
            case "float64":
                return { copy: true, eq: false, hash: false };
            case "string":
            case "DocumentUri":
            case "URI":
                return { copy: false, eq: true, hash: true };
            case "any":
                return { copy: false, eq: false, hash: false };
        }
        const kind = enumKinds.get(goType);
        if (kind === "string") return { copy: false, eq: true, hash: true };
        if (kind === "int") return { ...allTraits };
        if (literalNames.has(goType)) return { ...allTraits };
        const named = namedTraits.get(goType);
        if (named) return named;
        if (goType === "TelemetryEvent") {
            return goTypeTraits(resolveType(customTypeAliases.find(a => a.name === "TelemetryEvent")!.type).name);
        }
        throw new Error(`No traits for Go type ${goType}`);
    }

    for (let changed = true; changed;) {
        changed = false;
        for (const [name, fields] of typeFields) {
            const cur = namedTraits.get(name)!;
            const next = { ...allTraits };
            for (const field of fields) {
                const t = goTypeTraits(field.goType);
                next.copy &&= t.copy && !isBoxed(name, field);
                next.eq &&= t.eq;
                next.hash &&= t.hash;
            }
            if (next.copy !== cur.copy || next.eq !== cur.eq || next.hash !== cur.hash) {
                namedTraits.set(name, next);
                changed = true;
            }
        }
    }

    function deriveLine(t: Traits, ordered = false): string {
        const d = ["Clone"];
        if (t.copy) d.push("Copy");
        d.push("Debug", "Default", "PartialEq");
        if (t.eq) d.push("Eq");
        if (t.hash) d.push("Hash");
        if (ordered) d.push("PartialOrd", "Ord");
        return `#[derive(${d.join(", ")})]`;
    }

    // copyOrClone reads a field by value (Go copies values).
    function copyOrClone(expr: string, goType: string): string {
        return goTypeTraits(goType).copy ? expr : `${expr}.clone()`;
    }

    // ------------------------------------------------------------------
    // Shared emitters.
    // ------------------------------------------------------------------
    interface RustField extends GoField {
        rustName: string;
        rustType: string;
    }

    function rustFields(owner: string, fields: GoField[]): RustField[] {
        const seen = new Set<string>();
        return fields.map(f => {
            const rustName = rustFieldName(f.goName);
            if (seen.has(rustName)) {
                throw new Error(`Rust field name collision in ${owner}: ${rustName}`);
            }
            seen.add(rustName);
            return { ...f, rustName, rustType: rustFieldType(owner, f) };
        });
    }

    // zeroExpr is the Go reflect IsZero test for a field value.
    function zeroExpr(expr: string, rustType: string): string {
        if (rustType.startsWith("Option<")) return `${expr}.is_none()`;
        if (rustType.startsWith("Vec<") || rustType.startsWith("IndexMap<") || rustType === "String") return `${expr}.is_empty()`;
        if (rustType === "bool") return `!${expr}`;
        if (rustType === "i32" || rustType === "u32" || rustType === "u64") return `${expr} == 0`;
        // PORT: Go reflect IsZero is true only for +0.0.
        if (rustType === "f64") return `${expr}.to_bits() == 0`;
        if (rustType === "[u32; 2]") return `${expr} == [0, 0]`;
        if (rustType === "DocumentUri" || rustType === "URI") return `${expr}.0.is_empty()`;
        return `${expr}.is_zero()`;
    }

    function writeIsZero(typeName: string, fields: RustField[], comment: string) {
        writeLine(`impl IsZero for ${typeName} {`);
        writeLine(`    // ${comment}`);
        writeLine(`    fn is_zero(&self) -> bool {`);
        if (fields.length === 0) {
            writeLine(`        true`);
        }
        else {
            writeLine(`        ${fields.map(f => zeroExpr(`self.${f.rustName}`, f.rustType)).join("\n            && ")}`);
        }
        writeLine(`    }`);
        writeLine(`}`);
        writeLine("");
    }

    // marshalFieldStmt writes one struct member with the JSON v2 default
    // rules: omitzero skips the zero value; a non-omitzero nil pointer
    // writes null; a non-omitzero slice or map always writes [] or {}.
    function marshalFieldStmt(f: RustField): string {
        const name = rustStr(f.jsonName);
        const value = `&self.${f.rustName}`;
        if (!f.omitzero) {
            return `marshal_field(enc, &mut first, ${name}, ${value})?;`;
        }
        if (f.rustType.startsWith("Option<")) {
            return `marshal_opt_field(enc, &mut first, ${name}, ${value})?;`;
        }
        if (f.rustType === "DocumentUri" || f.rustType === "URI") {
            return `if !(${zeroExpr(`self.${f.rustName}`, f.rustType)}) {\n            marshal_field(enc, &mut first, ${name}, ${value})?;\n        }`;
        }
        return `marshal_field_omitzero(enc, &mut first, ${name}, ${value})?;`;
    }

    function writeReflectMarshal(typeName: string, fields: RustField[]) {
        const members = fields.filter(f => f.jsonName !== "-");
        writeLine(`impl MarshalerTo for ${typeName} {`);
        writeLine(`    // PORT: Go marshals ${typeName} with the JSON v2 default struct arshaler.`);
        writeLine(`    fn marshal_json_to(&self, enc: &mut String) -> Result<(), JsonError> {`);
        writeLine(`        write_object_start(enc);`);
        if (members.length > 0) {
            writeLine(`        let mut first = true;`);
        }
        for (const f of members) {
            writeLine(`        ${marshalFieldStmt(f)}`);
        }
        writeLine(`        write_object_end(enc);`);
        writeLine(`        Ok(())`);
        writeLine(`    }`);
        writeLine(`}`);
        writeLine("");
    }

    // writeReflectUnmarshal is the JSON v2 default struct unmarshaler for a
    // type that has no generated UnmarshalJSONFrom in Go: null sets the zero
    // value, an object sets fields by exact member name and skips unknown
    // names, and any other kind is an error.
    function writeReflectUnmarshal(typeName: string, fields: RustField[]) {
        const members = fields.filter(f => f.jsonName !== "-");
        writeLine(`impl UnmarshalerFrom for ${typeName} {`);
        writeLine(`    // PORT: Go has no UnmarshalJSONFrom for ${typeName}; this is the JSON v2`);
        writeLine(`    // default struct arshaler.`);
        writeLine(`    fn unmarshal_json_from(&mut self, dec: &mut JsonDecoder<'_>) -> Result<(), JsonError> {`);
        writeLine(`        match dec.peek_kind() {`);
        writeLine(`            b'n' => {`);
        writeLine(`                dec.read_token()?;`);
        writeLine(`                *self = ${typeName}::default();`);
        writeLine(`                Ok(())`);
        writeLine(`            }`);
        writeLine(`            b'{' => {`);
        writeLine(`                dec.read_token()?;`);
        writeLine(`                while dec.peek_kind() != b'}' {`);
        writeLine(`                    let mut name = String::new();`);
        writeLine(`                    json_unmarshal_decode(dec, &mut name)?;`);
        if (members.length === 0) {
            writeLine(`                    dec.skip_value()?;`);
        }
        else {
            writeLine(`                    match name.as_str() {`);
            for (const f of members) {
                writeLine(`                        ${rustStr(f.jsonName)} => json_unmarshal_decode(dec, &mut self.${f.rustName})?,`);
            }
            writeLine(`                        _ => dec.skip_value()?,`);
            writeLine(`                    }`);
        }
        writeLine(`                }`);
        writeLine(`                dec.read_token()?;`);
        writeLine(`                Ok(())`);
        writeLine(`            }`);
        writeLine(`            _ => {`);
        writeLine(`                dec.skip_value()?;`);
        writeLine(`                Err(JsonError {`);
        writeLine(`                    message: ${rustStr(`cannot unmarshal JSON value into Go lsproto.${typeName}`)}.to_string(),`);
        writeLine(`                })`);
        writeLine(`            }`);
        writeLine(`        }`);
        writeLine(`    }`);
        writeLine(`}`);
        writeLine("");
    }

    function writeStructDefinition(typeName: string, fields: RustField[], includeDocumentation: boolean) {
        writeLine(deriveLine(namedTraits.get(typeName)!));
        if (fields.length === 0) {
            writeLine(`pub struct ${typeName} {}`);
            writeLine("");
            return;
        }
        writeLine(`pub struct ${typeName} {`);
        for (const f of fields) {
            if (includeDocumentation && f.prop) {
                writeDocumentation(f.prop.documentation, "    ");
            }
            if (f.goName === "RegisterOptions" && typeName === "Registration") {
                writeLine("");
                writeLine(`    // Options necessary for the registration. Determines the method.`);
                writeLine(`    // Go: json:"-" (not marshaled by reflection).`);
            }
            writeLine(`    pub ${f.rustName}: ${f.rustType},`);
            if (includeDocumentation && f.prop) {
                writeLine("");
            }
        }
        writeLine("}");
        writeLine("");
    }

    /**
     * Generate Rust code for discriminator-based union dispatch.
     * Assumes a variable named `data` (the raw JSON value) is in scope.
     * Returns true if all match arms return (exhaustive).
     */
    function generateDiscriminatorDispatch(
        disc: NonNullable<ReturnType<typeof findDiscriminatorField>>,
        indent: string,
    ): boolean {
        writeLine(`${indent}match json_object_raw_field(data, ${rustStr(disc.fieldName)}).0.as_slice() {`);
        for (const [value, entry] of disc.mapping) {
            writeLine(`${indent}    ${rustByteStr(`"${value}"`)} => {`);
            writeLine(`${indent}        let v = self.${rustFieldName(entry.fieldName)}.insert(Default::default());`);
            writeLine(`${indent}        return unmarshal_read_value(data, ${derefBoxed(entry)});`);
            writeLine(`${indent}    }`);
        }
        let exhaustive = false;
        if (disc.unmapped.length > 0) {
            writeLine(`${indent}    _ => {`);
            exhaustive = generateUnmappedFallback(disc.unmapped, indent + "        ");
            writeLine(`${indent}    }`);
        }
        else {
            writeLine(`${indent}    _ => {}`);
        }
        writeLine(`${indent}}`);
        return exhaustive;
    }

    /**
     * Generate streaming discriminator dispatch for unions with at most one
     * unmapped fallback arm. Fields after the discriminator decode directly
     * from the active decoder; fields before it are replayed by the helper.
     *
     * PORT: Go (#63934) emits `scanDiscriminatedStruct` and
     * `unmarshalDiscriminatedArm`; the Rust helpers are in lsp.rs.
     */
    function generateStreamingDiscriminatorDispatch(
        name: string,
        disc: NonNullable<ReturnType<typeof findDiscriminatorField>>,
        indent: string,
    ) {
        writeLine(`${indent}let state = scan_discriminated_struct(dec, ${rustStr(name)}, ${rustStr(disc.fieldName)})?;`);
        writeLine(`${indent}match state.discriminator_value.as_slice() {`);
        for (const [value, entry] of disc.mapping) {
            writeLine(`${indent}    ${rustByteStr(`"${value}"`)} => {`);
            writeStreamingArm(entry, indent + "        ");
            writeLine(`${indent}    }`);
        }
        writeLine(`${indent}    _ => {`);
        if (disc.unmapped.length === 1) {
            writeStreamingArm(disc.unmapped[0], indent + "        ");
        }
        else {
            writeLine(`${indent}        return Err(state.invalid_discriminator());`);
        }
        writeLine(`${indent}    }`);
        writeLine(`${indent}}`);
    }

    // Go: return unmarshalDiscriminatedArm(state, &o.X)
    function writeStreamingArm(entry: UnionEntry, indent: string) {
        writeLine(`${indent}let v = unmarshal_discriminated_arm(&state, dec)?;`);
        writeLine(`${indent}self.${rustFieldName(entry.fieldName)} = Some(${entryIsBoxed(entry) ? "Box::new(v)" : "v"});`);
        writeLine(`${indent}return Ok(());`);
    }

    function canStreamDiscriminator(
        disc: NonNullable<ReturnType<typeof findDiscriminatorField>>,
    ): boolean {
        if (disc.unmapped.length > 1) {
            return false;
        }
        const entries = [...disc.mapping.values(), ...disc.unmapped];
        return entries.every(entry => {
            if (entry.originalType.kind !== "reference") {
                return false;
            }
            const name = entry.originalType.name;
            return !hasCustomStructureCodec(name) && model.structures.some(structure => structure.name === name);
        });
    }

    /**
     * Generate try-each fallback code for unmapped entries, chaining into
     * presence dispatch if possible before falling back to raw try-each.
     * Assumes a variable named `data` is in scope.
     * Returns true if all generated paths return (exhaustive).
     */
    function generateUnmappedFallback(
        unmapped: UnionEntry[],
        indent: string,
    ): boolean {
        if (unmapped.length <= 1) {
            // Exactly 1 entry: it's the only remaining variant after dispatch,
            // so use a hard error return instead of speculative err == nil.
            for (const entry of unmapped) {
                writeLine(`${indent}let v = self.${rustFieldName(entry.fieldName)}.insert(Default::default());`);
                writeLine(`${indent}return unmarshal_read_value(data, ${derefBoxed(entry)});`);
            }
            return unmapped.length === 1;
        }
        // Try chaining presence dispatch on the remaining subset
        const pres = findPresenceDiscriminator(unmapped);
        if (pres) {
            return generatePresenceDispatch(pres, indent);
        }
        else {
            for (const entry of unmapped) {
                writeTryEach(entry, indent);
            }
            return false;
        }
    }

    /**
     * Iteratively collect all presence discriminator checks across multiple
     * passes, so they can be emitted as a single flat switch with one scan.
     */
    function collectAllPresenceChecks(
        pres: NonNullable<ReturnType<typeof findPresenceDiscriminator>>,
    ): {
        allChecks: { jsonFieldName: string; entry: UnionEntry; }[];
        finalUnmapped: UnionEntry[];
    } {
        const allChecks = [...pres.checks];
        let remaining = pres.unmapped;
        while (remaining.length > 1) {
            const next = findPresenceDiscriminator(remaining);
            if (!next) break;
            allChecks.push(...next.checks);
            remaining = next.unmapped;
        }
        return { allChecks, finalUnmapped: remaining };
    }

    /**
     * Generate Rust code for presence-based union dispatch.
     * Assumes a variable named `data` is in scope.
     * Collects all presence checks iteratively, then emits a single flat
     * match on json_object_has_key(data, &[key1, key2, ...]) so data is scanned once.
     * Returns true if all match arms return (exhaustive).
     */
    function generatePresenceDispatch(
        pres: NonNullable<ReturnType<typeof findPresenceDiscriminator>>,
        indent: string,
    ): boolean {
        const { allChecks, finalUnmapped } = collectAllPresenceChecks(pres);
        const args = allChecks.map(c => rustStr(c.jsonFieldName)).join(", ");
        writeLine(`${indent}match json_object_has_key(data, &[${args}]) {`);
        for (let i = 0; i < allChecks.length; i++) {
            writeLine(`${indent}    ${i} => {`);
            writeLine(`${indent}        // ${allChecks[i].jsonFieldName}`);
            writeLine(`${indent}        let v = self.${rustFieldName(allChecks[i].entry.fieldName)}.insert(Default::default());`);
            writeLine(`${indent}        return unmarshal_read_value(data, ${derefBoxed(allChecks[i].entry)});`);
            writeLine(`${indent}    }`);
        }
        if (finalUnmapped.length > 0) {
            writeLine(`${indent}    _ => {`);
            if (finalUnmapped.length === 1) {
                // Only one variant left after dispatch — use hard error return.
                const entry = finalUnmapped[0];
                writeLine(`${indent}        let v = self.${rustFieldName(entry.fieldName)}.insert(Default::default());`);
                writeLine(`${indent}        return unmarshal_read_value(data, ${derefBoxed(entry)});`);
            }
            else {
                for (const entry of finalUnmapped) {
                    writeTryEach(entry, indent + "        ");
                }
            }
            writeLine(`${indent}    }`);
        }
        else {
            writeLine(`${indent}    _ => {}`);
        }
        writeLine(`${indent}}`);
        // Exhaustive if the default case has a single hard-returning entry
        return finalUnmapped.length === 1;
    }

    // The union type whose unmarshal code is being written (for Box checks).
    let currentUnion = "";

    function entryIsBoxed(entry: UnionEntry): boolean {
        return boxedEdges.has(`${currentUnion}.${entry.fieldName}`);
    }

    // derefBoxed is the `&mut T` to decode into, given `let v = field.insert(..)`.
    function derefBoxed(entry: UnionEntry): string {
        return entryIsBoxed(entry) ? "&mut **v" : "v";
    }

    // Go: var vX T; if err := json.Unmarshal(data, &vX); err == nil { o.X = &vX; return nil }
    function writeTryEach(entry: UnionEntry, indent: string) {
        const field = rustFieldName(entry.fieldName);
        const rustType = fieldGoTypeToRust(currentUnion, entry.typeName);
        writeLine(`${indent}let mut v_${field}: ${rustType} = Default::default();`);
        writeLine(`${indent}if json_unmarshal(data, &mut v_${field}, &[]).is_ok() {`);
        writeLine(`${indent}    self.${field} = Some(${entryIsBoxed(entry) ? `Box::new(v_${field})` : `v_${field}`});`);
        writeLine(`${indent}    return Ok(());`);
        writeLine(`${indent}}`);
    }

    // ------------------------------------------------------------------
    // Structures
    // ------------------------------------------------------------------
    const reflectionStructs: string[] = [];

    model.structures.forEach((structure, index) => {
        if (index % structuresPerFile === 0) {
            startFile(`structures_p${index / structuresPerFile + 1}.rs`);
            writeLine("// Structures");
            writeLine("");
        }

        const fields = rustFields(structure.name, structureGoFields(structure));

        write(formatDocumentation(structure.documentation));
        writeStructDefinition(structure.name, fields, true);

        if (hasTextDocumentURI(structure)) {
            // Generate TextDocumentURI method
            const textDocProp = structure.properties?.find(p => (p.name === "textDocument" || p.name === "_vs_textDocument") && p.type.kind === "reference" && p.type.name === "TextDocumentIdentifier");
            const textDocFieldName = textDocProp ? goFieldName(textDocProp) : "TextDocument";
            writeLine(`impl HasTextDocumentURI for ${structure.name} {`);
            writeLine(`    // Go: (s *${structure.name}) TextDocumentURI`);
            writeLine(`    fn text_document_uri(&self) -> DocumentUri {`);
            writeLine(`        self.${rustFieldName(textDocFieldName)}.uri.clone()`);
            writeLine(`    }`);
            writeLine(`}`);
            writeLine("");

            if (hasTextDocumentPosition(structure)) {
                // Generate TextDocumentPosition method
                const posProp = structure.properties?.find(p => (p.name === "position" || p.name === "_vs_position") && p.type.kind === "reference" && p.type.name === "Position");
                const posFieldName = posProp ? goFieldName(posProp) : "Position";
                writeLine(`impl HasTextDocumentPosition for ${structure.name} {`);
                writeLine(`    // Go: (s *${structure.name}) TextDocumentPosition`);
                writeLine(`    fn text_document_position(&self) -> Position {`);
                writeLine(`        ${copyOrClone(`self.${rustFieldName(posFieldName)}`, "Position")}`);
                writeLine(`    }`);
                writeLine(`}`);
                writeLine("");
            }
        }

        const locationUriProperty = getLocationUriProperty(structure);
        if (locationUriProperty) {
            // Generate Location method
            writeLine(`impl HasLocation for ${structure.name} {`);
            writeLine(`    // Go: (s ${structure.name}) GetLocation`);
            writeLine(`    fn get_location(&self) -> Location {`);
            if (locationUriProperty === "Uri" && structure.name === "Location") {
                writeLine(`        ${copyOrClone("self", "Location").replace(/^self$/, "*self")}`);
            }
            else {
                writeLine(`        Location {`);
                writeLine(`            uri: self.${rustFieldName(locationUriProperty)}.clone(),`);
                writeLine(`            range: ${copyOrClone(`self.${rustFieldName(locationUriProperty.replace(/Uri$/, "Range"))}`, "Range")},`);
                writeLine(`        }`);
            }
            writeLine(`    }`);
            writeLine(`}`);
            writeLine("");
        }

        // Generate UnmarshalJSONFrom method for structure validation
        // Skip Registration (has custom marshal/unmarshal generated separately)
        // Skip properties marked with omitzeroValue since they're optional by nature
        const requiredProps = structure.properties?.filter(p => {
            if (p.optional) return false;
            if (p.omitzeroValue) return false;
            return true;
        }) || [];
        // Check if any fields need null rejection
        const hasNullRejectableFields = structure.properties?.some(p => {
            if (p.omitzeroValue) return false;
            if (typeCanBeNull(p.type)) return false;
            const resolved = resolveType(p.type);
            return p.optional || resolved.needsPointer || resolved.name.startsWith("[]") || resolved.name.startsWith("map[");
        }) || false;
        if ((requiredProps.length > 0 || hasNullRejectableFields) && !hasCustomStructureCodec(structure.name)) {
            writeLine(`impl UnmarshalerFrom for ${structure.name} {`);
            writeLine(`    // Go: (s *${structure.name}) UnmarshalJSONFrom`);
            writeLine(`    fn unmarshal_json_from(&mut self, dec: &mut JsonDecoder<'_>) -> Result<(), JsonError> {`);
            if (requiredProps.length > 0) {
                for (let i = 0; i < requiredProps.length; i++) {
                    const prop = requiredProps[i];
                    writeLine(`        const MISSING_${rustConstName(goFieldName(prop))}: u64 = 1 << ${i};`);
                }
                writeLine(`        const _MISSING_LAST: u64 = 1 << ${requiredProps.length};`);
                writeLine(`        let mut missing = _MISSING_LAST - 1;`);
                writeLine("");
            }

            writeLine(`        let k = dec.peek_kind();`);
            writeLine(`        if k != b'{' {`);
            writeLine(`            return Err(err_not_object(k));`);
            writeLine(`        }`);
            writeLine(`        dec.read_token()?;`);
            writeLine("");

            writeLine(`        while dec.peek_kind() != b'}' {`);
            writeLine(`            let name = dec.read_value()?;`);
            writeLine(`            match name {`);

            for (const prop of structure.properties) {
                writeLine(`                ${rustByteStr(`"${prop.name}"`)} => {`);
                if (!prop.optional && !prop.omitzeroValue) {
                    writeLine(`                    missing &= !MISSING_${rustConstName(goFieldName(prop))};`);
                }
                // Reject null for fields whose types cannot represent null but whose Go types
                // silently accept it (pointers, slices, maps).
                const resolvedType = resolveType(prop.type);
                const goTypeAcceptsNull = (prop.optional || resolvedType.needsPointer || resolvedType.name.startsWith("[]") || resolvedType.name.startsWith("map[")) && !prop.omitzeroValue;
                if (goTypeAcceptsNull && !typeCanBeNull(prop.type)) {
                    writeLine(`                    if dec.peek_kind() == b'n' {`);
                    writeLine(`                        return Err(err_null(${rustStr(prop.name)}));`);
                    writeLine(`                    }`);
                }
                if (resolvedType.name.startsWith("map[string]")) {
                    // PORT: `IndexMap<String, V>` decodes as Go `collections.OrderedMap`; a Go
                    // map field gets the v2 map arshaler's kind error (`unmarshal_go_map`).
                    writeLine(`                    unmarshal_go_map(dec, &mut self.${rustFieldName(goFieldName(prop))}, ${rustStr(goReflectTypeName(resolvedType.name))})?;`);
                }
                else {
                    writeLine(`                    json_unmarshal_decode(dec, &mut self.${rustFieldName(goFieldName(prop))})?;`);
                }
                writeLine(`                }`);
            }

            writeLine(`                _ => {`);
            writeLine(`                    dec.skip_value()?;`);
            writeLine(`                }`);
            writeLine(`            }`);
            writeLine(`        }`);
            writeLine("");

            writeLine(`        dec.read_token()?;`);
            writeLine("");

            if (requiredProps.length > 0) {
                writeLine(`        if missing != 0 {`);
                writeLine(`            let mut missing_props: Vec<String> = Vec::new();`);
                for (const prop of requiredProps) {
                    writeLine(`            if missing & MISSING_${rustConstName(goFieldName(prop))} != 0 {`);
                    writeLine(`                missing_props.push(${rustStr(prop.name)}.to_string());`);
                    writeLine(`            }`);
                }
                writeLine(`            return Err(err_missing(&missing_props));`);
                writeLine(`        }`);
                writeLine("");
            }

            writeLine(`        Ok(())`);
            writeLine(`    }`);
            writeLine(`}`);
            writeLine("");
        }
        else if (structure.name !== "Registration") {
            reflectionStructs.push(structure.name);
            writeReflectUnmarshal(structure.name, fields);
        }

        if (structure.name !== "Registration") {
            writeReflectMarshal(structure.name, fields);
        }
        writeIsZero(structure.name, fields, "PORT: Go reflect.Value.IsZero (omitzero).");

        // Generate RegisterOptions struct and custom Registration marshal/unmarshal
        // right after the Registration struct definition.
        if (structure.name === "Registration") {
            const regFields = rustFields("RegisterOptions", registerOptionsGoFields());

            // RegisterOptions struct
            writeLine(`// RegisterOptions is an externally-tagged union representing the options for a capability registration.`);
            writeLine(`// Exactly one field should be set. The set field determines the method for the registration.`);
            writeStructDefinition("RegisterOptions", regFields, false);

            // MarshalJSONTo for Registration
            writeLine(`impl MarshalerTo for Registration {`);
            writeLine(`    // Go: (s *Registration) MarshalJSONTo`);
            writeLine(`    fn marshal_json_to(&self, enc: &mut String) -> Result<(), JsonError> {`);

            // Assert RegisterOptions is set and exactly one field is set
            writeLine(`        let Some(register_options) = &self.register_options else {`);
            writeLine(`            crate::core::go_panic("RegisterOptions must be set".to_string());`);
            writeLine(`        };`);
            const regParts = regFields.map(r => `bool_to_int(register_options.${r.rustName}.is_some())`);
            writeLine(`        assert_only_one(`);
            writeLine(`            "exactly one element of RegisterOptions should be set",`);
            writeLine(`            ${regParts.join("\n                + ")},`);
            writeLine(`        );`);
            writeLine("");

            writeLine(`        write_object_start(enc);`);
            writeLine(`        let mut first = true;`);
            writeLine(`        marshal_field(enc, &mut first, "id", &self.id)?;`);
            writeLine(`        let mut method: &str = "";`);
            writeLine(`        let mut opts: Option<&dyn MarshalerTo> = None;`);
            regFields.forEach((r, i) => {
                const reg = registrationMethods[i];
                writeLine(`        ${i === 0 ? "if" : "} else if"} let Some(v) = &register_options.${r.rustName} {`);
                writeLine(`            method = ${rustStr(reg.registrationMethod)};`);
                writeLine(`            opts = Some(v);`);
            });
            writeLine(`        }`);
            writeLine(`        // PORT: Go writes the method as a raw JSON string value; the method`);
            writeLine(`        // names need no escaping, so the string marshaler writes the same bytes.`);
            writeLine(`        marshal_field(enc, &mut first, "method", method)?;`);
            writeLine(`        marshal_field(enc, &mut first, "registerOptions", &opts)?;`);
            writeLine(`        write_object_end(enc);`);
            writeLine(`        Ok(())`);
            writeLine(`    }`);
            writeLine(`}`);
            writeLine("");

            // UnmarshalJSONFrom for Registration
            writeLine(`impl UnmarshalerFrom for Registration {`);
            writeLine(`    // Go: (s *Registration) UnmarshalJSONFrom`);
            writeLine(`    fn unmarshal_json_from(&mut self, dec: &mut JsonDecoder<'_>) -> Result<(), JsonError> {`);
            writeLine(`        *self = Registration::default();`);
            writeLine(`        const MISSING_ID: u64 = 1 << 0;`);
            writeLine(`        const MISSING_METHOD: u64 = 1 << 1;`);
            writeLine(`        const _MISSING_LAST: u64 = 1 << 2;`);
            writeLine(`        let mut missing = _MISSING_LAST - 1;`);
            writeLine("");
            writeLine(`        let k = dec.peek_kind();`);
            writeLine(`        if k != b'{' {`);
            writeLine(`            return Err(err_not_object(k));`);
            writeLine(`        }`);
            writeLine(`        dec.read_token()?;`);
            writeLine("");
            writeLine(`        let mut method = String::new();`);
            writeLine(`        let mut raw_register_options: &[u8] = &[];`);
            writeLine("");
            writeLine(`        while dec.peek_kind() != b'}' {`);
            writeLine(`            let name = dec.read_value()?;`);
            writeLine(`            match name {`);
            writeLine(`                b"\\"id\\"" => {`);
            writeLine(`                    missing &= !MISSING_ID;`);
            writeLine(`                    json_unmarshal_decode(dec, &mut self.id)?;`);
            writeLine(`                }`);
            writeLine(`                b"\\"method\\"" => {`);
            writeLine(`                    missing &= !MISSING_METHOD;`);
            writeLine(`                    json_unmarshal_decode(dec, &mut method)?;`);
            writeLine(`                }`);
            writeLine(`                b"\\"registerOptions\\"" => {`);
            writeLine(`                    let v = dec.read_value()?;`);
            writeLine(`                    raw_register_options = v;`);
            writeLine(`                }`);
            writeLine(`                _ => {`);
            writeLine(`                    dec.skip_value()?;`);
            writeLine(`                }`);
            writeLine(`            }`);
            writeLine(`        }`);
            writeLine("");
            writeLine(`        dec.read_token()?;`);
            writeLine("");
            writeLine(`        if missing != 0 {`);
            writeLine(`            let mut missing_props: Vec<String> = Vec::new();`);
            writeLine(`            if missing & MISSING_ID != 0 {`);
            writeLine(`                missing_props.push("id".to_string());`);
            writeLine(`            }`);
            writeLine(`            if missing & MISSING_METHOD != 0 {`);
            writeLine(`                missing_props.push("method".to_string());`);
            writeLine(`            }`);
            writeLine(`            return Err(err_missing(&missing_props));`);
            writeLine(`        }`);
            writeLine("");
            writeLine(`        if !raw_register_options.is_empty() {`);
            writeLine(`            let register_options = self.register_options.insert(RegisterOptions::default());`);
            writeLine(`            // Go: switch Method(method) { case MethodX: ... }`);
            writeLine(`            let m = Method(Cow::Owned(method.clone()));`);
            regFields.forEach((r, i) => {
                const reg = registrationMethods[i];
                writeLine(`            ${i === 0 ? "if" : "} else if"} m == Method::${rustConstName(reg.fieldName)} {`);
                writeLine(`                let mut v = ${reg.optionsTypeName}::default();`);
                writeLine(`                json_unmarshal(raw_register_options, &mut v, &[])?;`);
                writeLine(`                register_options.${r.rustName} = Some(v);`);
            });
            writeLine(`            } else {`);
            writeLine(`                return Err(JsonError {`);
            writeLine(`                    message: format!("unknown registration method: {}", method),`);
            writeLine(`                });`);
            writeLine(`            }`);
            writeLine(`        } else {`);
            writeLine(`            return Err(JsonError {`);
            writeLine(`                message: format!("missing registerOptions for method: {}", method),`);
            writeLine(`            });`);
            writeLine(`        }`);
            writeLine("");
            writeLine(`        Ok(())`);
            writeLine(`    }`);
            writeLine(`}`);
            writeLine("");

            // PORT: Go never marshals or decodes RegisterOptions alone
            // (Registration does); these are the JSON v2 default arshalers.
            reflectionStructs.push("RegisterOptions");
            writeReflectUnmarshal("RegisterOptions", regFields);
            writeReflectMarshal("RegisterOptions", regFields);
            writeIsZero("RegisterOptions", regFields, "PORT: Go reflect.Value.IsZero (omitzero).");
        }

        if (compareStructures.has(structure.name)) {
            generateCompareMethod(structure);
        }
    });

    function generateCompareMethod(structure: Structure) {
        const props = structure.properties ?? [];
        writeLine(`impl ${structure.name} {`);
        writeLine(`    // Go: (s *${structure.name}) Compare`);
        writeLine(`    pub fn compare(&self, other: &${structure.name}) -> i32 {`);
        for (let i = 0; i < props.length; i++) {
            const prop = props[i];
            const isLast = i === props.length - 1;
            const fieldName = goFieldName(prop);
            const expr = compareExpressionForProperty(structure.name, prop, fieldName);
            if (isLast) {
                writeLine(`        ${expr}`);
            }
            else {
                writeLine(`        let c = ${expr};`);
                writeLine(`        if c != 0 {`);
                writeLine(`            return c;`);
                writeLine(`        }`);
            }
        }
        writeLine(`    }`);
        writeLine(`}`);
        writeLine("");
    }

    function compareExpressionForProperty(structName: string, prop: Property, fieldName: string): string {
        const resolved = resolveType(prop.type);
        const isPointerField = (prop.optional || resolved.needsPointer) && !prop.omitzeroValue;
        const field = rustFieldName(fieldName);

        if (prop.type.kind === "reference") {
            const refName = prop.type.name;
            if (compareStructures.has(refName)) {
                if (isPointerField) {
                    // PORT: Go calls Compare on a nil pointer and panics.
                    return `self.${field}.as_ref().unwrap().compare(other.${field}.as_ref().unwrap())`;
                }
                return `self.${field}.compare(&other.${field})`;
            }
        }

        if (prop.type.kind === "base") {
            switch (prop.type.name) {
                case "string":
                case "URI":
                case "DocumentUri":
                case "integer":
                case "uinteger":
                    // Go cmp.Compare: -1, 0 or +1.
                    return `self.${field}.cmp(&other.${field}) as i32`;
            }
        }

        throw new Error(`Cannot generate Compare for ${structName}.${fieldName}: unsupported field type ${JSON.stringify(prop.type)}. Add support in compareExpressionForProperty.`);
    }

    // ------------------------------------------------------------------
    // Enumerations
    // ------------------------------------------------------------------

    // Helper function to detect if an enum is a bitflag enum
    // Hardcoded list of bitflag enums
    const bitflagEnums = new Set(["WatchKind"]);

    function isBitflagEnum(enumeration: any): boolean {
        return bitflagEnums.has(enumeration.name);
    }

    startFile("enumerations.rs");
    writeLine("// Enumerations");
    writeLine("");

    for (const enumeration of model.enumerations) {
        write(formatDocumentation(enumeration.documentation));

        let baseType;
        switch (enumeration.type.name) {
            case "string":
                baseType = "string";
                break;
            case "integer":
                baseType = "int32";
                break;
            case "uinteger":
                baseType = "uint32";
                break;
            default:
                throw new Error(`Unsupported enum type: ${enumeration.type.name}`);
        }

        const isString = baseType === "string";
        writeLine(deriveLine(isString ? { copy: false, eq: true, hash: true } : allTraits, true));
        writeLine(`pub struct ${enumeration.name}(pub ${isString ? "Cow<'static, str>" : goTypeToRust(baseType)});`);
        writeLine("");

        // Get the pre-processed enum entries map that avoids duplicates

        const enumValues = enumeration.values.map(value => ({
            value: String(value.value),
            numericValue: Number(value.value),
            name: value.name,
            identifier: `${enumeration.name}${titleCase(value.name)}`,
            documentation: value.documentation,
            deprecated: value.deprecated,
        }));

        writeLine(`impl ${enumeration.name} {`);

        // Process entries with unique identifiers
        const constNames = new Set<string>();
        for (const entry of enumValues) {
            writeDocumentation(entry.documentation, "    ");

            let valueLiteral;
            // Handle string values
            if (isString) {
                valueLiteral = `Cow::Borrowed(${rustStr(entry.value.replace(/^"|"$/g, ""))})`;
            }
            else {
                valueLiteral = entry.value;
            }

            // Go: ${entry.identifier}
            const constName = rustConstName(entry.name);
            if (constNames.has(constName)) {
                throw new Error(`Enum const name collision in ${enumeration.name}: ${constName}`);
            }
            constNames.add(constName);
            writeLine(`    pub const ${constName}: ${enumeration.name} = ${enumeration.name}(${valueLiteral});`);
        }

        writeLine("}");
        writeLine("");

        // PORT: Go reflect IsZero of the underlying string or integer.
        writeLine(`impl IsZero for ${enumeration.name} {`);
        writeLine(`    fn is_zero(&self) -> bool {`);
        writeLine(isString ? `        self.0.is_empty()` : `        self.0 == 0`);
        writeLine(`    }`);
        writeLine(`}`);
        writeLine("");

        // PORT: Go uses the JSON v2 default string or integer arshalers for
        // named string and integer types (no value check).
        writeLine(`impl MarshalerTo for ${enumeration.name} {`);
        writeLine(`    fn marshal_json_to(&self, enc: &mut String) -> Result<(), JsonError> {`);
        writeLine(isString ? `        <str as MarshalerTo>::marshal_json_to(&self.0, enc)` : `        self.0.marshal_json_to(enc)`);
        writeLine(`    }`);
        writeLine(`}`);
        writeLine("");

        // PORT: the errors name the Go type, as the v2 default arshalers do.
        writeLine(`impl UnmarshalerFrom for ${enumeration.name} {`);
        writeLine(`    fn unmarshal_json_from(&mut self, dec: &mut JsonDecoder<'_>) -> Result<(), JsonError> {`);
        if (isString) {
            writeLine(`        let mut v = String::new();`);
            writeLine(`        unmarshal_string_as(dec, &mut v, ${rustStr(`lsproto.${enumeration.name}`)})?;`);
            writeLine(`        self.0 = Cow::Owned(v);`);
        }
        else {
            const helper = baseType === "uint32" ? "unmarshal_uint_as" : "unmarshal_int_as";
            writeLine(`        self.0 = ${helper}(dec, ${rustStr(`lsproto.${enumeration.name}`)})?;`);
        }
        writeLine(`        Ok(())`);
        writeLine(`    }`);
        writeLine(`}`);
        writeLine("");

        // Generate String() method for non-string enums
        if (enumeration.type.name !== "string") {
            const isBitflag = isBitflagEnum(enumeration);
            const nameConst = `_${rustConstName(enumeration.name)}_NAME`;
            const indexVar = `_${rustConstName(enumeration.name)}_INDEX`;
            const fmtDefault = `format!("${enumeration.name}({})", self.0)`;

            if (isBitflag) {
                // Generate bitflag-aware String() method using stringer-style efficiency
                const sortedValues = [...enumValues].sort((a, b) => a.numericValue - b.numericValue);
                const names = sortedValues.map(v => v.name);
                const values = sortedValues.map(v => v.numericValue);

                const combinedNames = names.join("");

                writeLine(`const ${nameConst}: &str = ${rustStr(combinedNames)};`);
                write(`const ${indexVar}: [u16; ${names.length + 1}] = [0`);
                let offset = 0;
                for (const name of names) {
                    offset += name.length;
                    write(`, ${offset}`);
                }
                writeLine(`];`);
                writeLine("");

                writeLine(`impl ${enumeration.name} {`);
                writeLine(`    // Go: (e ${enumeration.name}) String`);
                writeLine(`    pub fn string(&self) -> String {`);
                writeLine(`        if self.0 == 0 {`);
                writeLine(`            return "0".to_string();`);
                writeLine(`        }`);
                writeLine(`        let mut parts: Vec<&str> = Vec::new();`);
                for (let i = 0; i < values.length; i++) {
                    writeLine(`        if self.0 & ${values[i]} != 0 {`);
                    writeLine(`            parts.push(&${nameConst}[${indexVar}[${i}] as usize..${indexVar}[${i + 1}] as usize]);`);
                    writeLine(`        }`);
                }
                writeLine(`        if parts.is_empty() {`);
                writeLine(`            return ${fmtDefault};`);
                writeLine(`        }`);
                writeLine(`        parts.join("|")`);
                writeLine(`    }`);
                writeLine(`}`);
                writeLine("");
            }
            else {
                // Generate regular String() method using stringer-style approach
                // Split values into runs of contiguous values
                const sortedValues = [...enumValues].sort((a, b) => a.numericValue - b.numericValue);

                // Split into runs
                const runs: Array<{ names: string[]; values: number[]; }> = [];
                let currentRun = { names: [sortedValues[0].name], values: [sortedValues[0].numericValue] };

                for (let i = 1; i < sortedValues.length; i++) {
                    if (sortedValues[i].numericValue === sortedValues[i - 1].numericValue + 1) {
                        currentRun.names.push(sortedValues[i].name);
                        currentRun.values.push(sortedValues[i].numericValue);
                    }
                    else {
                        runs.push(currentRun);
                        currentRun = { names: [sortedValues[i].name], values: [sortedValues[i].numericValue] };
                    }
                }
                runs.push(currentRun);

                if (runs.length === 1) {
                    // Single contiguous run - simple case
                    const combinedNames = runs[0].names.join("");
                    writeLine(`const ${nameConst}: &str = ${rustStr(combinedNames)};`);
                    write(`const ${indexVar}: [u16; ${runs[0].names.length + 1}] = [0`);
                    let offset = 0;
                    for (const name of runs[0].names) {
                        offset += name.length;
                        write(`, ${offset}`);
                    }
                    writeLine(`];`);
                    writeLine("");

                    const minVal = runs[0].values[0];
                    writeLine(`impl ${enumeration.name} {`);
                    writeLine(`    // Go: (e ${enumeration.name}) String`);
                    writeLine(`    pub fn string(&self) -> String {`);
                    writeLine(`        let i = self.0 as i64 - (${minVal});`);
                    // For unsigned types, i can still be negative if e < minVal (due to underflow in conversion)
                    // So we always need to check both bounds
                    writeLine(`        if i < 0 || i >= ${indexVar}.len() as i64 - 1 {`);
                    writeLine(`            return ${fmtDefault};`);
                    writeLine(`        }`);
                    writeLine(`        let i = i as usize;`);
                    writeLine(`        ${nameConst}[${indexVar}[i] as usize..${indexVar}[i + 1] as usize].to_string()`);
                    writeLine(`    }`);
                    writeLine(`}`);
                    writeLine("");
                }
                else if (runs.length <= 10) {
                    // Multiple runs - use switch statement
                    let allNames = "";
                    const runInfo: Array<{ startOffset: number; endOffset: number; minVal: number; maxVal: number; }> = [];

                    for (const run of runs) {
                        const startOffset = allNames.length;
                        allNames += run.names.join("");
                        const endOffset = allNames.length;
                        runInfo.push({
                            startOffset,
                            endOffset,
                            minVal: run.values[0],
                            maxVal: run.values[run.values.length - 1],
                        });
                    }

                    writeLine(`const ${nameConst}: &str = ${rustStr(allNames)};`);
                    writeLine("");

                    // Generate index variables for each run
                    // PORT: Go also declares index arrays for one-value runs
                    // but never reads them; they are left out here.
                    for (let i = 0; i < runs.length; i++) {
                        if (runs[i].values.length === 1) continue;
                        write(`const ${indexVar}_${i}: [u16; ${runs[i].names.length + 1}] = [0`);
                        let offset = 0;
                        for (const name of runs[i].names) {
                            offset += name.length;
                            write(`, ${offset}`);
                        }
                        writeLine(`];`);
                    }
                    writeLine("");

                    writeLine(`impl ${enumeration.name} {`);
                    writeLine(`    // Go: (e ${enumeration.name}) String`);
                    writeLine(`    pub fn string(&self) -> String {`);
                    writeLine(`        let e = self.0;`);

                    for (let i = 0; i < runs.length; i++) {
                        const run = runs[i];
                        const info = runInfo[i];
                        const keyword = i === 0 ? "if" : "} else if";

                        if (run.values.length === 1) {
                            writeLine(`        ${keyword} e == ${run.values[0]} {`);
                            writeLine(`            ${nameConst}[${info.startOffset}..${info.endOffset}].to_string()`);
                        }
                        else {
                            if (info.minVal === 0 && baseType.startsWith("uint")) {
                                writeLine(`        ${keyword} e <= ${info.maxVal} {`);
                            }
                            else if (info.minVal === 0) {
                                writeLine(`        ${keyword} 0 <= e && e <= ${info.maxVal} {`);
                            }
                            else {
                                writeLine(`        ${keyword} ${info.minVal} <= e && e <= ${info.maxVal} {`);
                            }
                            writeLine(`            let i = (e as i64 - (${info.minVal})) as usize;`);
                            writeLine(`            ${nameConst}[${info.startOffset} + ${indexVar}_${i}[i] as usize..${info.startOffset} + ${indexVar}_${i}[i + 1] as usize].to_string()`);
                        }
                    }

                    writeLine(`        } else {`);
                    writeLine(`            ${fmtDefault}`);
                    writeLine(`        }`);
                    writeLine(`    }`);
                    writeLine(`}`);
                    writeLine("");
                }
                else {
                    // Too many runs - use a map
                    let allNames = "";
                    const valueMap: Array<{ value: number; startOffset: number; endOffset: number; }> = [];

                    for (const run of runs) {
                        for (let i = 0; i < run.names.length; i++) {
                            const startOffset = allNames.length;
                            allNames += run.names[i];
                            const endOffset = allNames.length;
                            valueMap.push({ value: run.values[i], startOffset, endOffset });
                        }
                    }

                    const mapVar = `${rustConstName(enumeration.name)}_MAP`;
                    writeLine(`const ${nameConst}: &str = ${rustStr(allNames)};`);
                    writeLine("");
                    writeLine(`static ${mapVar}: std::sync::LazyLock<FxHashMap<${enumeration.name}, &'static str>> =`);
                    writeLine(`    std::sync::LazyLock::new(|| {`);
                    writeLine(`        let mut m = FxHashMap::default();`);
                    for (const entry of valueMap) {
                        writeLine(`        m.insert(${enumeration.name}(${entry.value}), &${nameConst}[${entry.startOffset}..${entry.endOffset}]);`);
                    }
                    writeLine(`        m`);
                    writeLine(`    });`);
                    writeLine("");

                    writeLine(`impl ${enumeration.name} {`);
                    writeLine(`    // Go: (e ${enumeration.name}) String`);
                    writeLine(`    pub fn string(&self) -> String {`);
                    writeLine(`        if let Some(str) = ${mapVar}.get(self) {`);
                    writeLine(`            return str.to_string();`);
                    writeLine(`        }`);
                    writeLine(`        ${fmtDefault}`);
                    writeLine(`    }`);
                    writeLine(`}`);
                    writeLine("");
                }
            }

            // Go formats a value with a String method through that method.
            writeLine(`impl std::fmt::Display for ${enumeration.name} {`);
            writeLine(`    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {`);
            writeLine(`        f.write_str(&self.string())`);
            writeLine(`    }`);
            writeLine(`}`);
            writeLine("");
        }

        // Generate Error() method for ErrorCode to implement the error interface
        if (enumeration.name === "ErrorCode") {
            writeLine(`impl ${enumeration.name} {`);
            writeLine(`    // Go: (e ${enumeration.name}) Error`);
            writeLine(`    pub fn error(&self) -> String {`);
            writeLine(`        self.string()`);
            writeLine(`    }`);
            writeLine(`}`);
            writeLine("");
            writeLine(`impl std::error::Error for ${enumeration.name} {}`);
            writeLine("");
        }
    }

    // PORT: Go tsgo#4471 stopped generating `unmarshalParams` and
    // `unmarshalResult`: inbound params stay raw JSON until the handler
    // decodes them (`lsp.rs` `unmarshal_params`), and `RequestInfo`
    // decodes a result into its own type. So no dispatch.rs is written.

    function responseTypeNameOf(request: Request | Notification): string {
        const methodName = methodNameIdentifier(request.method);
        if (request.typeName && request.typeName.endsWith("Request")) {
            return request.typeName.replace(/Request$/, "Response");
        }
        return `${methodName}Response`;
    }


    // ------------------------------------------------------------------
    // Methods, response types, type mapping info, type aliases
    // ------------------------------------------------------------------
    startFile("methods.rs");

    writeLine("// Methods");
    writeLine("impl Method {");
    for (const request of requestsAndNotifications) {
        writeDocumentation(request.documentation, "    ");

        const methodName = methodNameIdentifier(request.method);

        writeLine(`    pub const ${rustConstName(methodName)}: Method = Method(Cow::Borrowed(${rustStr(request.method)}));`);
    }
    // Emit constants for registration-only methods (not also a request/notification)
    for (const reg of registrationMethods) {
        if (reg.isRegistrationOnly) {
            writeLine(`    // Registration-only method for ${reg.registrationMethod}.`);
            writeLine(`    pub const ${rustConstName(reg.fieldName)}: Method = Method(Cow::Borrowed(${rustStr(reg.registrationMethod)}));`);
        }
    }
    writeLine("}");
    writeLine("");

    // Generate request response types
    writeLine("// Request response types");
    writeLine("");

    for (const request of requestsAndNotifications) {
        const methodName = methodNameIdentifier(request.method);

        let responseTypeName: string | undefined;

        if ("result" in request) {
            responseTypeName = responseTypeNameOf(request);

            writeLine(`// Response type for \`${request.method}\``);

            // Special case for response types that are explicitly base type "null"
            if (request.result.kind === "base" && request.result.name === "null") {
                writeLine(`pub type ${responseTypeName} = Null;`);
            }
            else {
                const resultType = resolveType(request.result);
                const goType = resultType.needsPointer ? `*${resultType.name}` : resultType.name;
                writeLine(`pub type ${responseTypeName} = ${goTypeToRust(goType)};`);
            }
            writeLine("");
        }

        if (Array.isArray(request.params)) {
            throw new Error("Unexpected request params for " + methodName + ": " + JSON.stringify(request.params));
        }

        // PORT: Go names the params type as a pointer (*HoverParams); the Rust
        // type parameter is the value type (handlers downcast Box<dyn AnyValue>).
        const paramType = request.params ? resolveType(request.params) : undefined;
        const paramRustType = paramType ? goTypeToRust(paramType.name) : "NoParams";

        writeLine(`// Type mapping info for \`${request.method}\``);
        if (responseTypeName) {
            writeLine(`pub const ${rustConstName(`${methodName}Info`)}: RequestInfo<${paramRustType}, ${responseTypeName}> = ${requestInfoLiteral(methodName)};`);
        }
        else {
            writeLine(`pub const ${rustConstName(`${methodName}Info`)}: NotificationInfo<${paramRustType}> = ${notificationInfoLiteral(methodName)};`);
        }

        writeLine("");
    }

    // Generate type aliases
    writeLine("// Type aliases");
    writeLine("");
    for (const aliasName of customTypeAliases) {
        const resolvedType = resolveType(aliasName.type);
        const goType = resolvedType.needsPointer ? `*${resolvedType.name}` : resolvedType.name;
        writeLine(`pub type ${aliasName.name} = ${goTypeToRust(goType)};`);
        writeLine("");
    }

    // ------------------------------------------------------------------
    // Union types
    // ------------------------------------------------------------------
    startFile("unions.rs");
    writeLine("// Union types");
    writeLine("");

    for (const [name, members] of typeInfo.unionTypes.entries()) {
        currentUnion = name;
        const fields = rustFields(name, unionGoFields(members));

        let hasLocations = false;
        for (const f of fields) {
            if (f.goName === "Locations" && f.goType === "*[]Location") {
                hasLocations = true;
            }
        }

        writeStructDefinition(name, fields, false);

        // Get the field names and types for marshal/unmarshal methods
        const fieldEntries = unionGoEntries(members);

        // Marshal method
        writeLine(`impl MarshalerTo for ${name} {`);
        writeLine(`    // Go: (o *${name}) MarshalJSONTo`);
        writeLine(`    fn marshal_json_to(&self, enc: &mut String) -> Result<(), JsonError> {`);

        // Determine if this union contained null (check if any member has containedNull = true)
        const unionContainedNull = members.some(member => member.containedNull);
        // Always assert for non-nullable unions; for nullable unions, only when there are multiple fields.
        if (!unionContainedNull || fieldEntries.length > 1) {
            const sum = fieldEntries.map(e => `bool_to_int(self.${rustFieldName(e.fieldName)}.is_some())`).join("\n                + ");
            if (unionContainedNull) {
                writeLine(`        assert_at_most_one(`);
                writeLine(`            ${rustStr(`more than one element of ${name} is set`)},`);
            }
            else {
                writeLine(`        assert_only_one(`);
                writeLine(`            ${rustStr(`exactly one element of ${name} should be set`)},`);
            }
            writeLine(`            ${sum},`);
            writeLine(`        );`);
            writeLine("");
        }

        for (const entry of fieldEntries) {
            writeLine(`        if let Some(v) = &self.${rustFieldName(entry.fieldName)} {`);
            writeLine(`            return v.marshal_json_to(enc);`);
            writeLine(`        }`);
        }

        // If all fields are nil, marshal as null (only for unions that can contain null)
        if (unionContainedNull) {
            writeLine(`        // Go: enc.WriteToken(json.Null)`);
            writeLine(`        enc.push_str("null");`);
            writeLine(`        Ok(())`);
        }
        else {
            writeLine(`        panic!("unreachable")`);
        }
        writeLine(`    }`);
        writeLine(`}`);
        writeLine("");

        // Unmarshal method
        writeLine(`impl UnmarshalerFrom for ${name} {`);
        writeLine(`    // Go: (o *${name}) UnmarshalJSONFrom`);
        writeLine(`    fn unmarshal_json_from(&mut self, dec: &mut JsonDecoder<'_>) -> Result<(), JsonError> {`);
        writeLine(`        *self = ${name}::default();`);
        writeLine("");

        // Group field entries by their expected JSON token kind for optimized dispatch.
        const kindMap = new Map<string, typeof fieldEntries>();
        const unknownKindEntries: typeof fieldEntries = [];
        for (const entry of fieldEntries) {
            const kind = jsonKindForType(entry.originalType);
            if (!kind) {
                unknownKindEntries.push(entry);
            }
            else {
                if (!kindMap.has(kind)) kindMap.set(kind, []);
                kindMap.get(kind)!.push(entry);
            }
        }

        // Sort ambiguous variants (same JSON kind) by number of required fields
        // descending, so more specific variants are tried first. This prevents
        // a less specific variant from greedily matching inputs intended for
        // a more specific one.
        function countRequiredFields(entry: typeof fieldEntries[0]): number {
            if (entry.originalType.kind !== "reference") return 0;
            const structure = model.structures.find(s => s.name === (entry.originalType as ReferenceType).name);
            if (!structure) return 0;
            return structure.properties.filter(p => !p.optional && !p.omitzeroValue).length;
        }

        for (const [, entries] of kindMap) {
            if (entries.length > 1) {
                entries.sort((a, b) => countRequiredFields(b) - countRequiredFields(a));
            }
        }

        // Also sort the flat fieldEntries to match (for the fallback path)
        // We need to sort only within groups of the same kind.
        {
            const sorted: typeof fieldEntries = [];
            const seen = new Set<string>();
            for (const [, entries] of kindMap) {
                for (const entry of entries) {
                    sorted.push(entry);
                    seen.add(entry.fieldName);
                }
            }
            for (const entry of unknownKindEntries) {
                if (!seen.has(entry.fieldName)) {
                    sorted.push(entry);
                }
            }
            // Replace fieldEntries contents with sorted order
            fieldEntries.length = 0;
            fieldEntries.push(...sorted);
        }

        // Validate that ambiguous union variants (same JSON kind) don't have
        // order-dependent overlap. Two struct variants overlap if one's required
        // fields are a subset of the other's, meaning any valid input for the
        // superset also successfully parses as the subset (since unknown properties
        // are ignored). This would make the unmarshal result depend on try order.
        //
        // Exception: variants discriminated by literal field values (e.g., a "kind"
        // field with different string literal types) are safe because the literal
        // unmarshaler rejects mismatched values.
        for (const [kind, entries] of kindMap) {
            if (entries.length <= 1) continue;

            // Get required fields with their types for each variant
            const variantInfo = entries.map(entry => {
                if (entry.originalType.kind !== "reference") return null;
                const structure = model.structures.find(s => s.name === (entry.originalType as ReferenceType).name);
                if (!structure) return null;
                const requiredFields = new Map<string, Type>();
                for (const p of structure.properties) {
                    if (!p.optional && !p.omitzeroValue) {
                        requiredFields.set(p.name, p.type);
                    }
                }
                return { entry, requiredFields };
            }).filter((v): v is NonNullable<typeof v> => v !== null);

            // Check if two variants are discriminated by literal field values.
            // Returns true if they share a field where both sides have different
            // literal types (stringLiteral, integerLiteral, booleanLiteral).
            function isDiscriminatedByLiteral(
                a: Map<string, Type>,
                b: Map<string, Type>,
            ): boolean {
                for (const [fieldName, aType] of a) {
                    const bType = b.get(fieldName);
                    if (!bType) continue;
                    const aLiteral = aType.kind === "stringLiteral" || aType.kind === "integerLiteral" || aType.kind === "booleanLiteral";
                    const bLiteral = bType.kind === "stringLiteral" || bType.kind === "integerLiteral" || bType.kind === "booleanLiteral";
                    if (aLiteral && bLiteral) {
                        // Both are literals for the same field — check if values differ
                        if (aType.kind === bType.kind && (aType as any).value !== (bType as any).value) {
                            return true;
                        }
                        // Different literal kinds on same field also discriminates
                        if (aType.kind !== bType.kind) {
                            return true;
                        }
                    }
                }
                return false;
            }

            // Check each pair for subset relationships
            for (let i = 0; i < variantInfo.length; i++) {
                for (let j = 0; j < variantInfo.length; j++) {
                    if (i === j) continue;
                    const a = variantInfo[i];
                    const b = variantInfo[j];
                    const aNames = new Set(a.requiredFields.keys());
                    const bNames = new Set(b.requiredFields.keys());

                    const aSubsetOfB = [...aNames].every(f => bNames.has(f));
                    if (!aSubsetOfB) continue;

                    // Skip if discriminated by literal values
                    if (isDiscriminatedByLiteral(a.requiredFields, b.requiredFields)) continue;

                    if (aNames.size < bNames.size) {
                        // a is a strict subset of b
                        const aIdx = entries.indexOf(a.entry);
                        const bIdx = entries.indexOf(b.entry);
                        if (aIdx < bIdx) {
                            console.warn(
                                `Warning: In union ${name} (${kind} variants), ` +
                                    `${a.entry.fieldName} (required: [${[...aNames]}]) is tried before ` +
                                    `${b.entry.fieldName} (required: [${[...bNames]}]), but ` +
                                    `${a.entry.fieldName}'s required fields are a strict subset — ` +
                                    `it will greedily match inputs intended for ${b.entry.fieldName}. ` +
                                    `Reorder so the more specific variant is tried first.`,
                            );
                        }
                    }
                    else if (aNames.size === bNames.size && i < j) {
                        // Identical required fields — truly ambiguous
                        console.warn(
                            `Warning: In union ${name} (${kind} variants), ` +
                                `${a.entry.fieldName} and ${b.entry.fieldName} have identical ` +
                                `required fields [${[...aNames]}] — they are structurally ` +
                                `indistinguishable and the unmarshal result is order-dependent.`,
                        );
                    }
                }
            }
        }

        // Determine if we can use PeekKind-based dispatch:
        // - Every entry must have a known kind (no `any` etc.)
        // - There must be at least 2 distinct cases (kind groups + null) for a switch to be worthwhile
        const hasUnknownKinds = unknownKindEntries.length > 0;
        const distinctKinds = kindMap.size + (unionContainedNull ? 1 : 0);
        const canDispatch = !hasUnknownKinds && distinctKinds >= 2;

        // Check if all kind groups are unambiguous (exactly 1 entry each).
        // When unambiguous, we can UnmarshalDecode directly without buffering.
        const allUnambiguous = canDispatch && Array.from(kindMap.values()).every(entries => entries.length === 1);

        // Rust match arm pattern for the Go case that goKindCasesForJsonKind writes
        // (`case 't', 'f':` becomes `b't' | b'f'`).
        function rustKindPattern(kind: string): string {
            const chars = [...goKindCasesForJsonKind(kind).matchAll(/'(.)'/g)].map(m => m[1]);
            if (chars.length === 0) {
                throw new Error(`Unexpected JSON kind ${kind}`);
            }
            return chars.map(c => `b'${c}'`).join(" | ");
        }

        function writeDirectDecode(kind: string, entry: UnionEntry) {
            const field = rustFieldName(entry.fieldName);
            if (kind === "boolean") {
                writeLine(`                self.${field} = Some(kind == b't');`);
                writeLine(`                dec.read_token()?;`);
                writeLine(`                return Ok(());`);
            }
            else {
                writeLine(`                let v = self.${field}.insert(Default::default());`);
                writeLine(`                return json_unmarshal_decode(dec, ${derefBoxed(entry)});`);
            }
        }

        let fallbackExhaustive = false;
        const hasBooleanKind = kindMap.has("boolean");
        if (canDispatch) {
            // allUnambiguous: PeekKind + UnmarshalDecode directly, no ReadValue buffer needed.
            // Otherwise (mixed case): some kind groups have multiple entries.
            // Use PeekKind to dispatch, then ReadValue + try-each within ambiguous groups,
            // or UnmarshalDecode directly for unambiguous groups.
            if (hasBooleanKind) {
                writeLine(`        let kind = dec.peek_kind();`);
                writeLine(`        match kind {`);
            }
            else {
                writeLine(`        match dec.peek_kind() {`);
            }

            if (unionContainedNull) {
                writeLine(`            b'n' => {`);
                writeLine(`                dec.read_token()?;`);
                writeLine(`                return Ok(());`);
                writeLine(`            }`);
            }

            for (const [kind, entries] of kindMap) {
                writeLine(`            ${rustKindPattern(kind)} => {`);
                if (allUnambiguous || entries.length === 1) {
                    // Unambiguous: decode directly
                    writeDirectDecode(kind, entries[0]);
                }
                else {
                    let exhaustive = false;
                    const disc = findDiscriminatorField(entries);
                    if (disc && canStreamDiscriminator(disc)) {
                        generateStreamingDiscriminatorDispatch(name, disc, "                ");
                        exhaustive = true;
                    }
                    else {
                        // Ambiguous non-discriminated objects need the complete
                        // value for presence checks or speculative decoding.
                        writeLine(`                let data = dec.read_value()?;`);
                    }
                    if (disc && !canStreamDiscriminator(disc)) {
                        exhaustive = generateDiscriminatorDispatch(disc, "                ");
                    }
                    else if (!disc) {
                        const pres = findPresenceDiscriminator(entries);
                        if (pres) {
                            exhaustive = generatePresenceDispatch(pres, "                ");
                        }
                        else {
                            for (const entry of entries) {
                                writeTryEach(entry, "                ");
                            }
                        }
                    }
                    if (!exhaustive) {
                        writeLine(`                return Err(err_invalid_value(${rustStr(name)}, data));`);
                    }
                }
                writeLine(`            }`);
            }

            writeLine(`            _ => {`);
            writeLine(`                return Err(err_invalid_kind(${rustStr(name)}, dec.peek_kind()));`);
            writeLine(`            }`);
            writeLine(`        }`);
        }
        else {
            // Fallback for unknown kinds (e.g. `any`). Discriminated object
            // unions can still stream; other unions use ReadValue + try-each.
            let exhaustive = false;
            const disc = findDiscriminatorField(fieldEntries);
            if (disc && canStreamDiscriminator(disc)) {
                generateStreamingDiscriminatorDispatch(name, disc, "        ");
                exhaustive = true;
            }
            else {
                writeLine(`        let data = dec.read_value()?;`);
                if (unionContainedNull) {
                    writeLine(`        if data == b"null" {`);
                    writeLine(`            return Ok(());`);
                    writeLine(`        }`);
                    writeLine("");
                }
            }
            if (disc && !canStreamDiscriminator(disc)) {
                exhaustive = generateDiscriminatorDispatch(disc, "        ");
            }
            else if (!disc) {
                const pres = findPresenceDiscriminator(fieldEntries);
                if (pres) {
                    exhaustive = generatePresenceDispatch(pres, "        ");
                }
                else {
                    for (const entry of fieldEntries) {
                        writeTryEach(entry, "        ");
                    }
                }
            }
            fallbackExhaustive = exhaustive;
        }

        if (canDispatch) {
            // Dispatch paths have an exhaustive match with a default arm, nothing after the match.
        }
        else if (!fallbackExhaustive) {
            // Fallback paths: the final error references `data` which is in scope.
            writeLine(`        Err(err_invalid_value(${rustStr(name)}, data))`);
        }
        writeLine(`    }`);
        writeLine(`}`);
        writeLine("");

        writeIsZero(name, fields, "PORT: Go reflect.Value.IsZero (all fields nil).");

        // Generate GetLocations method
        if (hasLocations) {
            writeLine(`impl HasLocations for ${name} {`);
            writeLine(`    // Go: (o ${name}) GetLocations`);
            writeLine(`    fn get_locations(&self) -> Option<&Vec<Location>> {`);
            writeLine(`        self.locations.as_ref()`);
            writeLine(`    }`);
            writeLine(`}`);
            writeLine("");
        }
    }

    // ------------------------------------------------------------------
    // Literal types
    // ------------------------------------------------------------------
    startFile("literals.rs");
    writeLine("// Literal types");
    writeLine("");

    for (const [value, name] of typeInfo.literalTypes.entries()) {
        const jsonValue = JSON.stringify(value);

        writeLine(`// ${name} is a literal type for ${jsonValue}`);
        writeLine(deriveLine(allTraits));
        writeLine(`pub struct ${name};`);
        writeLine("");

        writeLine(`impl MarshalerTo for ${name} {`);
        writeLine(`    // Go: (o ${name}) MarshalJSONTo`);
        writeLine(`    fn marshal_json_to(&self, enc: &mut String) -> Result<(), JsonError> {`);
        writeLine(`        enc.push_str(${rustStr(jsonValue)});`);
        writeLine(`        Ok(())`);
        writeLine(`    }`);
        writeLine(`}`);
        writeLine("");

        writeLine(`impl UnmarshalerFrom for ${name} {`);
        writeLine(`    // Go: (o *${name}) UnmarshalJSONFrom`);
        writeLine(`    fn unmarshal_json_from(&mut self, dec: &mut JsonDecoder<'_>) -> Result<(), JsonError> {`);
        writeLine(`        let v = dec.read_value()?;`);
        writeLine(`        if v != ${rustByteStr(jsonValue)} {`);
        writeLine(`            return Err(err_literal_mismatch(${rustStr(name)}, ${rustStr(jsonValue)}, v));`);
        writeLine(`        }`);
        writeLine(`        Ok(())`);
        writeLine(`    }`);
        writeLine(`}`);
        writeLine("");

        writeIsZero(name, [], "PORT: Go reflect.Value.IsZero of struct{}.");
    }

    // Go `struct{}` (an empty object literal type, `EmptyObject` in union names).
    writeLine("// EmptyObject is Go `struct{}`: the empty object literal type `{}`.");
    writeLine(deriveLine(allTraits));
    writeLine("pub struct EmptyObject;");
    writeLine("");
    writeLine(`impl MarshalerTo for EmptyObject {`);
    writeLine(`    // PORT: Go marshals struct{} with the JSON v2 default struct arshaler.`);
    writeLine(`    fn marshal_json_to(&self, enc: &mut String) -> Result<(), JsonError> {`);
    writeLine(`        write_object_start(enc);`);
    writeLine(`        write_object_end(enc);`);
    writeLine(`        Ok(())`);
    writeLine(`    }`);
    writeLine(`}`);
    writeLine("");
    writeReflectUnmarshal("EmptyObject", []);
    writeIsZero("EmptyObject", [], "PORT: Go reflect.Value.IsZero of struct{}.");

    // ------------------------------------------------------------------
    // Resolved capabilities
    // ------------------------------------------------------------------
    startFile("resolved_capabilities.rs");

    if (clientCapsStructure) {
        writeLine("// Helper function for dereferencing pointers with zero value fallback");
        writeLine("// Go: derefOr");
        writeLine("pub fn deref_or<T: Clone + Default>(v: &Option<T>) -> T {");
        writeLine("    if let Some(v) = v {");
        writeLine("        return v.clone();");
        writeLine("    }");
        writeLine("    T::default()");
        writeLine("}");
        writeLine("");

        for (const { structure, isMain } of resolvedStructures) {
            if (isMain) {
                // Generate the main ResolvedClientCapabilities type and function
                writeLine("// ResolvedClientCapabilities is a version of ClientCapabilities where all nested");
                writeLine("// fields are values (not pointers), making it easier to access deeply nested capabilities.");
                writeLine("// Use ClientCapabilities::resolve (Go: (*ClientCapabilities).Resolve()) to convert from ClientCapabilities.");
                if (clientCapsStructure.documentation) {
                    writeLine("//");
                    const typeDoc = formatDocumentation(clientCapsStructure.documentation);
                    for (const line of typeDoc.split("\n").filter(l => l)) {
                        writeLine(line);
                    }
                }
            }
            generateResolvedTypeAndHelper(structure, isMain);
        }
    }

    function generateResolvedTypeAndHelper(structure: Structure, isMain: boolean) {
        const typeName = `Resolved${structure.name}`;
        // Main method is exported (Resolve), helpers are unexported (resolve)
        const methodName = isMain ? `Resolve` : `resolve`;

        // Generate the resolved type with documentation
        if (!isMain) {
            // For non-main types, add standard documentation header
            if (structure.documentation) {
                const typeDoc = formatDocumentation(structure.documentation);
                if (typeDoc) {
                    // Prepend comment explaining this is the resolved version
                    writeLine(`// ${typeName} is a resolved version of ${structure.name} with all optional fields`);
                    writeLine(`// converted to non-pointer values for easier access.`);
                    writeLine(`//`);
                    // Add the original structure documentation
                    for (const line of typeDoc.split("\n").filter(l => l)) {
                        writeLine(line);
                    }
                }
            }
            else {
                // If no documentation, just add a basic comment
                writeLine(`// ${typeName} is a resolved version of ${structure.name} with all optional fields`);
                writeLine(`// converted to non-pointer values for easier access.`);
            }
        }
        // For main type, documentation is added separately before calling this function

        const fields = rustFields(typeName, resolvedGoFields(structure));
        writeStructDefinition(typeName, fields, true);

        // Generate the conversion function (Go: method on the pointer receiver;
        // a nil receiver is None).
        const sourceFields = rustFields(structure.name, structureGoFields(structure));
        writeLine(`impl ${structure.name} {`);
        writeLine(`    // Go: (v *${structure.name}) ${methodName}`);
        writeLine(`    pub fn resolve(v: Option<&${structure.name}>) -> ${typeName} {`);
        writeLine(`        let Some(v) = v else {`);
        writeLine(`            return ${typeName}::default();`);
        writeLine(`        };`);
        writeLine(`        ${typeName} {`);
        structure.properties.forEach((prop, i) => {
            const type = resolveType(prop.type);
            const source = sourceFields[i];
            const accessPath = `v.${source.rustName}`;

            // For reference types that are structures, call the resolve method
            if (prop.type.kind === "reference") {
                const refStructure = model.structures.find(s => s.name === type.name);
                if (refStructure) {
                    let arg: string;
                    if (source.rustType.startsWith("Option<Box<")) arg = `${accessPath}.as_deref()`;
                    else if (source.rustType.startsWith("Option<")) arg = `${accessPath}.as_ref()`;
                    else arg = `Some(&${accessPath})`;
                    writeLine(`            ${source.rustName}: ${type.name}::resolve(${arg}),`);
                    return;
                }
            }

            // For other types, dereference if pointer
            if (prop.optional || type.needsPointer) {
                if (!source.rustType.startsWith("Option<")) {
                    throw new Error(`derefOr on a non-pointer field ${structure.name}.${source.goName}`);
                }
                writeLine(`            ${source.rustName}: deref_or(&${accessPath}),`);
            }
            else {
                writeLine(`            ${source.rustName}: ${copyOrClone(accessPath, source.goType)},`);
            }
        });
        writeLine(`        }`);
        writeLine(`    }`);
        writeLine(`}`);
        writeLine("");

        // PORT: Go marshals Resolved structures by reflection (the server logs
        // them) and never decodes them; both default arshalers are emitted.
        writeReflectMarshal(typeName, fields);
        writeReflectUnmarshal(typeName, fields);
        writeIsZero(typeName, fields, "PORT: Go reflect.Value.IsZero (omitzero).");
    }

    // ------------------------------------------------------------------
    // Report
    // ------------------------------------------------------------------
    console.log(`Boxed fields (type cycles): ${[...boxedEdges].join(", ") || "none"}`);
    console.log(`Structures decoded by the v2 default struct arshaler (${reflectionStructs.length}): ${reflectionStructs.join(", ")}`);

    const result = new Map<string, string>();
    for (const [name, fileParts] of files) {
        result.set(name, fileParts.join(""));
    }
    return result;
}

// ----------------------------------------------------------------------
// Runtime contract. The generated code calls these lsproto runtime items
// (lsp.rs, w1-lsproto-base) by name: RequestInfo and NotificationInfo are
// built as consts from a Method.
// ----------------------------------------------------------------------
function requestInfoLiteral(methodName: string): string {
    return `RequestInfo::new(Method::${rustConstName(methodName)})`;
}

function notificationInfoLiteral(methodName: string): string {
    return `NotificationInfo::new(Method::${rustConstName(methodName)})`;
}


function hasSomeProp(structure: Structure, propName: string, propTypeName: string) {
    return structure.properties?.some(p =>
        !p.optional &&
        p.name === propName &&
        p.type.kind === "reference" &&
        p.type.name === propTypeName
    );
}

function hasTextDocumentURI(structure: Structure) {
    return hasSomeProp(structure, "textDocument", "TextDocumentIdentifier") ||
        hasSomeProp(structure, "_vs_textDocument", "TextDocumentIdentifier");
}

function hasTextDocumentPosition(structure: Structure) {
    return hasSomeProp(structure, "position", "Position") ||
        hasSomeProp(structure, "_vs_position", "Position");
}

function getLocationUriProperty(structure: Structure) {
    const prop = structure.properties?.find(p =>
        !p.optional &&
        titleCase(p.name).endsWith("Uri") &&
        p.type.kind === "base" &&
        p.type.name === "DocumentUri"
    );
    if (
        prop &&
        structure.properties.some(p =>
            !p.optional &&
            titleCase(p.name) === titleCase(prop.name).replace(/Uri$/, "Range") &&
            p.type.kind === "reference" &&
            p.type.name === "Range"
        )
    ) {
        return titleCase(prop.name);
    }
}

/**
 * Main function
 */
async function main() {
    collectTypeDefinitions();
    const generatedFiles = generateCode();
    fs.mkdirSync(outDir, { recursive: true });
    const written: string[] = [];
    for (const [name, code] of generatedFiles) {
        const file = path.join(outDir, name);
        fs.writeFileSync(file, code);
        written.push(file);
    }

    // PORT: Go formats its output with dprint; the Rust output is formatted
    // with rustfmt. The import is dynamic so the copied header stays verbatim.
    const { execFileSync } = await import("node:child_process");
    execFileSync("rustfmt", ["--edition", "2024", ...written], { cwd: repoRoot, stdio: "inherit" });

    console.log(`Successfully generated ${written.length} files in ${outDir}`);
}

main().catch(e => {
    console.error(e);
    process.exit(1);
});
