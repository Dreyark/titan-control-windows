# Smoke test of a Windows package on a machine that has no GTK: starts the packaged executable with a clean PATH
# (only the Windows folders, no MSYS2) in safe mode (--no-ddc: no monitor is touched) and checks its log.
#   .\scripts\smoke_windows.ps1 -Dir dist\TitanControl-1.0.1-windows-x64
param([Parameter(Mandatory = $true)][string]$Dir)
$ErrorActionPreference = 'Stop'
$exe = Join-Path (Resolve-Path $Dir) 'bin\titan_control.exe'
if (-not (Test-Path $exe)) { throw "not found: $exe" }
$log = Join-Path $env:LOCALAPPDATA 'titan_control\logs\titan_control.log'
$before = if (Test-Path $log) { (Get-Item $log).Length } else { 0 }

$clean = "$env:SystemRoot\System32;$env:SystemRoot"
$info = New-Object System.Diagnostics.ProcessStartInfo
$info.FileName = $exe
$info.Arguments = '--no-ddc'
$info.UseShellExecute = $false
$info.EnvironmentVariables['PATH'] = $clean
$info.EnvironmentVariables.Remove('MSYSTEM') | Out-Null
# Its own single-instance lock: the test must not meet an app that is open (it would only ask that one to show).
$info.EnvironmentVariables['TITAN_INSTANCE_ID'] = 'smoke'
$process = [System.Diagnostics.Process]::Start($info)
if (-not $process.WaitForExit(30000)) { $process.Kill(); throw 'the app did not finish by itself within 30 s' }
$exit = $process.ExitCode

Start-Sleep -Milliseconds 300
$text = if (Test-Path $log) { Get-Content $log -Tail 40 -Encoding utf8 | Out-String } else { '' }
Write-Host $text
$ok = ($exit -eq 0) -and ($text -match 'GTK \d+\.\d+') -and ($text -match 'Safe mode')
if ($ok) { Write-Host "SMOKE TEST OK (exit code $exit)" } else { Write-Host "SMOKE TEST FAILED (exit code $exit)"; exit 1 }
