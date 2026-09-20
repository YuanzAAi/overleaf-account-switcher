param(
    [Parameter(Mandatory)][int]$ParentProcessId,
    [Parameter(Mandatory)][string]$Target,
    [Parameter(Mandatory)][string]$Stage
)

$ErrorActionPreference = 'Stop'
$Target = [IO.Path]::GetFullPath($Target)
$Stage = (Resolve-Path -LiteralPath $Stage).Path
$InstallDir = [IO.Path]::GetDirectoryName($Target)
if ([IO.Path]::GetDirectoryName($Stage) -ne $InstallDir -or
    -not [IO.Path]::GetFileName($Stage).StartsWith('.overleaf-update-') -or
    ((Get-Item -LiteralPath $Stage).Attributes -band [IO.FileAttributes]::ReparsePoint)) {
    throw 'Invalid update staging directory'
}

$Items = @()
$ParentExited = $false
$Recovered = $true
try {
    $Parent = Get-Process -Id $ParentProcessId -ErrorAction SilentlyContinue
    if ($Parent -and -not $Parent.WaitForExit(90000)) { throw 'Application did not exit' }
    $ParentExited = $true
    $Unpacked = Join-Path $Stage 'unpacked'
    Expand-Archive -LiteralPath (Join-Path $Stage 'payload.zip') -DestinationPath $Unpacked
    $Source = Join-Path $Unpacked 'overleaf-account-switcher'
    foreach ($Name in @('overleaf-desktop.exe', 'overleaf-service-api.exe', 'chrome_extension')) {
        if (-not (Test-Path -LiteralPath (Join-Path $Source $Name))) { throw "Update is missing $Name" }
    }
    $Items = @(@{ Source = (Join-Path $Source 'overleaf-desktop.exe'); Target = $Target; Backup = (Join-Path $Stage 'previous.exe'); Moved = $false; Installed = $false })
    foreach ($Name in @('overleaf-service-api.exe', 'chrome_extension')) {
        $Destination = Join-Path $InstallDir $Name
        if (Test-Path -LiteralPath $Destination) {
            $Items += @{ Source = (Join-Path $Source $Name); Target = $Destination; Backup = (Join-Path $Stage "previous-$Name"); Moved = $false; Installed = $false }
        }
    }
    foreach ($Item in $Items) {
        $ResolvedTarget = (Resolve-Path -LiteralPath $Item.Target).Path
        if ([IO.Path]::GetDirectoryName($ResolvedTarget) -ne $InstallDir -or
            ((Get-Item -LiteralPath $ResolvedTarget).Attributes -band [IO.FileAttributes]::ReparsePoint)) {
            throw 'Unsafe update destination'
        }
        Move-Item -LiteralPath $Item.Target -Destination $Item.Backup
        $Item.Moved = $true
        Move-Item -LiteralPath $Item.Source -Destination $Item.Target
        $Item.Installed = $true
    }
    Start-Process -FilePath $Target -WorkingDirectory $InstallDir -WindowStyle Hidden
} catch {
    $Reason = $_.Exception.Message
    [array]::Reverse($Items)
    foreach ($Item in $Items) {
        try {
            if ($Item.Installed) {
                $Resolved = (Resolve-Path -LiteralPath $Item.Target).Path
                if (-not $Resolved.StartsWith($InstallDir + [IO.Path]::DirectorySeparatorChar)) { throw 'Unsafe rollback target' }
                Remove-Item -LiteralPath $Resolved -Recurse -Force
            }
            if ($Item.Moved) { Move-Item -LiteralPath $Item.Backup -Destination $Item.Target }
        } catch { $Recovered = $false }
    }
    if ($ParentExited -and $Recovered) {
        Start-Process -FilePath $Target -WorkingDirectory $InstallDir -WindowStyle Hidden
    }
    Add-Type -AssemblyName System.Windows.Forms
    $Detail = if ($Recovered) { 'The previous version is unchanged.' } else { "Recovery files: $Stage" }
    [System.Windows.Forms.MessageBox]::Show("Update failed: $Reason`n$Detail", 'Overleaf Account Switcher') | Out-Null
} finally {
    if ($Recovered) {
        $Resolved = (Resolve-Path -LiteralPath $Stage).Path
        if ([IO.Path]::GetDirectoryName($Resolved) -eq $InstallDir -and [IO.Path]::GetFileName($Resolved).StartsWith('.overleaf-update-')) {
            Remove-Item -LiteralPath $Resolved -Recurse -Force
        }
    }
}
