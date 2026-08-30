# Binaries

Precompiled binaries bundled for private npm distribution. Targets:
- darwin-arm64
- darwin-x64
- linux-x64
- win32-x64 (infraware.exe)

Binaries are built in CI on every push to `main` and committed directly to this directory by an isolated privileged job. Do not commit placeholders. Each binary is validated for non-empty, executable, and native format (ELF/Mach-O/PE) before commit. `checksums.txt` at repo root is generated deterministically.

Do not add postinstall downloaders.
