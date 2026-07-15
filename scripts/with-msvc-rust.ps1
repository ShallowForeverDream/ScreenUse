param(
  [Parameter(ValueFromRemainingArguments = $true)]
  [string[]]$CommandArgs
)

$ErrorActionPreference = 'Stop'
$rustc = (& rustup which rustc --toolchain stable-x86_64-pc-windows-msvc).Trim()
if (-not (Test-Path -LiteralPath $rustc)) {
  throw "Cannot locate stable-x86_64-pc-windows-msvc rustc. Run: rustup toolchain install stable-x86_64-pc-windows-msvc"
}
$toolchainBin = Split-Path -Parent $rustc
$env:PATH = "$toolchainBin;$env:PATH"
$env:RUSTC = $rustc
if (-not $CommandArgs -or $CommandArgs.Count -eq 0) {
  throw 'No command provided.'
}
& $CommandArgs[0] @($CommandArgs | Select-Object -Skip 1)
exit $LASTEXITCODE
