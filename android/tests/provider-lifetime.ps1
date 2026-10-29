param(
    [Parameter(Mandatory = $true)][string]$Serial,
    [string]$Sdk = "$env:LOCALAPPDATA\Android\Sdk",
    [string]$Apk = "$PSScriptRoot\..\app\build\outputs\apk\debug\app-debug.apk",
    [int]$Rounds = 500
)
$ErrorActionPreference = 'Stop'
function Invoke-Checked([string]$Exe, [string[]]$Arguments) {
    & $Exe @Arguments
    if ($LASTEXITCODE -ne 0) { throw "$Exe failed with exit code $LASTEXITCODE" }
}
$out = [IO.Path]::GetFullPath("$PSScriptRoot\..\build\provider-probe")
New-Item -ItemType Directory -Force $out | Out-Null
$platform = Get-ChildItem "$Sdk\platforms" -Directory | Sort-Object Name -Descending | Select-Object -First 1
$buildTools = Get-ChildItem "$Sdk\build-tools" -Directory | Sort-Object Name -Descending | Select-Object -First 1
$adb = "$Sdk\platform-tools\adb.exe"
Invoke-Checked "$env:JAVA_HOME\bin\javac.exe" @('-encoding', 'UTF-8', '-cp', "$($platform.FullName)\android.jar", '-d', $out, "$PSScriptRoot\ProviderLifetimeProbe.java")
Invoke-Checked "$($buildTools.FullName)\d8.bat" @('--min-api', '29', '--output', $out, "$out\ProviderLifetimeProbe.class")
Invoke-Checked $adb @('-s', $Serial, 'push', "$out\classes.dex", '/data/local/tmp/syncboard-provider-probe.dex')
Invoke-Checked $adb @('-s', $Serial, 'push', $Apk, '/data/local/tmp/syncboard-provider-probe.apk')
Invoke-Checked $adb @('-s', $Serial, 'shell', "CLASSPATH=/data/local/tmp/syncboard-provider-probe.apk:/data/local/tmp/syncboard-provider-probe.dex app_process /system/bin ProviderLifetimeProbe $Rounds")
