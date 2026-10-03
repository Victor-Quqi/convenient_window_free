param([string]$ComponentPath)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
. (Join-Path $PSScriptRoot '../../../scripts/windows-toolchain.ps1')
Initialize-MsvcEnvironment
$out = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../../target/taskbar-native-tests'))
New-Item -ItemType Directory -Force -Path $out | Out-Null
if (-not $ComponentPath) {
  & (Join-Path $PSScriptRoot '../build.ps1') -OutputDir $out
  $ComponentPath = Join-Path $out 'taskbar-appearance.dll'
}
$ComponentPath = (Resolve-Path -LiteralPath $ComponentPath).Path
$current = Join-Path $out 'taskbar-1111111111111111.dll'
$resident = Join-Path $out 'taskbar-2222222222222222.dll'
$legacy = Join-Path $out 'taskbar-0000000000000002.dll'
$incompatible = Join-Path $out 'taskbar-3333333333333333.dll'
Copy-Item -LiteralPath $ComponentPath -Destination $current -Force
Copy-Item -LiteralPath $ComponentPath -Destination $resident -Force
$source = Join-Path $PSScriptRoot 'taskbar-appearance-native-test.cpp'
$exe = Join-Path $out 'native-test.exe'
$previous = $ErrorActionPreference
$ErrorActionPreference = 'Continue'
try {
  & cl.exe /nologo /std:c++20 /EHsc /W4 /WX /MT /DNOMINMAX "/Fo$(Join-Path $out 'native-test.obj')" $source /link "/OUT:$exe" user32.lib advapi32.lib
  if ($LASTEXITCODE -ne 0) { throw "Native test compile failed: $LASTEXITCODE" }
  & cl.exe /nologo /std:c++20 /EHsc /W4 /WX /MT /LD /DNOMINMAX /DCW_TASKBAR_LEGACY_FIXTURE "/Fo$(Join-Path $out 'legacy.obj')" $source /link /EXPORT:DllGetClassObject,PRIVATE "/OUT:$legacy" "/IMPLIB:$(Join-Path $out 'legacy.lib')" user32.lib advapi32.lib
  if ($LASTEXITCODE -ne 0) { throw "Legacy fixture compile failed: $LASTEXITCODE" }
  & cl.exe /nologo /std:c++20 /EHsc /W4 /WX /MT /LD /DNOMINMAX /DCW_TASKBAR_LEGACY_FIXTURE /DCW_TASKBAR_INCOMPATIBLE_FIXTURE "/Fo$(Join-Path $out 'incompatible.obj')" $source /link /EXPORT:DllGetClassObject,PRIVATE "/OUT:$incompatible" "/IMPLIB:$(Join-Path $out 'incompatible.lib')" user32.lib advapi32.lib
  if ($LASTEXITCODE -ne 0) { throw "Incompatible fixture compile failed: $LASTEXITCODE" }
  foreach ($case in @(@($resident,'reuse'),@($legacy,'reuse'),@($resident,'stale'),@($resident,'conflict'),@($incompatible,'incompatible'))) {
    & $exe $current $case[0] $case[1]
    if ($LASTEXITCODE -ne 0) { throw "Native taskbar regression failed: $($case[1]) / $($case[0])" }
  }
} finally { $ErrorActionPreference = $previous }
Write-Output 'taskbar native regressions: 5 passed; no Explorer injection'
