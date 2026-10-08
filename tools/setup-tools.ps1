# Pinned compiler and official Setup producer; no app installation occurs here.
[CmdletBinding()]
param([Parameter(Mandatory)][string]$Directory)
$ErrorActionPreference = 'Stop'
if (-not $IsWindows) { throw 'Windows is required' }
$Directory = [IO.Path]::GetFullPath($Directory)
[void][IO.Directory]::CreateDirectory($Directory)
$lock = Get-Content "$PSScriptRoot/../assets/setup-tools.lock.json" -Raw | ConvertFrom-Json
foreach ($name in @('compiler','packager','dotnet')) {
    $spec = $lock.$name
    $archive = Join-Path $Directory $spec.name
    if (-not (Test-Path -LiteralPath $archive)) {
        Invoke-WebRequest -Uri $spec.url -OutFile "$archive.partial"
        if ((Get-FileHash -LiteralPath "$archive.partial" -Algorithm $spec.algorithm).Hash.ToLowerInvariant() -ne $spec.hash) { throw "Downloaded $name hash mismatch" }
        Move-Item -LiteralPath "$archive.partial" -Destination $archive
    }
    if ((Get-FileHash -LiteralPath $archive -Algorithm $spec.algorithm).Hash.ToLowerInvariant() -ne $spec.hash) { throw "Cached $name hash mismatch" }
    $destination = switch ($name) { compiler { $Directory }; packager { "$Directory/vpk" }; dotnet { "$Directory/dotnet8" } }
    $marker = "$Directory/$name.extracted"
    if (-not (Test-Path -LiteralPath $marker)) {
        Expand-Archive -LiteralPath $archive -DestinationPath $destination -Force
        [IO.File]::WriteAllText($marker, $spec.hash)
    }
}
$compiler = "$Directory/llvm-mingw-20260922-ucrt-x86_64/bin"
foreach ($path in @("$compiler/clang.exe", "$compiler/llvm-ar.exe", "$Directory/dotnet8/dotnet.exe", "$Directory/vpk/tools/net8.0/any/vpk.dll")) {
    if (-not (Test-Path -LiteralPath $path -PathType Leaf)) { throw "Missing setup tool: $path" }
}
[pscustomobject]@{directory=$Directory;compiler=$compiler}
