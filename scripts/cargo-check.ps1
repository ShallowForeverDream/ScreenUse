$ErrorActionPreference = 'Stop'
Push-Location "$PSScriptRoot\..\src-tauri"
try {
  & "$PSScriptRoot\with-msvc-rust.ps1" cargo check
  exit $LASTEXITCODE
} finally {
  Pop-Location
}
