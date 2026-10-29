# Native smoke test: a Rune overlay can be shown, hidden, and shown again.
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Push-Location $root
try {
    cargo build --locked -p background
    if ($LASTEXITCODE -ne 0) { throw 'Building background failed' }

    Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
using System.Text;
public static class OverlaySmokeWindows {
    public delegate bool EnumProc(IntPtr hwnd, IntPtr param);
    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc proc, IntPtr param);
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr hwnd, out uint pid);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr hwnd);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetWindowText(IntPtr hwnd, StringBuilder text, int size);
    public static IntPtr Find(uint pid, string title) {
        IntPtr found = IntPtr.Zero;
        EnumWindows((hwnd, param) => {
            uint owner; GetWindowThreadProcessId(hwnd, out owner);
            var text = new StringBuilder(256); GetWindowText(hwnd, text, text.Capacity);
            if (owner == pid && text.ToString() == title) { found = hwnd; return false; }
            return true;
        }, IntPtr.Zero);
        return found;
    }
}
'@
    $start = New-Object System.Diagnostics.ProcessStartInfo
    $target = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $root 'target' }
    $start.FileName = Join-Path $target 'debug\background.exe'
    $start.UseShellExecute = $false
    $start.RedirectStandardInput = $true
    $process = [System.Diagnostics.Process]::Start($start)
    try {
        function Wait-Visibility([bool] $visible) {
            $deadline = [DateTime]::UtcNow.AddSeconds(20)
            do {
                if ($process.HasExited) { throw 'Background example exited early' }
                $hwnd = [OverlaySmokeWindows]::Find([uint32]$process.Id, 'background-hud')
                if ($hwnd -ne [IntPtr]::Zero -and [OverlaySmokeWindows]::IsWindowVisible($hwnd) -eq $visible) { return }
                Start-Sleep -Milliseconds 50
            } while ([DateTime]::UtcNow -lt $deadline)
            throw "Overlay did not become visible=$visible"
        }
        foreach ($command in @('show', 'hide', 'show', 'hide')) {
            $process.StandardInput.WriteLine("hud $command")
            $process.StandardInput.Flush()
            Wait-Visibility ($command -eq 'show')
        }
        Write-Host 'Windows overlay show/hide cycles passed'
    } finally {
        if (-not $process.HasExited) {
            $process.StandardInput.WriteLine('quit')
            $process.StandardInput.Flush()
            if (-not $process.WaitForExit(5000)) { $process.Kill() }
        }
        $process.Dispose()
    }
} finally { Pop-Location }
