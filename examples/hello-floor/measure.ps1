<#
Prints the private working set of each step of the floor: the number Task Manager shows as "Memory".

  pwsh examples/hello-floor/measure.ps1                     every step, on the adapter Rune picks
  pwsh examples/hello-floor/measure.ps1 -Adapter nvidia     ... on the adapter whose name holds "nvidia"
  pwsh examples/hello-floor/measure.ps1 -Settle 12 -NoBuild

One window opens at a time, stays -Settle seconds, is read from outside and closed. Windows only.
#>
param(
    [string]$Adapter = '',
    [int]$Settle = 8,
    [switch]$NoBuild,
    [string]$Bin = ''
)
$ErrorActionPreference = 'Stop'
if (-not $IsWindows) { 'measure.ps1 reads Windows counters. Elsewhere: cargo run --release -p hello-floor, then ps -o rss= -p <pid>.'; exit 1 }
$repo = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
if (-not $Bin) { $Bin = Join-Path $repo 'target\release' }
if (-not $NoBuild) {
    cargo build --release -p hello-floor --manifest-path (Join-Path $repo 'Cargo.toml')
    if ($LASTEXITCODE) { exit $LASTEXITCODE }
}

$rows = @(
    @{ Step = 'bare: a window, filled by GDI'; Exe = 'hello-floor-bare'; Args = 'win32' }
    @{ Step = 'bare D3D12: device'; Exe = 'hello-floor-bare'; Args = 'd3d12 device' }
    @{ Step = 'bare D3D12: + command queue'; Exe = 'hello-floor-bare'; Args = 'd3d12 queue' }
    @{ Step = 'bare D3D12: + swap chain'; Exe = 'hello-floor-bare'; Args = 'd3d12 swapchain' }
    @{ Step = 'bare D3D12: + one frame'; Exe = 'hello-floor-bare'; Args = 'd3d12 frame' }
    @{ Step = 'bare D3D11: device'; Exe = 'hello-floor-bare'; Args = 'd3d11 device' }
    @{ Step = 'bare D3D11: + swap chain'; Exe = 'hello-floor-bare'; Args = 'd3d11 swapchain' }
    @{ Step = 'bare D3D11: + one frame'; Exe = 'hello-floor-bare'; Args = 'd3d11 frame' }
    @{ Step = 'Rune: an empty window'; Exe = 'hello-floor'; Env = @{ HELLO_FLOOR_STEP = 'empty' } }
    @{ Step = 'Rune: + a line of text'; Exe = 'hello-floor'; Env = @{ HELLO_FLOOR_STEP = 'text' } }
    @{ Step = 'Rune: + a backdrop blur'; Exe = 'hello-floor'; Env = @{ HELLO_FLOOR_STEP = 'effect' } }
    @{ Step = 'Rune: text, GPU blocks of 4 to 64 MiB'; Exe = 'hello-floor'; Env = @{ HELLO_FLOOR_STEP = 'text'; HELLO_FLOOR_MEMORY_POLICY = 'manual:4-64' } }
    @{ Step = 'Rune: text, no effect pipeline warmed'; Exe = 'hello-floor'; Env = @{ HELLO_FLOOR_STEP = 'text'; HELLO_FLOOR_WARM = 'none' } }
    @{ Step = 'Rune: text, on the CPU renderer'; Exe = 'hello-floor'; Env = @{ HELLO_FLOOR_STEP = 'text'; RUNE_CPU_FIRST = 'only' } }
)

$mb = { param($bytes) [math]::Round($bytes / 1MB, 1) }
$table = foreach ($row in $rows) {
    $exe = Join-Path $Bin "$($row.Exe).exe"
    if (-not (Test-Path $exe)) { throw "not built: $exe" }
    $start = [Diagnostics.ProcessStartInfo]::new($exe)
    $start.UseShellExecute = $false
    if ($row.Args) { $start.Arguments = $row.Args }
    foreach ($name in 'HELLO_FLOOR_STEP', 'HELLO_FLOOR_MEMORY_POLICY', 'HELLO_FLOOR_WARM', 'RUNE_CPU_FIRST') { $start.Environment.Remove($name) | Out-Null }
    if ($row.Env) { foreach ($pair in $row.Env.GetEnumerator()) { $start.Environment[$pair.Key] = $pair.Value } }
    if ($Adapter) { $start.Environment['WGPU_ADAPTER_NAME'] = $Adapter; $start.Environment['HELLO_FLOOR_ADAPTER'] = $Adapter }
    $process = [Diagnostics.Process]::Start($start)
    try {
        Start-Sleep -Seconds $Settle
        if ($process.HasExited) { throw "$($row.Step): the program ended (exit $($process.ExitCode))" }
        $counters = Get-CimInstance Win32_PerfRawData_PerfProc_Process -Filter "IDProcess = $($process.Id)"
        $line = [pscustomobject]@{
            Step = $row.Step
            'Private working set MB' = & $mb $counters.WorkingSetPrivate
            'Private bytes MB' = & $mb $counters.PrivateBytes
        }
        Write-Host ('{0,-40} {1,6:N1} MB' -f $line.Step, $line.'Private working set MB')
        $line
    } finally {
        if (-not $process.HasExited) {
            $process.CloseMainWindow() | Out-Null
            if (-not $process.WaitForExit(3000)) { $process.Kill() }
        }
    }
}
''
"Adapter: $(if ($Adapter) { $Adapter } else { 'the one Rune picks (RUNE_GPU_LOG=1 names it)' }); each read after $Settle s."
$table | Format-Table -AutoSize
