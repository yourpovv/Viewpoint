# This will run the server and client you just need to build it first:
# run: powershell -ExecutionPolicy Bypass -File build.ps1

$ErrorActionPreference = "Stop"
Set-Location -LiteralPath $PSScriptRoot

$Dist = Join-Path $PSScriptRoot "dist"
if (-not (Test-Path -LiteralPath (Join-Path $Dist "viewpoint-server.exe")) -or
    -not (Test-Path -LiteralPath (Join-Path $Dist "viewpoint-client.exe"))) {
    throw "dist/ binaries missing. Run: powershell -ExecutionPolicy Bypass -File build.ps1"
}

if (-not (Test-Path -LiteralPath (Join-Path $Dist "viewpoint.yaml"))) {
    Copy-Item -LiteralPath (Join-Path $Dist "viewpoint.example.yaml") -Destination (Join-Path $Dist "viewpoint.yaml")
    Write-Output "Created dist/viewpoint.yaml from example. Edit it, then re-run."
}

if ([string]::IsNullOrWhiteSpace($env:VIEWPOINT_JWT_SECRET)) {
    $env:VIEWPOINT_JWT_SECRET = "local-test"
    Write-Output "VIEWPOINT_JWT_SECRET not set, using localhost default 'local-test'."
}

Set-Location -LiteralPath $Dist

$server = Start-Process -FilePath ".\viewpoint-server.exe" -NoNewWindow -PassThru
Write-Output "Server started (PID $($server.Id)). Starting client, Ctrl+C stops both."

try {
    & ".\viewpoint-client.exe"
}
finally {
    if (-not $server.HasExited) {
        Stop-Process -InputObject $server -Force
        Write-Output "Server stopped."
    }
}
