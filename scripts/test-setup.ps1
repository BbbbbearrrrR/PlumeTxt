param([Parameter(Mandatory = $true)][string]$SetupPath)
$ErrorActionPreference = 'Stop'
$SetupPath = (Resolve-Path -LiteralPath $SetupPath).Path
if (-not ('SetupSmokeWindow' -as [type])) {
    Add-Type @'
using System;
using System.Runtime.InteropServices;
using System.Text;
public static class SetupSmokeWindow {
    delegate bool EnumProc(IntPtr window, IntPtr parameter);
    [DllImport("user32.dll")] static extern bool EnumWindows(EnumProc callback, IntPtr parameter);
    [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr window, out uint pid);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] static extern int GetClassName(IntPtr window, StringBuilder name, int length);
    [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr window, uint message, IntPtr wp, IntPtr lp);
    public static IntPtr Find(uint pid) {
        IntPtr result = IntPtr.Zero;
        EnumWindows((window, parameter) => {
            uint owner; GetWindowThreadProcessId(window, out owner);
            var name = new StringBuilder(128); GetClassName(window, name, name.Capacity);
            if (owner == pid && name.ToString() == "PlumeTxtWindow") { result = window; return false; }
            return true;
        }, IntPtr.Zero);
        return result;
    }
}
'@
}
$repo = Split-Path $PSScriptRoot -Parent
$uninstallKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\{89803657-528F-439F-944A-3C754712938E}_is1'
$shortcut = Join-Path ([Environment]::GetFolderPath('Programs')) 'PlumeTxt.lnk'
if ((Test-Path -LiteralPath $uninstallKey) -or (Test-Path -LiteralPath $shortcut)) {
    throw 'Use a test Windows account without an installed PlumeTxt or PlumeTxt Start menu shortcut.'
}
$scratch = Join-Path $repo ('tmp/setup-smoke-' + [guid]::NewGuid().ToString('N'))
$destination = Join-Path $scratch '安装 测试'
[void](New-Item -ItemType Directory -Path $scratch)
function Run-Installer($exe, $arguments) {
    $process = Start-Process -FilePath $exe -ArgumentList $arguments -WindowStyle Hidden -PassThru
    if (-not $process.WaitForExit(30000)) { throw 'Installer timed out; inspect the log before retrying.' }
    if ($process.ExitCode -ne 0) { throw "Installer returned $($process.ExitCode)." }
}
$uninstaller = Join-Path $destination 'unins000.exe'
try {
    foreach ($pass in 1..2) {
        Run-Installer $SetupPath @('/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', '/TASKS=""', '/LANG=chinesesimplified', ('/DIR="' + $destination + '"'), ('/LOG="' + $scratch + "/install-$pass.log" + '"'))
        foreach ($file in @('PlumeTxt.exe', 'LICENSE', 'runtime/pdfium/pdfium.dll', 'runtime/pdfium/LICENSE', 'runtime/pdfium/licenses/pdfium.txt', 'unins000.exe')) {
            if (-not (Test-Path -LiteralPath (Join-Path $destination $file))) { throw "Missing installed file: $file" }
        }
        if (-not (Test-Path -LiteralPath $uninstallKey)) { throw 'Missing Windows uninstall entry.' }
        $link = (New-Object -ComObject WScript.Shell).CreateShortcut($shortcut)
        if ($link.TargetPath -ne (Join-Path $destination 'PlumeTxt.exe')) { throw 'Start menu target is incorrect.' }
        $marker = Join-Path $destination 'user-document.txt'
        if ($pass -eq 1) { 'Keep this user document.' | Set-Content -LiteralPath $marker }
        if ((Get-Content -LiteralPath $marker) -ne 'Keep this user document.') { throw 'Upgrade changed user data.' }
    }
    $app = Start-Process -FilePath (Join-Path $destination 'PlumeTxt.exe') -ArgumentList ('"' + $marker + '"') -WindowStyle Hidden -PassThru
    try {
        $deadline = [DateTime]::UtcNow.AddSeconds(10)
        do {
            Start-Sleep -Milliseconds 100
            $app.Refresh()
            $window = [SetupSmokeWindow]::Find($app.Id)
        } while (-not $app.HasExited -and $window -eq [IntPtr]::Zero -and [DateTime]::UtcNow -lt $deadline)
        if ($app.HasExited -or $window -eq [IntPtr]::Zero) { throw 'Installed application failed to create its window.' }
    } finally {
        if (-not $app.HasExited) {
            [void][SetupSmokeWindow]::PostMessage([SetupSmokeWindow]::Find($app.Id), 0x0010, [IntPtr]::Zero, [IntPtr]::Zero)
            if (-not $app.WaitForExit(10000)) { throw 'Test application did not close normally.' }
        }
    }
} finally {
    if (Test-Path -LiteralPath $uninstaller) {
        Run-Installer $uninstaller @('/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', ('/LOG="' + $scratch + '/uninstall.log"'))
    }
}
if ((Test-Path -LiteralPath $uninstallKey) -or (Test-Path -LiteralPath $shortcut) -or (Test-Path -LiteralPath (Join-Path $destination 'PlumeTxt.exe'))) {
    throw 'Uninstall left application files or registration behind.'
}
if ((Get-Content -LiteralPath $marker) -ne 'Keep this user document.') { throw 'Uninstall removed user data.' }
Write-Output "PASS: Chinese/spaced install path, upgrade, launch, shortcuts, uninstall, user-data preservation. Logs: $scratch"

