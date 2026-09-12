param([switch]$Remove)
$ErrorActionPreference = 'Stop'
try {
    if ([Environment]::OSVersion.Version.Build -lt 22000) { exit 0 }
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $package = Join-Path $PSScriptRoot 'PlumeTxt.ShellMenu.msix'
    $zip = [IO.Compression.ZipFile]::OpenRead($package)
    try {
        $reader = [IO.StreamReader]::new($zip.GetEntry('AppxManifest.xml').Open())
        try { [xml]$manifest = $reader.ReadToEnd() } finally { $reader.Dispose() }
    } finally { $zip.Dispose() }
    if ($manifest.Package.Identity.Name -ne 'PlumeTxt.ShellMenu') { throw 'Unexpected shell package identity.' }
    if ($Remove) {
        Get-AppxPackage -Name 'PlumeTxt.ShellMenu' | Where-Object Publisher -EQ $manifest.Package.Identity.Publisher |
            Remove-AppxPackage -ErrorAction Stop
    } else {
        Add-AppxPackage -Path $package -ExternalLocation $PSScriptRoot -ForceUpdateFromAnyVersion -ErrorAction Stop
    }
    exit 0
} catch {
    $log = Join-Path $env:LOCALAPPDATA 'PlumeTxt/shell-menu-install.log'
    [void](New-Item -ItemType Directory -Force -Path (Split-Path $log))
    $_ | Out-String | Add-Content -LiteralPath $log -Encoding UTF8
    Write-Error $_ -ErrorAction Continue
    exit 1
}
