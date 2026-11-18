# Install (or remove) the Claude plugin for Glint.
#
#   .\install.ps1                      build glint-mcp.exe, copy the plugin into Glint, add it to Claude Desktop
#   .\install.ps1 -Transcripts         also download speech to text (whisper.cpp + the base.en model, about 150 MB)
#   .\install.ps1 -Model small.en      ... with another model (tiny.en, base.en, small.en, small, medium ...)
#   .\install.ps1 -NoClaudeDesktop     do not touch Claude Desktop's config (use Claude Code instead)
#   .\install.ps1 -Exe path\to\glint-mcp.exe   use a ready-made exe instead of building one
#   .\install.ps1 -Uninstall           remove the plugin (your shared-folder list and transcripts go with it)
#
# Glint loads the plugin when it starts: restart Glint afterwards (tray icon > Quit, then start it again).
param(
    [switch]$Transcripts,
    [string]$Model = "base.en",
    [switch]$NoClaudeDesktop,
    [string]$Exe = "",
    [switch]$Uninstall
)
$ErrorActionPreference = "Stop"
$here = $PSScriptRoot
$dataDir = if ($env:GLINT_DATA_DIR) { $env:GLINT_DATA_DIR } else { Join-Path $env:APPDATA "Glint" }
$dest = Join-Path $dataDir "plugins\claude-mcp"

if ($Uninstall) {
    Remove-Item $dest -Recurse -Force -ErrorAction SilentlyContinue
    # Claude Desktop's config: the classic app's, and the Store app's (Claude-Data or its package's LocalCache).
    $cfgs = @(Join-Path $env:APPDATA "Claude\claude_desktop_config.json")
    $cfgs += Get-ChildItem (Join-Path $env:LOCALAPPDATA "Claude-Data\claude_*\claude_desktop_config.json") -ErrorAction SilentlyContinue | ForEach-Object FullName
    $cfgs += Get-ChildItem (Join-Path $env:LOCALAPPDATA "Packages\Claude_*\LocalCache\Roaming\Claude\claude_desktop_config.json") -ErrorAction SilentlyContinue | ForEach-Object FullName
    foreach ($cfg in $cfgs) {
    if (Test-Path $cfg) {
        $j = Get-Content $cfg -Raw | ConvertFrom-Json
        if ($j.mcpServers -and $j.mcpServers.PSObject.Properties["glint"]) {
            $j.mcpServers.PSObject.Properties.Remove("glint")
            $j | ConvertTo-Json -Depth 20 | Set-Content $cfg -Encoding UTF8
        }
    }
    }
    Write-Host "Removed. Restart Glint and Claude Desktop."
    return
}

if (-not $Exe) {
    Push-Location (Join-Path $here "server")
    try { cargo build --release; if ($LASTEXITCODE -ne 0) { throw "The build failed." } } finally { Pop-Location }
    $target = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $here "server\target" }
    $Exe = Join-Path $target "release\glint-mcp.exe"
}
if (-not (Test-Path $Exe)) { throw "glint-mcp.exe not found at $Exe" }

New-Item -ItemType Directory -Force $dest | Out-Null
Copy-Item (Join-Path $here "plugin.json"), (Join-Path $here "bridge.js") $dest -Force
Copy-Item $Exe (Join-Path $dest "glint-mcp.exe") -Force
$mcp = Join-Path $dest "glint-mcp.exe"
Write-Host "Installed into $dest"

if (-not $NoClaudeDesktop) { & $mcp install-claude }
if ($Transcripts) { & $mcp setup-transcripts $Model }

Write-Host ""
Write-Host "Next:"
Write-Host "  1. Restart Glint (tray icon > Quit, then start it again)."
Write-Host "  2. Restart Claude Desktop. Or for Claude Code: claude mcp add glint -- `"$mcp`""
Write-Host "  3. Share folders Claude may read:  & `"$mcp`" grant `"D:\Assignment`" --note `"what is in it`""
Write-Host "     Add --write for a folder Claude may copy or save files into."
