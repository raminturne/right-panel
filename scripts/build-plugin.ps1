param([Parameter(Mandatory=$true)][string]$PluginDirectory)
$source = Resolve-Path $PluginDirectory
if (!(Test-Path "$source\manifest.json")) { throw 'manifest.json is required.' }
$manifest = Get-Content "$source\manifest.json" -Raw | ConvertFrom-Json
if (!$manifest.id -or !$manifest.version) { throw 'manifest.json must include id and version.' }
$out = Join-Path (Split-Path $PSScriptRoot -Parent) "dist\plugins"
New-Item -ItemType Directory -Force $out | Out-Null
Compress-Archive -Path "$source\*" -DestinationPath (Join-Path $out "$($manifest.id)-$($manifest.version).rpp") -Force
