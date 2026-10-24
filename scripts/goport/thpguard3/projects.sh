# Sourced by the thpguard3 scripts: name|cwd|tsgo args (all --noEmit: CI type check, no writes to inputs).
REPO=/home/theo/Code/sandbox/ts-rust
PROJECTS=(
  "valibot|$REPO/target/project-inputs-extra/valibot|-p tsconfig.goport.json --noEmit"
  "query|$REPO|-p target/project-inputs/query/source/packages/query-core/tsconfig.prod.json --noEmit"
  "hono|$REPO|-p target/project-inputs/hono/source/tsconfig.build.json --noEmit"
  "zod|$REPO|-p target/project-inputs/zod/source/packages/zod/tsconfig.json --noEmit"
  "effect|$REPO|-p target/project-inputs/effect/source/packages/effect/tsconfig.json --noEmit"
)
OURS=${THPG_OURS:-$REPO/target/continuation-r97-goport/evidence-cache/6e5c80f11fee7081/bins/tsgo}
GO=/home/theo/.local/bin/tsgo-oracle-3e9236361fa9
