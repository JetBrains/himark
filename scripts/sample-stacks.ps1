param(
    [int]$DurationSec = 30,
    [int]$IntervalMs = 100,
    [int]$SnapshotTimeoutSec = 60
)

$repoRoot = Split-Path $PSScriptRoot -Parent

$pkg = Get-AppxPackage Microsoft.WinDbg -ErrorAction Stop
$cdb = Join-Path $pkg.InstallLocation 'amd64\cdb.exe'
if (-not (Test-Path $cdb)) { throw "cdb.exe not found at $cdb" }

Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class ThreadResume {
    [DllImport("kernel32.dll", SetLastError = true)] static extern IntPtr OpenThread(uint access, bool inherit, uint tid);
    [DllImport("kernel32.dll", SetLastError = true)] static extern int ResumeThread(IntPtr hThread);
    [DllImport("kernel32.dll")] static extern bool CloseHandle(IntPtr h);
    // Decrement the suspend count until the thread runs. Returns decrements done.
    public static int ResumeAll(uint tid) {
        IntPtr h = OpenThread(0x0002 /*THREAD_SUSPEND_RESUME*/, false, tid);
        if (h == IntPtr.Zero) return -1;
        int n = 0;
        while (true) {
            int prev = ResumeThread(h);
            if (prev <= 0) break;   // 0 = was not suspended; -1 = error
            n++;
            if (prev == 1) break;   // was 1, now 0 -> running
        }
        CloseHandle(h);
        return n;
    }
}
'@

function Resume-AppThreads([System.Diagnostics.Process]$proc) {
    try { $proc.Refresh() } catch { return }
    if ($proc.HasExited) { return }
    $resumed = 0
    foreach ($t in $proc.Threads) {
        $n = [ThreadResume]::ResumeAll([uint32]$t.Id)
        if ($n -gt 0) { $resumed += $n }
    }
    if ($resumed -gt 0) { Write-Host "Resumed $resumed suspended thread(s)." }
}

$outDir = Join-Path $repoRoot 'target\profile-samples'
if (Test-Path $outDir) { Remove-Item -Recurse -Force $outDir }
New-Item -ItemType Directory -Force $outDir | Out-Null

Write-Host 'Waiting for windows.exe (himark)...'
do {
    $p = Get-Process windows -ErrorAction SilentlyContinue |
        Where-Object { $_.Path -like "$repoRoot\target\*" } |
        Select-Object -First 1
    if (-not $p) { Start-Sleep -Milliseconds 300 }
} until ($p)

Resume-AppThreads $p

$symPath = "srv*$env:TEMP\symcache*https://msdl.microsoft.com/download/symbols;$(Split-Path $p.Path)"

Write-Host "Sampling PID $($p.Id) for up to $DurationSec s -> $outDir"
Write-Host 'Reproduce the freeze now. Ctrl+C to stop early.'
$deadline = (Get-Date).AddSeconds($DurationSec)
$i = 0
try {
    while ((Get-Date) -lt $deadline) {
        $p.Refresh()
        if ($p.HasExited) { Write-Host 'App exited.'; break }
        $i++
        $file = Join-Path $outDir ('{0:d3}.txt' -f $i)
        $threadCpu = ($p.Threads | ForEach-Object { '{0}:{1:f3}' -f $_.Id, $_.TotalProcessorTime.TotalSeconds }) -join ' '
        "sample=$i time=$(Get-Date -Format 'HH:mm:ss.fff') procCpu=$($p.CPU)`nthreadCpu: $threadCpu" |
            Out-File -Encoding utf8 $file
        $stdout = "$file.cdb"
        $args = @('-pv', '-p', $p.Id, '-lines', '-y', "`"$symPath`"", '-c', '"~*k 60; qd"')
        $snap = Start-Process -FilePath $cdb -ArgumentList $args -WindowStyle Hidden -PassThru -RedirectStandardOutput $stdout
        if (-not $snap.WaitForExit($SnapshotTimeoutSec * 1000)) {
            Write-Warning "cdb timed out; killing it and resuming the app."
            try { $snap.Kill() } catch {}
            Resume-AppThreads $p
        }
        if (Test-Path $stdout) {
            Get-Content $stdout | Out-File -Encoding utf8 -Append $file
            Remove-Item $stdout
        }
        Start-Sleep -Milliseconds $IntervalMs
    }
} finally {
    Resume-AppThreads $p
    Write-Host "Done: $i samples in $outDir"
}
