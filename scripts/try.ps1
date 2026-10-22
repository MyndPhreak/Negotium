# Builds the release exe and copies it to try\negotium.exe, the one copy to run when testing a
# change. The copy is what runs, so target\release\negotium.exe is never locked and the next build
# can replace it. try\build.txt says which branch and commit the copy was built from.
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
Set-Location $root

cargo build --release -p negotium
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

$dir = Join-Path $root 'try'
$exe = Join-Path $dir 'negotium.exe'
New-Item -ItemType Directory -Force $dir | Out-Null
$running = Get-Process negotium -ErrorAction SilentlyContinue | Where-Object { $_.Path -eq $exe }
if ($running) {
    Write-Error "try\negotium.exe is running (PID $($running.Id -join ', ')). Close it, then run this again."
}
Copy-Item (Join-Path $root 'target\release\negotium.exe') $exe -Force

$branch = git rev-parse --abbrev-ref HEAD
$commit = git log -1 --format='%h %s'
$dirty = if (git status --porcelain --untracked-files=no) { ' + uncommitted changes' } else { '' }
@"
Branch: $branch
Commit: $commit$dirty
Built:  $(Get-Date -Format 'yyyy-MM-dd HH:mm')
"@ | Set-Content (Join-Path $dir 'build.txt')

Write-Host "Ready: $exe ($branch, $commit$dirty)"
