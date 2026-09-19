param(
    [switch]$Clean
)

$ErrorActionPreference = "Stop"
Set-Location -LiteralPath $PSScriptRoot

if ($Clean) {
    Remove-Item -LiteralPath "dist" -Recurse -Force -ErrorAction SilentlyContinue
    Remove-Item -LiteralPath "viewpoint-windows-amd64.zip" -Force -ErrorAction SilentlyContinue
}

try { & go version | Out-Null } catch { throw "Go not found. Install Go 1.22+ from https://go.dev/dl/" }
try { & cargo --version | Out-Null } catch { throw "Rust not found. Install stable Rust from https://rustup.rs/" }

New-Item -ItemType Directory -Path "dist" -Force | Out-Null

Write-Output "Building server..."
& go build -o "dist/viewpoint-server.exe" ./server/cmd/viewpoint-server
if (-not $?) { throw "go build failed" }

Write-Output "Building client (release + audio)..."
& cargo build --release --manifest-path "client/Cargo.toml" --features audio
if (-not $?) { throw "cargo build failed" }

Copy-Item -LiteralPath "client/target/release/viewpoint-client.exe" -Destination "dist/viewpoint-client.exe" -Force
Copy-Item -LiteralPath "web" -Destination "dist/web" -Recurse -Force
Copy-Item -LiteralPath "viewpoint.example.yaml" -Destination "dist/viewpoint.example.yaml" -Force
Copy-Item -LiteralPath "LICENSE" -Destination "dist/LICENSE" -Force

if ((Test-Path -LiteralPath "viewpoint.yaml") -and (-not (Test-Path -LiteralPath "dist/viewpoint.yaml"))) {
    Copy-Item -LiteralPath "viewpoint.yaml" -Destination "dist/viewpoint.yaml"
}

$version = "dev"
try { $version = (& git rev-parse --short HEAD).Trim() } catch { }

Set-Content -LiteralPath "dist/VERSION.txt" -Value "Viewpoint $version`nBuilt $(Get-Date -Format o)" -NoNewline:$false

if (Test-Path -LiteralPath "viewpoint-windows-amd64.zip") {
    Remove-Item -LiteralPath "viewpoint-windows-amd64.zip" -Force
}
$zipItems = Get-ChildItem -LiteralPath "dist" -Exclude "viewpoint.yaml"
Compress-Archive -Path $zipItems.FullName -DestinationPath "viewpoint-windows-amd64.zip" -Force

Write-Output ""
Write-Output "Built dist/ ($version):"
Get-ChildItem -LiteralPath "dist" | Select-Object Name, Length | Format-Table -AutoSize | Out-String -Width 120 | Write-Output
Write-Output "Zip: viewpoint-windows-amd64.zip"
Write-Output "Run both with: powershell -ExecutionPolicy Bypass -File run.ps1"
