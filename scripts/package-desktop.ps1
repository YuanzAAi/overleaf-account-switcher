param([switch]$SkipBuild)

$ErrorActionPreference = "Stop"
$Root = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$OutputDir = Join-Path $Root "target\desktop-package\overleaf-account-switcher"
$Archive = Join-Path $Root "target\desktop-package\overleaf-windows-x64-portable.zip"
$StandaloneExe = Join-Path $Root "target\desktop-package\overleaf-windows-x64.exe"

Push-Location $Root
try {
    if (-not $SkipBuild) {
        npm.cmd --prefix apps/web run build
        if ($LASTEXITCODE -ne 0) { throw "Web build failed" }
        cargo build --locked -p overleaf-service --bin overleaf-service-api --release
        if ($LASTEXITCODE -ne 0) { throw "Service build failed" }

        Push-Location (Join-Path $Root "apps\desktop")
        try {
            npm.cmd run tauri:build
            if ($LASTEXITCODE -ne 0) { throw "Desktop build failed" }
        } finally {
            Pop-Location
        }
    }

    $inputs = @(
        "target\release\overleaf-desktop.exe",
        "target\release\overleaf-service-api.exe",
        "chrome_extension"
    )
    foreach ($inputPath in $inputs) {
        if (-not (Test-Path -LiteralPath (Join-Path $Root $inputPath))) {
            throw "Missing package input: $inputPath"
        }
    }

    if (Test-Path -LiteralPath $OutputDir) {
        $ResolvedOutput = (Resolve-Path -LiteralPath $OutputDir).Path
        if (-not $ResolvedOutput.StartsWith($Root + [IO.Path]::DirectorySeparatorChar) -or
            ((Get-Item -LiteralPath $OutputDir).Attributes -band [IO.FileAttributes]::ReparsePoint)) {
            throw "Unsafe package output directory: $ResolvedOutput"
        }
        Remove-Item -LiteralPath $OutputDir -Recurse -Force
    }
    New-Item -ItemType Directory -Path $OutputDir -Force | Out-Null
    foreach ($inputPath in $inputs) {
        Copy-Item -LiteralPath (Join-Path $Root $inputPath) -Destination $OutputDir -Recurse
    }
    Compress-Archive -LiteralPath $OutputDir -DestinationPath $Archive -Force
    Copy-Item -LiteralPath (Join-Path $Root "target\release\overleaf-desktop.exe") -Destination $StandaloneExe -Force
    Write-Host "Package: $OutputDir"
    Write-Host "Archive: $Archive"
    Write-Host "Standalone: $StandaloneExe"
} finally {
    Pop-Location
}
