$ErrorActionPreference = 'Stop'
& "$PSScriptRoot\with-msvc-rust.ps1" pnpm exec tauri dev
exit $LASTEXITCODE
