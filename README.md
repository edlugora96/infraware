# infraware

## Install (private npm)

```sh
# configure auth once (choose one)
npm login  # or set NPM_TOKEN
# or create ~/.npmrc with:
# //registry.npmjs.org/:_authToken=${NPM_TOKEN}

npm install -g @edlugora96/infraware
infraware --help

# one-shot
npx @edlugora96/infraware --help
```

Requires Node >=18. Package is private (`--access restricted`); you need an npm account granted access. Auth via `npm login` or `NPM_TOKEN` env / `.npmrc`.

## Supported targets

Bundled binaries: `darwin-arm64`, `darwin-x64`, `linux-x64`, `win32-x64` (others error: unsupported platform). No auto-updater; update via `npm install -g @edlugora96/infraware@<version>`.

## Versioning

- `0.1.0` (Cargo.toml authoritative, package.json in sync)
- Git tags `v*` trigger releases.

## Troubleshooting

- `npm ERR! 404/403` → not logged in or not granted access to `@edlugora96/infraware`; run `npm login` or check `NPM_TOKEN`.
- Unsupported platform → only four targets above are supported.
