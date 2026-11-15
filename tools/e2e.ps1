# End-to-end test of the editor (0.1.37): the edit model in node writes scenarios, then
# glint.exe --e2e-test exports and checks them. Glint never starts node; this script runs the two.
#   powershell -File tools\e2e.ps1 -Exe <path\to\glint.exe> [-Work <folder>] [-Model <editmodel.js>]
# -Model plays the scenarios on another copy of ui/editmodel.js (an older release).
param([Parameter(Mandatory = $true)][string]$Exe, [string]$Work = "", [string]$Model = "")
$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
if (-not $Work) { $Work = Join-Path ([IO.Path]::GetTempPath()) ("glint-e2e-" + (Get-Date -Format "yyyyMMdd-HHmmss")) }
$scen = Join-Path $Work "scenarios"
$data = Join-Path $Work "data"
New-Item -ItemType Directory -Force $scen, $data | Out-Null
# The 0.1.36 model, for the scenario that loads a state 0.1.36 saved (a read-only git command).
$old = Join-Path $Work "editmodel-0136.js"
Push-Location $root
try { [IO.File]::WriteAllText($old, ((git show 1b36be8:ui/editmodel.js) -join "`n") + "`n", (New-Object Text.UTF8Encoding $false)) } finally { Pop-Location }
if ($Model) { $env:E2E_MODEL = $Model } else { Remove-Item Env:E2E_MODEL -ErrorAction SilentlyContinue }
node (Join-Path $root "tools\e2e-scenarios.js") $scen $old
if ($LASTEXITCODE -ne 0) { "node failed ($LASTEXITCODE)"; exit 1 }
Remove-Item Env:E2E_MODEL -ErrorAction SilentlyContinue
$report = Join-Path $Work "e2e.txt"
Remove-Item $report -ErrorAction SilentlyContinue
$env:GLINT_DATA_DIR = $data
$env:GLINT_E2E_DIR = $scen
$t = Get-Date
Start-Process $Exe -ArgumentList "--e2e-test", $report -WindowStyle Hidden -Wait
$secs = ((Get-Date) - $t).TotalSeconds
$lines = if (Test-Path $report) { Get-Content $report } else { @() }
$pass = ($lines | Where-Object { $_ -like "PASS*" }).Count
$fail = ($lines | Where-Object { $_ -like "FAIL*" }).Count
if (-not $lines.Count) { $fail = 1; "no report was written" }
"--e2e-test: pass={0} fail={1} ({2:N0} s) report {3}" -f $pass, $fail, $secs, $report
$lines | Where-Object { $_ -like "FAIL*" }
if ($fail -gt 0) { exit 1 }
