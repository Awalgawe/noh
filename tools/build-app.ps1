param(
    [switch]$NoBundle,
    [switch]$Offline,
    [string]$FfmpegPath = $env:NOH_FFMPEG
)
$ErrorActionPreference = 'Stop'
& (Join-Path $PSScriptRoot 'build.ps1') -Gui -Offline:$Offline -NoBundle:$NoBundle -FfmpegPath $FfmpegPath
