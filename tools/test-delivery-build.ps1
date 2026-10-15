$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$tokens = $null
$errors = $null
$ast = [System.Management.Automation.Language.Parser]::ParseFile(
    (Join-Path $PSScriptRoot 'delivery-build.ps1'), [ref]$tokens, [ref]$errors)
if ($errors.Count) { throw 'Delivery script did not parse' }
# Exercise only the Linux input-selection branch, never the hosted build runner.
$blocks = @($ast.FindAll({ param($node)
    $node -is [System.Management.Automation.Language.StatementBlockAst] -and
    $node.Extent.Text.Contains("'dist/NOH-linux-x64'") -and
    -not $node.Extent.Text.Contains('rustup')
}, $true))
if ($blocks.Count -ne 1) { throw 'Expected one Linux input-selection branch' }
$block = $blocks[0].Extent.Text
$selectInputs = [scriptblock]::Create($block.Substring(1, $block.Length - 2))
$temporary = Join-Path ([System.IO.Path]::GetTempPath()) ('noh-delivery-path-' + [guid]::NewGuid())
$inputs = Join-Path $temporary 'inputs'
$materials = Join-Path $inputs 'materials'
$previousMpv = $env:NOH_LIBMPV
try {
    New-Item -ItemType Directory -Path $materials -Force | Out-Null
    @{ffmpeg='/usr/bin/ffmpeg'} | ConvertTo-Json | Set-Content (Join-Path $materials 'HOST-RUNTIME.json')
    function Get-Command {
        param($Name, $CommandType)
        @([pscustomobject]@{Source='/usr/bin/ffmpeg'}, [pscustomobject]@{Source='/bin/ffmpeg'})
    }
    . $selectInputs
    if ($ffmpeg -isnot [string] -or $ffmpeg -ne '/usr/bin/ffmpeg') {
        throw 'The Linux build must use one FFmpeg path from the verified host record'
    }
    $arguments = @('--ffmpeg', $ffmpeg, '--speech', 'synthetic speech')
    function Capture-Arguments { $args | ConvertTo-Json -Compress }
    $captured = Capture-Arguments @arguments | ConvertFrom-Json
    if ($captured.Count -ne 4 -or $captured[1] -ne '/usr/bin/ffmpeg') {
        throw 'FFmpeg paths expanded into extra command arguments'
    }
    Write-Output 'Linux delivery uses the single verified FFmpeg path despite duplicate PATH matches.'
} finally {
    $env:NOH_LIBMPV = $previousMpv
    $resolved = [System.IO.Path]::GetFullPath($temporary)
    $tempRoot = [System.IO.Path]::GetFullPath([System.IO.Path]::GetTempPath())
    if (-not $resolved.StartsWith($tempRoot) -or -not ([System.IO.Path]::GetFileName($resolved)).StartsWith('noh-delivery-path-')) {
        throw 'Unexpected test cleanup path'
    }
    if (Test-Path -LiteralPath $resolved) { Remove-Item -LiteralPath $resolved -Recurse -Force }
}
