# Install the pinned official compiler on the disposable CI runner.
[CmdletBinding()]
param([Parameter(Mandatory)][string]$Directory)
. "$PSScriptRoot/update-qualification/common.ps1"
$Directory = Assert-PlainPath $Directory
[void][IO.Directory]::CreateDirectory($Directory)
$archive = "$Directory/innosetup-7.1.0-x64.exe"
$expected = '0362a383ed217d4c4239b5933866dd96d3eb2102737da92f80f6057a4b40df2f'
if (-not (Test-Path -LiteralPath $archive)) {
    Invoke-WebRequest 'https://github.com/jrsoftware/issrc/releases/download/is-7_1_0/innosetup-7.1.0-x64.exe' -OutFile "$archive.partial"
    if ((Get-FileHash -LiteralPath "$archive.partial").Hash.ToLowerInvariant() -ne $expected) { throw 'Inno Setup download digest differs' }
    Move-Item -LiteralPath "$archive.partial" -Destination $archive
}
if ((Get-FileHash -LiteralPath $archive).Hash.ToLowerInvariant() -ne $expected) { throw 'Cached Inno Setup archive digest differs' }
if ((Get-AuthenticodeSignature -FilePath $archive).Status -ne 'Valid') { throw 'Inno Setup archive signature is invalid' }
$destination = "$Directory/compiler"
Invoke-RecordedProcess $archive @('/VERYSILENT','/SUPPRESSMSGBOXES','/NORESTART','/CURRENTUSER','/NOICONS',"/DIR=$destination") $Directory "$Directory/install" 180
# The existing wrapper builder also checks compiler pins and Authenticode.
Write-Output $destination
