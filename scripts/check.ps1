# Same workspace gate as check.sh, for Windows without a Unix shell.
$ErrorActionPreference = 'Stop'
Push-Location (Split-Path $PSScriptRoot -Parent)
try {
    cargo fmt --all --check
    if ($LASTEXITCODE -ne 0) { throw 'cargo fmt failed' }
    cargo check --workspace
    if ($LASTEXITCODE -ne 0) { throw 'cargo check failed' }
    cargo test --workspace
    if ($LASTEXITCODE -ne 0) { throw 'cargo test failed' }
    cargo clippy --workspace --all-targets -- -D warnings
    if ($LASTEXITCODE -ne 0) { throw 'cargo clippy failed' }
} finally {
    Pop-Location
}
Write-Output 'gate passed: fmt, check, test, clippy -D warnings'
