$ErrorActionPreference = 'Stop'
& "$PSScriptRoot\with-msvc-rust.ps1" pnpm exec tauri build --features custom-protocol
exit $LASTEXITCODE
