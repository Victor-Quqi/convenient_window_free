param([Parameter(Mandatory=$true)][string]$OutputDir)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
. (Join-Path $PSScriptRoot '../../scripts/windows-toolchain.ps1')
if ($env:VSINSTALLDIR) { $env:VSINSTALLDIR = $env:VSINSTALLDIR.TrimEnd('\') + '\' }
Initialize-MsvcEnvironment
$out = [IO.Path]::GetFullPath($OutputDir)
New-Item -ItemType Directory -Path $out -Force | Out-Null
$source = Join-Path $PSScriptRoot 'taskbar-appearance.cpp'
$previous = $ErrorActionPreference
$ErrorActionPreference = 'Continue'
try {
  & cl.exe /nologo /std:c++20 /EHsc /O2 /MT /LD /DUNICODE /D_UNICODE /DNOMINMAX "/Fo$(Join-Path $out 'taskbar-appearance.obj')" $source /link "/DEF:$(Join-Path $PSScriptRoot 'taskbar-appearance.def')" "/OUT:$(Join-Path $out 'taskbar-appearance.dll')" "/IMPLIB:$(Join-Path $out 'taskbar-appearance.lib')" user32.lib advapi32.lib ole32.lib runtimeobject.lib windowsapp.lib
  $exit = $LASTEXITCODE
} finally { $ErrorActionPreference = $previous }
if ($exit -ne 0) { throw "Native taskbar compile failed: $exit" }
