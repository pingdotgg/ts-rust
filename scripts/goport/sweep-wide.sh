#!/bin/bash
# usage: sweep-wide.sh <round>. Re-runs goport on each prepared wide project config (51 projects,
# 292 configs) and diffs against its saved oracle file. Entries come from each <project>/manifest.json
# "configs" list (label, cwd, config, oracle file).
# Outputs go to measure/<round>/<label>.{out,err}. Saved oracle files are never overwritten.
# Line format matches sweep-extra2.sh. goport never writes an output file, so it runs on the
# read-only inputs. GOPORT_BIN picks the binary (default: accepted R118 goport).
# GOPORT_PIN=<key> runs this against that upstream pin (scripts/upstream/pin.py). Unset: no change.
[[ -z ${GOPORT_PIN:-} || -n ${GOPORT_PIN_ACTIVE:-} ]] || exec python3 /home/theo/Code/sandbox/ts-rust/scripts/upstream/pin.py exec -- bash "$0" "$@"
. "$(dirname "$(realpath "$0")")/exit-rule.sh" || exit 2
[ -n "$1" ] || { echo "usage: $0 <round>" >&2; exit 2; }
X=/home/theo/Code/sandbox/ts-rust/target/project-inputs-wide
B=${GOPORT_BIN:-/home/theo/Code/sandbox/ts-rust/target/continuation-r97-goport/runtime/cargo-r118/goport/goport}
# label|project dir|cwd (relative to project dir)|config (relative to cwd)|oracle file (relative to project dir)
# Same cwd and -p as the saved oracle run, so paths in output match.
# Labels are <project>:<manifest label>. Variant labels are TS7-compatible configs outside src
# (see each manifest.json).
# A run is complete when incomplete() of exit-rule.sh is false: exit 0 or 1 and no "unported" line on
# stderr, and at the Go pins in its EXIT2_PINS also exit 2 with no Go "panic: " line.
for entry in \
 "ajv-validator-ajv:root|ajv-validator-ajv|src|tsconfig.json|oracle/root.txt" \
 "ajv-validator-ajv:variant-ts7|ajv-validator-ajv|variant-ts7|tsconfig.json|oracle/variant-ts7.txt" \
 "angular-core:core-variant|angular-core|variant|tsconfig.json|oracle/core-variant.txt" \
 "apollo-client:root|apollo-client|src|tsconfig.json|oracle/root.txt" \
 "apollo-client:config|apollo-client|src/config|tsconfig.json|oracle/config.txt" \
 "arktypeio-arktype:root|arktypeio-arktype|src|tsconfig.json|oracle/root.txt" \
 "biomejs-biome-js-api:js-api|biomejs-biome-js-api|src/packages/@biomejs/js-api|tsconfig.json|oracle/js-api.txt" \
 "biomejs-biome-js-api:backend-jsonrpc|biomejs-biome-js-api|src/packages/@biomejs/backend-jsonrpc|tsconfig.json|oracle/backend-jsonrpc.txt" \
 "biomejs-biome-js-api:runtime|biomejs-biome-js-api|src/packages/@biomejs/runtime|tsconfig.json|oracle/runtime.txt" \
 "colinhacks-zod-v4:root|colinhacks-zod-v4|src|tsconfig.json|oracle/root.txt" \
 "colinhacks-zod-v4:mini|colinhacks-zod-v4|src/packages/mini|tsconfig.json|oracle/mini.txt" \
 "colinhacks-zod-v4:zod-build|colinhacks-zod-v4|src/packages/zod|tsconfig.build.json|oracle/zod-build.txt" \
 "colinhacks-zod-v4:mini-build|colinhacks-zod-v4|src/packages/mini|tsconfig.build.json|oracle/mini-build.txt" \
 "colinhacks-zod-v4:tsc|colinhacks-zod-v4|src/packages/tsc|tsconfig.json|oracle/tsc.txt" \
 "colinhacks-zod-v4:int-ai-sdk|colinhacks-zod-v4|src/packages/integration|fixtures/ai-sdk/tsconfig.json|oracle/int-ai-sdk.txt" \
 "colinhacks-zod-v4:int-drizzle|colinhacks-zod-v4|src/packages/integration|fixtures/drizzle-zod/tsconfig.json|oracle/int-drizzle.txt" \
 "date-fns-date-fns-other:core|date-fns-date-fns-other|src/pkgs/core|tsconfig.json|oracle/core.txt" \
 "date-fns-date-fns-other:docs|date-fns-date-fns-other|src/pkgs/docs|tsconfig.json|oracle/docs.txt" \
 "date-fns-date-fns-other:utc|date-fns-date-fns-other|src/pkgs/utc|tsconfig.json|oracle/utc.txt" \
 "date-fns-date-fns-other:utc-dist|date-fns-date-fns-other|src/pkgs/utc|tsconfig.dist.json|oracle/utc-dist.txt" \
 "date-fns-date-fns-other:tz|date-fns-date-fns-other|src/pkgs/tz|tsconfig.json|oracle/tz.txt" \
 "date-fns-date-fns-other:tz-dist|date-fns-date-fns-other|src/pkgs/tz|tsconfig.dist.json|oracle/tz-dist.txt" \
 "date-fns-date-fns-other:tz-engines|date-fns-date-fns-other|src/pkgs/tz/test/engines|tsconfig.json|oracle/tz-engines.txt" \
 "definitelytyped-express:express|definitelytyped-express|src/types/express|tsconfig.json|oracle/express.txt" \
 "definitelytyped-express:express-serve-static-core|definitelytyped-express|src/types/express-serve-static-core|tsconfig.json|oracle/express-serve-static-core.txt" \
 "definitelytyped-express:serve-static|definitelytyped-express|src/types/serve-static|tsconfig.json|oracle/serve-static.txt" \
 "definitelytyped-express:body-parser|definitelytyped-express|src/types/body-parser|tsconfig.json|oracle/body-parser.txt" \
 "definitelytyped-express:connect|definitelytyped-express|src/types/connect|tsconfig.json|oracle/connect.txt" \
 "definitelytyped-express:send|definitelytyped-express|src/types/send|tsconfig.json|oracle/send.txt" \
 "definitelytyped-express:qs|definitelytyped-express|src/types/qs|tsconfig.json|oracle/qs.txt" \
 "definitelytyped-express:http-errors|definitelytyped-express|src/types/http-errors|tsconfig.json|oracle/http-errors.txt" \
 "definitelytyped-express:cors|definitelytyped-express|src/types/cors|tsconfig.json|oracle/cors.txt" \
 "definitelytyped-express:express-session|definitelytyped-express|src/types/express-session|tsconfig.json|oracle/express-session.txt" \
 "definitelytyped-express:multer|definitelytyped-express|src/types/multer|tsconfig.json|oracle/multer.txt" \
 "definitelytyped-large:node|definitelytyped-large|src/types/node|tsconfig.json|oracle/node.txt" \
 "definitelytyped-large:node-dom|definitelytyped-large|src/types/node|tsconfig.dom.json|oracle/node-dom.txt" \
 "definitelytyped-large:node-non-dom|definitelytyped-large|src/types/node|tsconfig.non-dom.json|oracle/node-non-dom.txt" \
 "definitelytyped-large:node-webworker|definitelytyped-large|src/types/node|tsconfig.webworker.json|oracle/node-webworker.txt" \
 "definitelytyped-large:react|definitelytyped-large|src/types/react|tsconfig.json|oracle/react.txt" \
 "definitelytyped-large:react-dom|definitelytyped-large|src/types/react-dom|tsconfig.json|oracle/react-dom.txt" \
 "definitelytyped-large:jquery|definitelytyped-large|src/types/jquery|tsconfig.json|oracle/jquery.txt" \
 "definitelytyped-large:lodash|definitelytyped-large|src/types/lodash|tsconfig.json|oracle/lodash.txt" \
 "definitelytyped-large:lodash-es|definitelytyped-large|src/types/lodash-es|tsconfig.json|oracle/lodash-es.txt" \
 "definitelytyped-large:three|definitelytyped-large|src/types/three|tsconfig.json|oracle/three.txt" \
 "definitelytyped-large:openui5|definitelytyped-large|src/types/openui5|tsconfig.json|oracle/openui5.txt" \
 "definitelytyped-large:office-js|definitelytyped-large|src/types/office-js|tsconfig.json|oracle/office-js.txt" \
 "definitelytyped-large:google-apps-script|definitelytyped-large|src/types/google-apps-script|tsconfig.json|oracle/google-apps-script.txt" \
 "definitelytyped-large:chrome|definitelytyped-large|src/types/chrome|tsconfig.json|oracle/chrome.txt" \
 "definitelytyped-large:p5|definitelytyped-large|src/types/p5|tsconfig.json|oracle/p5.txt" \
 "definitelytyped-large:google.maps|definitelytyped-large|src/types/google.maps|tsconfig.json|oracle/google.maps.txt" \
 "definitelytyped-large:selenium-webdriver|definitelytyped-large|src/types/selenium-webdriver|tsconfig.json|oracle/selenium-webdriver.txt" \
 "definitelytyped-large:sharepoint|definitelytyped-large|src/types/sharepoint|tsconfig.json|oracle/sharepoint.txt" \
 "definitelytyped-large:vscode|definitelytyped-large|src/types/vscode|tsconfig.json|oracle/vscode.txt" \
 "definitelytyped-large:xrm|definitelytyped-large|src/types/xrm|tsconfig.json|oracle/xrm.txt" \
 "definitelytyped-large:jasmine|definitelytyped-large|src/types/jasmine|tsconfig.json|oracle/jasmine.txt" \
 "definitelytyped-large:es-abstract|definitelytyped-large|src/types/es-abstract|tsconfig.json|oracle/es-abstract.txt" \
 "definitelytyped-large:facebook-nodejs-business-sdk|definitelytyped-large|src/types/facebook-nodejs-business-sdk|tsconfig.json|oracle/facebook-nodejs-business-sdk.txt" \
 "drizzle-team-drizzle-orm:drizzle-zod.build|drizzle-team-drizzle-orm|src/drizzle-zod|tsconfig.build.json|oracle/drizzle-zod.build.txt" \
 "drizzle-team-drizzle-orm:drizzle-zod.tests|drizzle-team-drizzle-orm|src/drizzle-zod/tests|tsconfig.json|oracle/drizzle-zod.tests.txt" \
 "drizzle-team-drizzle-orm:variant.drizzle-zod.build|drizzle-team-drizzle-orm|variant|drizzle-zod.build.json|oracle/variant.drizzle-zod.build.txt" \
 "drizzle-team-drizzle-orm:variant.drizzle-zod.tests|drizzle-team-drizzle-orm|variant|drizzle-zod.tests.json|oracle/variant.drizzle-zod.tests.txt" \
 "drizzle-team-drizzle-orm:drizzle-valibot.build|drizzle-team-drizzle-orm|src/drizzle-valibot|tsconfig.build.json|oracle/drizzle-valibot.build.txt" \
 "drizzle-team-drizzle-orm:drizzle-valibot.tests|drizzle-team-drizzle-orm|src/drizzle-valibot/tests|tsconfig.json|oracle/drizzle-valibot.tests.txt" \
 "drizzle-team-drizzle-orm:variant.drizzle-valibot.build|drizzle-team-drizzle-orm|variant|drizzle-valibot.build.json|oracle/variant.drizzle-valibot.build.txt" \
 "drizzle-team-drizzle-orm:variant.drizzle-valibot.tests|drizzle-team-drizzle-orm|variant|drizzle-valibot.tests.json|oracle/variant.drizzle-valibot.tests.txt" \
 "drizzle-team-drizzle-orm:drizzle-typebox.build|drizzle-team-drizzle-orm|src/drizzle-typebox|tsconfig.build.json|oracle/drizzle-typebox.build.txt" \
 "drizzle-team-drizzle-orm:drizzle-typebox.tests|drizzle-team-drizzle-orm|src/drizzle-typebox/tests|tsconfig.json|oracle/drizzle-typebox.tests.txt" \
 "drizzle-team-drizzle-orm:variant.drizzle-typebox.build|drizzle-team-drizzle-orm|variant|drizzle-typebox.build.json|oracle/variant.drizzle-typebox.build.txt" \
 "drizzle-team-drizzle-orm:variant.drizzle-typebox.tests|drizzle-team-drizzle-orm|variant|drizzle-typebox.tests.json|oracle/variant.drizzle-typebox.tests.txt" \
 "drizzle-team-drizzle-orm:drizzle-arktype.build|drizzle-team-drizzle-orm|src/drizzle-arktype|tsconfig.build.json|oracle/drizzle-arktype.build.txt" \
 "drizzle-team-drizzle-orm:drizzle-arktype.tests|drizzle-team-drizzle-orm|src/drizzle-arktype/tests|tsconfig.json|oracle/drizzle-arktype.tests.txt" \
 "drizzle-team-drizzle-orm:variant.drizzle-arktype.build|drizzle-team-drizzle-orm|variant|drizzle-arktype.build.json|oracle/variant.drizzle-arktype.build.txt" \
 "drizzle-team-drizzle-orm:variant.drizzle-arktype.tests|drizzle-team-drizzle-orm|variant|drizzle-arktype.tests.json|oracle/variant.drizzle-arktype.tests.txt" \
 "effect-ts-effect:platform-node|effect-ts-effect|src/packages/platform/node|tsconfig.json|oracle/platform-node.txt" \
 "effect-ts-effect:platform-node-shared|effect-ts-effect|src/packages/platform/node-shared|tsconfig.json|oracle/platform-node-shared.txt" \
 "effect-ts-effect:platform-browser|effect-ts-effect|src/packages/platform/browser|tsconfig.json|oracle/platform-browser.txt" \
 "effect-ts-effect:platform-bun|effect-ts-effect|src/packages/platform/bun|tsconfig.json|oracle/platform-bun.txt" \
 "effect-ts-effect:sql-pg|effect-ts-effect|src/packages/sql/pg|tsconfig.json|oracle/sql-pg.txt" \
 "effect-ts-effect:sql-sqlite-node|effect-ts-effect|src/packages/sql/sqlite-node|tsconfig.json|oracle/sql-sqlite-node.txt" \
 "effect-ts-effect:sql-mysql2|effect-ts-effect|src/packages/sql/mysql2|tsconfig.json|oracle/sql-mysql2.txt" \
 "effect-ts-effect:ai-openai|effect-ts-effect|src/packages/ai/openai|tsconfig.json|oracle/ai-openai.txt" \
 "effect-ts-effect:ai-anthropic|effect-ts-effect|src/packages/ai/anthropic|tsconfig.json|oracle/ai-anthropic.txt" \
 "effect-ts-effect:opentelemetry|effect-ts-effect|src/packages/opentelemetry|tsconfig.json|oracle/opentelemetry.txt" \
 "effect-ts-effect:atom-react|effect-ts-effect|src/packages/atom/react|tsconfig.json|oracle/atom-react.txt" \
 "effect-ts-effect:vitest|effect-ts-effect|src/packages/vitest|tsconfig.json|oracle/vitest.txt" \
 "effect-ts-effect:tools-openapi-generator|effect-ts-effect|src/packages/tools/openapi-generator|tsconfig.json|oracle/tools-openapi-generator.txt" \
 "effect-ts-effect:tests|effect-ts-effect|src|tsconfig.tests.json|oracle/tests.txt" \
 "eslint-eslint:types|eslint-eslint|src|tsconfig.types.json|oracle/types.txt" \
 "eslint-eslint:tests-lib-types|eslint-eslint|src/tests/lib/types|tsconfig.json|oracle/tests-lib-types.txt" \
 "eslint-eslint:js-tests-types|eslint-eslint|src/packages/js/tests/types|tsconfig.json|oracle/js-tests-types.txt" \
 "eslint-eslint:config-eslint-tests-types|eslint-eslint|src/packages/eslint-config-eslint/tests/types|tsconfig.json|oracle/config-eslint-tests-types.txt" \
 "evanw-esbuild:lib|evanw-esbuild|src/lib|tsconfig.json|oracle/lib.txt" \
 "evanw-esbuild:lib-nolib|evanw-esbuild|src/lib|tsconfig-nolib.json|oracle/lib-nolib.txt" \
 "fastify:types|fastify|src/test/types|tsconfig.json|oracle/types.txt" \
 "floating-ui:react-lib|floating-ui|src/packages/react|tsconfig.lib.json|oracle/react-lib.txt" \
 "floating-ui:variant-react-lib|floating-ui|variant/react-lib|tsconfig.json|oracle/variant-react-lib.txt" \
 "floating-ui:react-test|floating-ui|src/packages/react|tsconfig.test.json|oracle/react-test.txt" \
 "floating-ui:variant-react-test|floating-ui|variant/react-test|tsconfig.json|oracle/variant-react-test.txt" \
 "floating-ui:dom-lib|floating-ui|src/packages/dom|tsconfig.lib.json|oracle/dom-lib.txt" \
 "floating-ui:variant-dom-lib|floating-ui|variant/dom-lib|tsconfig.json|oracle/variant-dom-lib.txt" \
 "floating-ui:dom-test|floating-ui|src/packages/dom|tsconfig.test.json|oracle/dom-test.txt" \
 "floating-ui:variant-dom-test|floating-ui|variant/dom-test|tsconfig.json|oracle/variant-dom-test.txt" \
 "floating-ui:core-lib|floating-ui|src/packages/core|tsconfig.lib.json|oracle/core-lib.txt" \
 "floating-ui:variant-core-lib|floating-ui|variant/core-lib|tsconfig.json|oracle/variant-core-lib.txt" \
 "floating-ui:core-test|floating-ui|src/packages/core|tsconfig.test.json|oracle/core-test.txt" \
 "floating-ui:variant-core-test|floating-ui|variant/core-test|tsconfig.json|oracle/variant-core-test.txt" \
 "floating-ui:utils-lib|floating-ui|src/packages/utils|tsconfig.lib.json|oracle/utils-lib.txt" \
 "floating-ui:variant-utils-lib|floating-ui|variant/utils-lib|tsconfig.json|oracle/variant-utils-lib.txt" \
 "floating-ui:utils-test|floating-ui|src/packages/utils|tsconfig.test.json|oracle/utils-test.txt" \
 "floating-ui:variant-utils-test|floating-ui|variant/utils-test|tsconfig.json|oracle/variant-utils-test.txt" \
 "graphql-js:graphql-js|graphql-js|src|tsconfig.json|oracle/graphql-js.txt" \
 "gvergnaud-hotscript:root|gvergnaud-hotscript|src|tsconfig.json|oracle/root.txt" \
 "gvergnaud-hotscript:test|gvergnaud-hotscript|src|test/tsconfig.json|oracle/test.txt" \
 "gvergnaud-hotscript:test-variant|gvergnaud-hotscript|variant-ts7|tsconfig.json|oracle/test-variant.txt" \
 "honojs-middleware:zod-openapi.build|honojs-middleware|src/packages/zod-openapi|tsconfig.build.json|oracle/zod-openapi.build.txt" \
 "honojs-middleware:zod-openapi.spec|honojs-middleware|src/packages/zod-openapi|tsconfig.spec.json|oracle/zod-openapi.spec.txt" \
 "honojs-middleware:valibot-validator.build|honojs-middleware|src/packages/valibot-validator|tsconfig.build.json|oracle/valibot-validator.build.txt" \
 "honojs-middleware:valibot-validator.spec|honojs-middleware|src/packages/valibot-validator|tsconfig.spec.json|oracle/valibot-validator.spec.txt" \
 "honojs-middleware:typebox-validator.build|honojs-middleware|src/packages/typebox-validator|tsconfig.build.json|oracle/typebox-validator.build.txt" \
 "honojs-middleware:typebox-validator.spec|honojs-middleware|src/packages/typebox-validator|tsconfig.spec.json|oracle/typebox-validator.spec.txt" \
 "honojs-middleware:arktype-validator.build|honojs-middleware|src/packages/arktype-validator|tsconfig.build.json|oracle/arktype-validator.build.txt" \
 "honojs-middleware:arktype-validator.spec|honojs-middleware|src/packages/arktype-validator|tsconfig.spec.json|oracle/arktype-validator.spec.txt" \
 "honojs-middleware:effect-validator.build|honojs-middleware|src/packages/effect-validator|tsconfig.build.json|oracle/effect-validator.build.txt" \
 "honojs-middleware:effect-validator.spec|honojs-middleware|src/packages/effect-validator|tsconfig.spec.json|oracle/effect-validator.spec.txt" \
 "honojs-middleware:standard-validator.build|honojs-middleware|src/packages/standard-validator|tsconfig.build.json|oracle/standard-validator.build.txt" \
 "honojs-middleware:standard-validator.spec|honojs-middleware|src/packages/standard-validator|tsconfig.spec.json|oracle/standard-validator.spec.txt" \
 "jestjs-jest:examples-typescript|jestjs-jest|src/examples/typescript|tsconfig.json|oracle/examples-typescript.txt" \
 "jestjs-jest:examples-expect-extend|jestjs-jest|src/examples/expect-extend|tsconfig.json|oracle/examples-expect-extend.txt" \
 "jestjs-jest:expect|jestjs-jest|src/packages/expect|tsconfig.json|oracle/expect.txt" \
 "jestjs-jest:expect~ts7|jestjs-jest|variant-ts7/expect|tsconfig.json|oracle/expect~ts7.txt" \
 "jestjs-jest:expect-tests~ts7|jestjs-jest|variant-ts7/expect-tests|tsconfig.json|oracle/expect-tests~ts7.txt" \
 "jestjs-jest:jest-runtime~ts7|jestjs-jest|variant-ts7/jest-runtime|tsconfig.json|oracle/jest-runtime~ts7.txt" \
 "kysely:kysely|kysely|src|tsconfig.json|oracle/kysely.txt" \
 "microsoft-vscode-base:base-variant|microsoft-vscode-base|variant-base|tsconfig.json|oracle/base-variant.txt" \
 "microsoft-vscode-base:monaco|microsoft-vscode-base|src/src|tsconfig.monaco.json|oracle/monaco.txt" \
 "microsoft-vscode-base:vscode-dts|microsoft-vscode-base|src/src|tsconfig.vscode-dts.json|oracle/vscode-dts.txt" \
 "microsoft-vscode-base:vscode-proposed-dts|microsoft-vscode-base|src/src|tsconfig.vscode-proposed-dts.json|oracle/vscode-proposed-dts.txt" \
 "microsoft-vscode-base:src|microsoft-vscode-base|src/src|tsconfig.json|oracle/src.txt" \
 "millsp-ts-toolbelt:root|millsp-ts-toolbelt|src|tsconfig.json|oracle/root.txt" \
 "millsp-ts-toolbelt:variant|millsp-ts-toolbelt|variant-ts7|tsconfig.json|oracle/variant.txt" \
 "mui-material-ui:material|mui-material-ui|src/packages/mui-material|tsconfig.json|oracle/material.txt" \
 "nestjs-nest:common|nestjs-nest|src/packages/common|tsconfig.build.json|oracle/common.txt" \
 "nestjs-nest:core|nestjs-nest|src/packages/core|tsconfig.build.json|oracle/core.txt" \
 "nestjs-nest:microservices|nestjs-nest|src/packages/microservices|tsconfig.build.json|oracle/microservices.txt" \
 "nestjs-nest:platform-express|nestjs-nest|src/packages/platform-express|tsconfig.build.json|oracle/platform-express.txt" \
 "nestjs-nest:platform-fastify|nestjs-nest|src/packages/platform-fastify|tsconfig.build.json|oracle/platform-fastify.txt" \
 "nestjs-nest:platform-socket.io|nestjs-nest|src/packages/platform-socket.io|tsconfig.build.json|oracle/platform-socket.io.txt" \
 "nestjs-nest:platform-ws|nestjs-nest|src/packages/platform-ws|tsconfig.build.json|oracle/platform-ws.txt" \
 "nestjs-nest:testing|nestjs-nest|src/packages/testing|tsconfig.build.json|oracle/testing.txt" \
 "nestjs-nest:websockets|nestjs-nest|src/packages/websockets|tsconfig.build.json|oracle/websockets.txt" \
 "pmndrs-valtio:root|pmndrs-valtio|src|tsconfig.json|oracle/root.txt" \
 "preactjs-preact:lint|preactjs-preact|src|jsconfig-lint.json|oracle/lint.txt" \
 "preactjs-preact:test-ts-core|preactjs-preact|src/test/ts|tsconfig.json|oracle/test-ts-core.txt" \
 "preactjs-preact:test-ts-compat|preactjs-preact|src/compat/test/ts|tsconfig.json|oracle/test-ts-compat.txt" \
 "preactjs-preact:variant-lint|preactjs-preact|variant/lint|tsconfig.json|oracle/variant-lint.txt" \
 "preactjs-preact:variant-test-ts-core|preactjs-preact|variant/test-ts-core|tsconfig.json|oracle/variant-test-ts-core.txt" \
 "preactjs-preact:variant-test-ts-compat|preactjs-preact|variant/test-ts-compat|tsconfig.json|oracle/variant-test-ts-compat.txt" \
 "prettier-prettier:root|prettier-prettier|src|tsconfig.json|oracle/root.txt" \
 "prisma-prisma-client:utils.typecheck|prisma-prisma-client|src|tsconfig.utils.typecheck.json|oracle/utils.typecheck.txt" \
 "prisma-prisma-client:client.build|prisma-prisma-client|src/packages/client|tsconfig.build.json|oracle/client.build.txt" \
 "prisma-prisma-client:variant.client|prisma-prisma-client|variant|tsconfig.client.json|oracle/variant.client.txt" \
 "radix-ui-primitives:react-dialog|radix-ui-primitives|src/packages/react/dialog|tsconfig.json|oracle/react-dialog.txt" \
 "radix-ui-primitives:react-select|radix-ui-primitives|src/packages/react/select|tsconfig.json|oracle/react-select.txt" \
 "radix-ui-primitives:radix-ui|radix-ui-primitives|src/packages/react/radix-ui|tsconfig.json|oracle/radix-ui.txt" \
 "remix-run-react-router:react-router|remix-run-react-router|src/packages/react-router|tsconfig.json|oracle/react-router.txt" \
 "remix-run-react-router:react-router-dev|remix-run-react-router|src/packages/react-router-dev|tsconfig.json|oracle/react-router-dev.txt" \
 "remix-run-react-router:react-router-node|remix-run-react-router|src/packages/react-router-node|tsconfig.json|oracle/react-router-node.txt" \
 "remix-run-react-router:react-router-express|remix-run-react-router|src/packages/react-router-express|tsconfig.json|oracle/react-router-express.txt" \
 "remix-run-react-router:react-router-cloudflare|remix-run-react-router|src/packages/react-router-cloudflare|tsconfig.json|oracle/react-router-cloudflare.txt" \
 "remix-run-react-router:react-router-architect|remix-run-react-router|src/packages/react-router-architect|tsconfig.json|oracle/react-router-architect.txt" \
 "remix-run-react-router:react-router-fs-routes|remix-run-react-router|src/packages/react-router-fs-routes|tsconfig.json|oracle/react-router-fs-routes.txt" \
 "remix-run-react-router:react-router-serve|remix-run-react-router|src/packages/react-router-serve|tsconfig.json|oracle/react-router-serve.txt" \
 "remix-run-react-router:create-react-router|remix-run-react-router|src/packages/create-react-router|tsconfig.json|oracle/create-react-router.txt" \
 "rollup-rollup:root|rollup-rollup|src|tsconfig.json|oracle/root.txt" \
 "sinclairzx81-typebox:root|sinclairzx81-typebox|src|tsconfig.json|oracle/root.txt" \
 "sinclairzx81-typebox:native-variant|sinclairzx81-typebox|variant-ts7|tsconfig.json|oracle/native-variant.txt" \
 "sindresorhus-type-fest:root|sindresorhus-type-fest|src|tsconfig.json|oracle/root.txt" \
 "sindresorhus-type-fest:minimum-lib|sindresorhus-type-fest|src|tsconfig.minimum-lib.json|oracle/minimum-lib.txt" \
 "sodiray-radash:root|sodiray-radash|src|tsconfig.json|oracle/root.txt" \
 "sodiray-radash:variant|sodiray-radash|variant-ts7|tsconfig.json|oracle/variant.txt" \
 "solidjs-solid:build|solidjs-solid|src/packages/solid|tsconfig.build.json|oracle/build.txt" \
 "solidjs-solid:main|solidjs-solid|src/packages/solid|tsconfig.json|oracle/main.txt" \
 "solidjs-solid:web|solidjs-solid|src/packages/solid|web/tsconfig.build.json|oracle/web.txt" \
 "solidjs-solid:variant-web|solidjs-solid|variant/web|tsconfig.json|oracle/variant-web.txt" \
 "solidjs-solid:web-storage|solidjs-solid|src/packages/solid|web/storage/tsconfig.build.json|oracle/web-storage.txt" \
 "solidjs-solid:variant-web-storage|solidjs-solid|variant/web-storage|tsconfig.json|oracle/variant-web-storage.txt" \
 "solidjs-solid:store|solidjs-solid|src/packages/solid|store/tsconfig.build.json|oracle/store.txt" \
 "solidjs-solid:variant-store|solidjs-solid|variant/store|tsconfig.json|oracle/variant-store.txt" \
 "solidjs-solid:html|solidjs-solid|src/packages/solid|html/tsconfig.json|oracle/html.txt" \
 "solidjs-solid:variant-html|solidjs-solid|variant/html|tsconfig.json|oracle/variant-html.txt" \
 "solidjs-solid:h|solidjs-solid|src/packages/solid|h/tsconfig.json|oracle/h.txt" \
 "solidjs-solid:variant-h|solidjs-solid|variant/h|tsconfig.json|oracle/variant-h.txt" \
 "solidjs-solid:h-jsx-runtime|solidjs-solid|src/packages/solid|h/jsx-runtime/tsconfig.json|oracle/h-jsx-runtime.txt" \
 "solidjs-solid:variant-h-jsx-runtime|solidjs-solid|variant/h-jsx-runtime|tsconfig.json|oracle/variant-h-jsx-runtime.txt" \
 "solidjs-solid:universal|solidjs-solid|src/packages/solid|universal/tsconfig.json|oracle/universal.txt" \
 "solidjs-solid:variant-universal|solidjs-solid|variant/universal|tsconfig.json|oracle/variant-universal.txt" \
 "solidjs-solid:test-types|solidjs-solid|src/packages/solid|tsconfig.test.json|oracle/test-types.txt" \
 "solidjs-solid:variant-test-types|solidjs-solid|variant/test-types|tsconfig.json|oracle/variant-test-types.txt" \
 "solidjs-solid:solid-element|solidjs-solid|src/packages/solid-element|tsconfig.json|oracle/solid-element.txt" \
 "statelyai-xstate:root|statelyai-xstate|src|tsconfig.json|oracle/root.txt" \
 "sveltejs-kit:kit|sveltejs-kit|src/packages/kit|tsconfig.json|oracle/kit.txt" \
 "sveltejs-kit:kit-test-types|sveltejs-kit|src/packages/kit|test/types/tsconfig.json|oracle/kit-test-types.txt" \
 "sveltejs-kit:kit-test-types-app-error|sveltejs-kit|src/packages/kit|test/types/app-error-enhanced/tsconfig.json|oracle/kit-test-types-app-error.txt" \
 "tailwindlabs-headlessui:react|tailwindlabs-headlessui|src/packages/@headlessui-react|tsconfig.json|oracle/react.txt" \
 "tailwindlabs-headlessui:variant-react|tailwindlabs-headlessui|variant/react|tsconfig.json|oracle/variant-react.txt" \
 "tailwindlabs-headlessui:vue|tailwindlabs-headlessui|src/packages/@headlessui-vue|tsconfig.json|oracle/vue.txt" \
 "tailwindlabs-headlessui:variant-vue|tailwindlabs-headlessui|variant/vue|tsconfig.json|oracle/variant-vue.txt" \
 "tj-commander.js:ts|tj-commander.js|src|tsconfig.ts.json|oracle/ts.txt" \
 "tj-commander.js:js|tj-commander.js|src|tsconfig.js.json|oracle/js.txt" \
 "toss-es-toolkit:root|toss-es-toolkit|src|tsconfig.json|oracle/root.txt" \
 "toss-es-toolkit:types|toss-es-toolkit|src|tests/types/tsconfig.json|oracle/types.txt" \
 "trpc-trpc:client|trpc-trpc|src/packages/client|tsconfig.json|oracle/client.txt" \
 "trpc-trpc:react-query|trpc-trpc|src/packages/react-query|tsconfig.json|oracle/react-query.txt" \
 "trpc-trpc:tanstack-react-query|trpc-trpc|src/packages/tanstack-react-query|tsconfig.json|oracle/tanstack-react-query.txt" \
 "trpc-trpc:next|trpc-trpc|src/packages/next|tsconfig.json|oracle/next.txt" \
 "trpc-trpc:openapi|trpc-trpc|src/packages/openapi|tsconfig.json|oracle/openapi.txt" \
 "ts-essentials-ts-essentials:test|ts-essentials-ts-essentials|src|tsconfig.test.json|oracle/test.txt" \
 "ts-essentials-ts-essentials:prod|ts-essentials-ts-essentials|src|tsconfig.prod.json|oracle/prod.txt" \
 "ts-rest-ts-rest:core-lib|ts-rest-ts-rest|src/libs/ts-rest/core|tsconfig.lib.json|oracle/core-lib.txt" \
 "ts-rest-ts-rest:express-lib|ts-rest-ts-rest|src/libs/ts-rest/express|tsconfig.lib.json|oracle/express-lib.txt" \
 "ts-rest-ts-rest:fastify-lib|ts-rest-ts-rest|src/libs/ts-rest/fastify|tsconfig.lib.json|oracle/fastify-lib.txt" \
 "ts-rest-ts-rest:nest-lib|ts-rest-ts-rest|src/libs/ts-rest/nest|tsconfig.lib.json|oracle/nest-lib.txt" \
 "ts-rest-ts-rest:next-lib|ts-rest-ts-rest|src/libs/ts-rest/next|tsconfig.lib.json|oracle/next-lib.txt" \
 "ts-rest-ts-rest:open-api-lib|ts-rest-ts-rest|src/libs/ts-rest/open-api|tsconfig.lib.json|oracle/open-api-lib.txt" \
 "ts-rest-ts-rest:react-query-v5-lib|ts-rest-ts-rest|src/libs/ts-rest/react-query-v5|tsconfig.lib.json|oracle/react-query-v5-lib.txt" \
 "ts-rest-ts-rest:serverless-lib|ts-rest-ts-rest|src/libs/ts-rest/serverless|tsconfig.lib.json|oracle/serverless-lib.txt" \
 "ts-rest-ts-rest:core-lib-variant|ts-rest-ts-rest|variant-ts7/libs/core|tsconfig.lib.json|oracle/core-lib-variant.txt" \
 "ts-rest-ts-rest:express-lib-variant|ts-rest-ts-rest|variant-ts7/libs/express|tsconfig.lib.json|oracle/express-lib-variant.txt" \
 "ts-rest-ts-rest:fastify-lib-variant|ts-rest-ts-rest|variant-ts7/libs/fastify|tsconfig.lib.json|oracle/fastify-lib-variant.txt" \
 "ts-rest-ts-rest:nest-lib-variant|ts-rest-ts-rest|variant-ts7/libs/nest|tsconfig.lib.json|oracle/nest-lib-variant.txt" \
 "ts-rest-ts-rest:next-lib-variant|ts-rest-ts-rest|variant-ts7/libs/next|tsconfig.lib.json|oracle/next-lib-variant.txt" \
 "ts-rest-ts-rest:open-api-lib-variant|ts-rest-ts-rest|variant-ts7/libs/open-api|tsconfig.lib.json|oracle/open-api-lib-variant.txt" \
 "ts-rest-ts-rest:react-query-v5-lib-variant|ts-rest-ts-rest|variant-ts7/libs/react-query-v5|tsconfig.lib.json|oracle/react-query-v5-lib-variant.txt" \
 "ts-rest-ts-rest:serverless-lib-variant|ts-rest-ts-rest|variant-ts7/libs/serverless|tsconfig.lib.json|oracle/serverless-lib-variant.txt" \
 "typeorm:typeorm.tsconfig|typeorm|src/packages/typeorm|tsconfig.json|oracle/typeorm.tsconfig.txt" \
 "typeorm:typeorm.node|typeorm|src/packages/typeorm|tsconfig.node.json|oracle/typeorm.node.txt" \
 "typeorm:variant-bundler|typeorm|variant|tsconfig.json|oracle/variant-bundler.txt" \
 "typescript-eslint-typescript-eslint-other:ast-spec-build|typescript-eslint-typescript-eslint-other|src/packages/ast-spec|tsconfig.build.json|oracle/ast-spec-build.txt" \
 "typescript-eslint-typescript-eslint-other:eslint-plugin-build|typescript-eslint-typescript-eslint-other|src/packages/eslint-plugin|tsconfig.build.json|oracle/eslint-plugin-build.txt" \
 "typescript-eslint-typescript-eslint-other:eslint-plugin-internal-build|typescript-eslint-typescript-eslint-other|src/packages/eslint-plugin-internal|tsconfig.build.json|oracle/eslint-plugin-internal-build.txt" \
 "typescript-eslint-typescript-eslint-other:parser-build|typescript-eslint-typescript-eslint-other|src/packages/parser|tsconfig.build.json|oracle/parser-build.txt" \
 "typescript-eslint-typescript-eslint-other:project-service-build|typescript-eslint-typescript-eslint-other|src/packages/project-service|tsconfig.build.json|oracle/project-service-build.txt" \
 "typescript-eslint-typescript-eslint-other:rule-schema-to-typescript-types-build|typescript-eslint-typescript-eslint-other|src/packages/rule-schema-to-typescript-types|tsconfig.build.json|oracle/rule-schema-to-typescript-types-build.txt" \
 "typescript-eslint-typescript-eslint-other:rule-tester-build|typescript-eslint-typescript-eslint-other|src/packages/rule-tester|tsconfig.build.json|oracle/rule-tester-build.txt" \
 "typescript-eslint-typescript-eslint-other:scope-manager-build|typescript-eslint-typescript-eslint-other|src/packages/scope-manager|tsconfig.build.json|oracle/scope-manager-build.txt" \
 "typescript-eslint-typescript-eslint-other:tsconfig-utils-build|typescript-eslint-typescript-eslint-other|src/packages/tsconfig-utils|tsconfig.build.json|oracle/tsconfig-utils-build.txt" \
 "typescript-eslint-typescript-eslint-other:typescript-eslint-build|typescript-eslint-typescript-eslint-other|src/packages/typescript-eslint|tsconfig.build.json|oracle/typescript-eslint-build.txt" \
 "typescript-eslint-typescript-eslint-other:typescript-estree-build|typescript-eslint-typescript-eslint-other|src/packages/typescript-estree|tsconfig.build.json|oracle/typescript-estree-build.txt" \
 "typescript-eslint-typescript-eslint-other:types-build|typescript-eslint-typescript-eslint-other|src/packages/types|tsconfig.build.json|oracle/types-build.txt" \
 "typescript-eslint-typescript-eslint-other:type-utils-build|typescript-eslint-typescript-eslint-other|src/packages/type-utils|tsconfig.build.json|oracle/type-utils-build.txt" \
 "typescript-eslint-typescript-eslint-other:visitor-keys-build|typescript-eslint-typescript-eslint-other|src/packages/visitor-keys|tsconfig.build.json|oracle/visitor-keys-build.txt" \
 "typescript-eslint-typescript-eslint-other:ast-spec-spec|typescript-eslint-typescript-eslint-other|src/packages/ast-spec|tsconfig.spec.json|oracle/ast-spec-spec.txt" \
 "typescript-eslint-typescript-eslint-other:eslint-plugin-spec|typescript-eslint-typescript-eslint-other|src/packages/eslint-plugin|tsconfig.spec.json|oracle/eslint-plugin-spec.txt" \
 "typescript-eslint-typescript-eslint-other:eslint-plugin-internal-spec|typescript-eslint-typescript-eslint-other|src/packages/eslint-plugin-internal|tsconfig.spec.json|oracle/eslint-plugin-internal-spec.txt" \
 "typescript-eslint-typescript-eslint-other:integration-tests-spec|typescript-eslint-typescript-eslint-other|src/packages/integration-tests|tsconfig.spec.json|oracle/integration-tests-spec.txt" \
 "typescript-eslint-typescript-eslint-other:parser-spec|typescript-eslint-typescript-eslint-other|src/packages/parser|tsconfig.spec.json|oracle/parser-spec.txt" \
 "typescript-eslint-typescript-eslint-other:project-service-spec|typescript-eslint-typescript-eslint-other|src/packages/project-service|tsconfig.spec.json|oracle/project-service-spec.txt" \
 "typescript-eslint-typescript-eslint-other:rule-schema-to-typescript-types-spec|typescript-eslint-typescript-eslint-other|src/packages/rule-schema-to-typescript-types|tsconfig.spec.json|oracle/rule-schema-to-typescript-types-spec.txt" \
 "typescript-eslint-typescript-eslint-other:rule-tester-spec|typescript-eslint-typescript-eslint-other|src/packages/rule-tester|tsconfig.spec.json|oracle/rule-tester-spec.txt" \
 "typescript-eslint-typescript-eslint-other:scope-manager-spec|typescript-eslint-typescript-eslint-other|src/packages/scope-manager|tsconfig.spec.json|oracle/scope-manager-spec.txt" \
 "typescript-eslint-typescript-eslint-other:tsconfig-utils-spec|typescript-eslint-typescript-eslint-other|src/packages/tsconfig-utils|tsconfig.spec.json|oracle/tsconfig-utils-spec.txt" \
 "typescript-eslint-typescript-eslint-other:typescript-eslint-spec|typescript-eslint-typescript-eslint-other|src/packages/typescript-eslint|tsconfig.spec.json|oracle/typescript-eslint-spec.txt" \
 "typescript-eslint-typescript-eslint-other:typescript-estree-spec|typescript-eslint-typescript-eslint-other|src/packages/typescript-estree|tsconfig.spec.json|oracle/typescript-estree-spec.txt" \
 "typescript-eslint-typescript-eslint-other:types-spec|typescript-eslint-typescript-eslint-other|src/packages/types|tsconfig.spec.json|oracle/types-spec.txt" \
 "typescript-eslint-typescript-eslint-other:type-utils-spec|typescript-eslint-typescript-eslint-other|src/packages/type-utils|tsconfig.spec.json|oracle/type-utils-spec.txt" \
 "typescript-eslint-typescript-eslint-other:utils-spec|typescript-eslint-typescript-eslint-other|src/packages/utils|tsconfig.spec.json|oracle/utils-spec.txt" \
 "typescript-eslint-typescript-eslint-other:visitor-keys-spec|typescript-eslint-typescript-eslint-other|src/packages/visitor-keys|tsconfig.spec.json|oracle/visitor-keys-spec.txt" \
 "vitejs-vite:pkg|vitejs-vite|src/packages/vite|tsconfig.json|oracle/pkg.txt" \
 "vitejs-vite:node|vitejs-vite|src/packages/vite|src/node/tsconfig.json|oracle/node.txt" \
 "vitejs-vite:client|vitejs-vite|src/packages/vite|src/client/tsconfig.json|oracle/client.txt" \
 "vitejs-vite:module-runner|vitejs-vite|src/packages/vite|src/module-runner/tsconfig.json|oracle/module-runner.txt" \
 "vitejs-vite:shared|vitejs-vite|src/packages/vite|src/shared/tsconfig.json|oracle/shared.txt" \
 "vitejs-vite:node-tests-dts|vitejs-vite|src/packages/vite|src/node/__tests_dts__/tsconfig.json|oracle/node-tests-dts.txt" \
 "vitejs-vite:module-runner-tests-dts|vitejs-vite|src/packages/vite|src/module-runner/__tests_dts__/tsconfig.json|oracle/module-runner-tests-dts.txt" \
 "vitejs-vite:check-dist|vitejs-vite|src/packages/vite|tsconfig.check.json|oracle/check-dist.txt" \
 "vitejs-vite:scripts|vitejs-vite|src|scripts/tsconfig.json|oracle/scripts.txt" \
 "vitest-dev-vitest-other:check|vitest-dev-vitest-other|src|tsconfig.check.json|oracle/check.txt" \
 "vitest-dev-vitest-other:vitest|vitest-dev-vitest-other|src/packages/vitest|tsconfig.json|oracle/vitest.txt" \
 "vitest-dev-vitest-other:browser|vitest-dev-vitest-other|src/packages/browser|tsconfig.json|oracle/browser.txt" \
 "vitest-dev-vitest-other:mocker|vitest-dev-vitest-other|src/packages/mocker|tsconfig.json|oracle/mocker.txt" \
 "vuejs-core:root|vuejs-core|src|tsconfig.json|oracle/root.txt" \
 "vuejs-core:build|vuejs-core|src|tsconfig.build.json|oracle/build.txt" \
 "vuejs-router:router|vuejs-router|src/packages/router|tsconfig.json|oracle/router.txt" \
 "vuejs-router:test-dts-experimental|vuejs-router|src/packages/router|test-dts/tsconfig.experimental.json|oracle/test-dts-experimental.txt" \
 "webpack-webpack:root|webpack-webpack|src|tsconfig.json|oracle/root.txt" \
 "webpack-webpack:types-test|webpack-webpack|src|tsconfig.types.test.json|oracle/types-test.txt" \
 "webpack-webpack:types-benchmark|webpack-webpack|src|tsconfig.types.benchmark.json|oracle/types-benchmark.txt" \
 "webpack-webpack:hot|webpack-webpack|src|tsconfig.hot.json|oracle/hot.txt" \
 "webpack-webpack:module-test|webpack-webpack|src|tsconfig.module.test.json|oracle/module-test.txt" \
 "withastro-astro:astro-build|withastro-astro|src/packages/astro|tsconfig.build.json|oracle/astro-build.txt" \
 "withastro-astro:astro-test|withastro-astro|src/packages/astro|tsconfig.test.json|oracle/astro-test.txt" \
 "withastro-astro:astro-types-test|withastro-astro|src/packages/astro|test/types/tsconfig.json|oracle/astro-types-test.txt" ; do
  IFS='|' read -r l n cwd c of <<< "$entry"
  OR=$X/$n/$of; O=$X/measure/$1; mkdir -p $O
  # Effect gate rule (Theo, 2026-10-10, issue #28): a config whose tsconfig lists @effect/language-service
  # expects the plain oracle lines plus the TS377xxx lines of effect-tsgo at the Effect pin. effect-oracle.py
  # writes that file, oracle/<label>.effect.txt, next to the plain one; the plain one stays the expected output
  # when it has no such file.
  [[ -f ${OR%.txt}.effect.txt ]] && OR=${OR%.txt}.effect.txt
  s=$(date +%s.%N); (cd $X/$n/$cwd && timeout 900 $B -p $c > $O/$l.out 2> $O/$l.err); e=$?
  t=$(python3 -c "import sys;print(round(float(sys.argv[2])-float(sys.argv[1]),1))" $s $(date +%s.%N))
  if incomplete $e $O/$l.err; then m=INCOMPLETE; elif diff -q $OR $O/$l.out >/dev/null; then m=MATCH; else m="DIFF(+$(diff $OR $O/$l.out | grep -c '^>') -$(diff $OR $O/$l.out | grep -c '^<'))"; fi
  echo "$l exit=$e diags=$(grep -c 'error TS' $O/$l.out) oracle=$(grep -c 'error TS' $OR) $m ${t}s panics=$(grep -c 'panic' $O/$l.err) unported=$(grep -c '^unported' $O/$l.err)"
done
