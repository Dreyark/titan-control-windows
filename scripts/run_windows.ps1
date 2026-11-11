# Builds and runs Titan Control on Windows through MSYS2 UCRT64 (GTK DLLs are found there).
# The default build is READ-ONLY (no ddc-write feature). Use -NoDdc to start without any access to the monitors.
param([switch]$NoDdc, [switch]$Release)
$repo = Split-Path -Parent $PSScriptRoot
$posix = '/' + $repo.Substring(0, 1).ToLower() + $repo.Substring(2).Replace('\', '/')
$flag = if ($Release) { '--release' } else { '' }
$bin = if ($Release) { 'release' } else { 'debug' }
$arg = if ($NoDdc) { '--no-ddc' } else { '' }
$env:MSYSTEM = 'UCRT64'
& C:\msys64\usr\bin\bash.exe -lc "cd '$posix' && cargo build $flag && ./target/$bin/titan_control.exe $arg"
