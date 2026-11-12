# Build Glint and publish it as a GitHub release.
#
#   .\release.ps1                 build, package into dist\release, and publish v<version>
#   .\release.ps1 -NoPublish      build and package only
#   .\release.ps1 -Notes "text"   release notes shown on GitHub and in Glint's update prompt
#
# The version comes from src-tauri\Cargo.toml. To ship an update: raise `version`
# there (and in src-tauri\tauri.conf.json), commit, push, then run this script.
# Every installed Glint finds the new release within a day, or at once when its
# update arrow is clicked.
#
# Needs: Rust (GNU toolchain), and the GitHub CLI (`gh`) signed in with access to the repo.
param(
    [switch]$NoPublish,
    [string]$Notes = "",
    [string]$Repo = "swamie/glint"
)
$ErrorActionPreference = "Stop"
$root = $PSScriptRoot
$version = (Select-String -Path "$root\src-tauri\Cargo.toml" -Pattern '^version\s*=\s*"(.+)"').Matches[0].Groups[1].Value
$tag = "v$version"
Write-Host "Glint $version"

Push-Location "$root\src-tauri"
try { cargo build --release; if ($LASTEXITCODE -ne 0) { throw "The build failed." } } finally { Pop-Location }

$target = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { "$root\src-tauri\target" }
$bin = Join-Path $target "release"
$out = "$root\dist\release"
Remove-Item $out -Recurse -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory "$out\pkg" -Force | Out-Null

# The app: Glint.exe needs WebView2Loader.dll beside it (GNU toolchain builds load it at run time).
Copy-Item "$bin\glint.exe" "$out\pkg\Glint.exe"
Copy-Item "$bin\WebView2Loader.dll" "$out\pkg\WebView2Loader.dll"
$zip = "$out\Glint-windows-x64.zip"
Compress-Archive -Path "$out\pkg\Glint.exe", "$out\pkg\WebView2Loader.dll" -DestinationPath $zip
# Checked by Glint's updater before it installs the download.
(Get-FileHash $zip -Algorithm SHA256).Hash.ToLower() | Set-Content "$zip.sha256" -NoNewline -Encoding ascii
# The one-time installer.
Copy-Item "$bin\glint-setup.exe" "$out\GlintSetup.exe"
Remove-Item "$out\pkg" -Recurse -Force

Get-ChildItem $out | ForEach-Object { "{0,-32} {1,8:N1} MB" -f $_.Name, ($_.Length / 1MB) }
if ($NoPublish) { Write-Host "Packaged in $out (not published)."; return }

if (-not $Notes) { $Notes = "Glint $version" }
gh release create $tag "$out\GlintSetup.exe" $zip "$zip.sha256" --repo $Repo --title "Glint $version" --notes $Notes
if ($LASTEXITCODE -ne 0) { throw "Publishing failed. If $tag already exists, raise the version first." }
Write-Host "Published https://github.com/$Repo/releases/tag/$tag"
