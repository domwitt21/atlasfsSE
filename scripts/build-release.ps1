$ErrorActionPreference = 'Stop'

$projectRoot = Split-Path -Parent $PSScriptRoot
Push-Location $projectRoot
try {
    cargo fmt --all -- --check
    cargo clippy --workspace --all-targets --locked -- -D warnings
    cargo test --workspace --locked
    cargo build --release --locked -p atlasfs-cli
    $binary = Join-Path $projectRoot 'target\release\atlas.exe'
    $hash = (Get-FileHash -Algorithm SHA256 -LiteralPath $binary).Hash.ToLowerInvariant()
    Set-Content -LiteralPath "$binary.sha256" -Value "$hash  atlas.exe" -Encoding ascii
    Write-Output "Release artifact: $binary"
    Write-Output "SHA-256: $hash"
}
finally {
    Pop-Location
}
