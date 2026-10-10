#!/usr/bin/env bash
# One-time setup of npm trusted publishing for the tsc-rs packages. Run it by hand, logged in to
# npm as an owner of tsc-rs and the @tsc-rs org, with 2FA on (npm asks for a code once).
#
# For each package (tsc-rs, @tsc-rs/linux-x64, @tsc-rs/linux-arm64,
# @tsc-rs/darwin-arm64, @tsc-rs/win32-x64) it:
#   1. publishes a 0.0.0-placeholder version when the package is not on npm yet: npm can only
#      trust a workflow for a package that exists. The release workflow refuses 0.0.x tags, so a
#      release never collides with a placeholder (tsc-rs has a 0.0.1 placeholder).
#   2. trusts .github/workflows/release.yml of pingdotgg/ts-rust in the environment `npm` to
#      publish it. When the package already trusts something else, it stops and prints the
#      revoke command (it does not revoke by itself).
# npm drops a new trust that publishes nothing in 2 days. So run this shortly before the first tag.
# The first publish binds a trust to the repo's GitHub ID, not only its name. After the repo is
# recreated, run it with --relink: it revokes every trust of each package first. The 2026-10-07
# recreation needs no relink: v0.1.0 already published from the new repo.
#
# usage: npm/trust-setup.sh [--relink]    needs npm 11.15.0 or later (npm trust)
set -euo pipefail
[[ ${1:-} != help ]] || { sed -n '2,18p' "$0" >&2; exit 2; }
relink=0
[[ ${1:-} != --relink ]] || relink=1
repo=pingdotgg/ts-rust
packages=(tsc-rs @tsc-rs/linux-x64 @tsc-rs/linux-arm64 @tsc-rs/darwin-arm64 @tsc-rs/win32-x64)

npm_version=$(npm --version)
[[ $(printf '%s\n' 11.15.0 "$npm_version" | sort -V | head -1) == 11.15.0 ]] ||
  { echo "npm $npm_version: npm trust needs 11.15.0 or later (npm install -g npm@11)" >&2; exit 1; }
echo "npm user: $(npm whoami)"

for name in "${packages[@]}"; do
  if npm view "$name" version > /dev/null 2>&1; then
    echo "$name is on npm"
  else
    dir=$(mktemp -d)
    cat > "$dir/package.json" << EOF
{
  "name": "$name",
  "version": "0.0.0-placeholder",
  "description": "A Rust port of the TypeScript 7 compiler. Placeholder: not released yet.",
  "license": "MIT",
  "repository": { "type": "git", "url": "git+https://github.com/$repo.git" }
}
EOF
    (cd "$dir" && npm publish --access public --tag placeholder)
    rm -rf "$dir"
    echo "$name: placeholder 0.0.0-placeholder published"
  fi
done

for name in "${packages[@]}"; do
  if ((relink)); then
    # An assignment, not `for id in $(...)`: set -e stops the script when the list or the parse fails.
    ids=$(npm trust list "$name" --json | node -e '
      const text = require("fs").readFileSync(0, "utf8").trim();
      for (const t of text ? JSON.parse(`[${text.replace(/}\s*{/g, "},{")}]`) : []) console.log(t.id);
    ')
    for id in $ids; do
      npm trust revoke "$name" --id="$id"
      sleep 2
    done
  fi
  # `npm trust list --json` prints one JSON object per trust (npm/cli lib/trust-cmd.js
  # logOptions): id, type, file, repository, environment and permissions. createPackage is
  # --allow-publish.
  state=$(npm trust list "$name" --json | node -e '
    const text = require("fs").readFileSync(0, "utf8").trim();
    const trusts = text ? JSON.parse(`[${text.replace(/}\s*{/g, "},{")}]`) : [];
    const ok = trusts.some(t => t.type === "github" && t.repository === process.argv[1] &&
      t.file === "release.yml" && t.environment === "npm" &&
      (t.permissions ?? []).includes("createPackage"));
    console.log(ok ? "ok" : trusts.length ? "other" : "none");
  ' "$repo")
  case $state in
    none)
      npm trust github "$name" --file release.yml --repo "$repo" --env npm --allow-publish --yes
      sleep 2 ;;
    other)
      echo "$name trusts another repo, workflow, environment or permission:" >&2
      npm trust list "$name" >&2
      echo "revoke it with: npm trust revoke $name --id <id>, then run this again" >&2
      exit 1 ;;
  esac
  npm trust list "$name"
done
