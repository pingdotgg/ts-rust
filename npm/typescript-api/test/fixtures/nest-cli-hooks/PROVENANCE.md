# nest-cli hook fixtures

The directories here are nest-cli's spec fixtures for its compiler hooks, copied without change from
[nestjs/nest-cli](https://github.com/nestjs/nest-cli):

- Tag `12.0.8`, commit `18649483169810a3485012aa1fccd11897d95f2c` (the commit that
  `scripts/typescript-api/consumers/nest/pins.json` pins)
- Upstream path: `test/lib/compiler/hooks/fixtures`

`test/emit.test.mjs` compiles them with the tsconfig-paths hook of `@nestjs/cli` 12.0.8.

nest-cli is licensed under the MIT License:
[licenses/nest-cli-LICENSE.txt](../../../../../licenses/nest-cli-LICENSE.txt).
