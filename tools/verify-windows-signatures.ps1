[CmdletBinding()]
param(
    [Parameter(Mandatory)][ValidateNotNullOrEmpty()][string[]]$File,
    [Parameter(Mandatory)][ValidatePattern('^[0-9A-Fa-f]{64}$')][string]$CertificateSha256,
    [Parameter(Mandatory)][string]$Output
)
$ErrorActionPreference = 'Stop'
if (-not $IsWindows) { throw 'Authenticode verification requires Windows' }
if (Test-Path -LiteralPath $Output) { throw 'Refusing to overwrite signature evidence' }
$outputPath = [IO.Path]::GetFullPath($Output)
$parent = Split-Path -Parent $outputPath
if (-not (Test-Path -LiteralPath $parent -PathType Container)) { throw 'Evidence directory does not exist' }

# Check only the explicitly selected NOH artifacts. Upstream DLLs keep their
# original identities; never recursively sign or attribute them to NOH.
$paths = @($File | ForEach-Object { (Get-Item -LiteralPath $_ -ErrorAction Stop).FullName })
if (@($paths | Sort-Object -Unique).Count -ne $paths.Count) { throw 'Duplicate signing input' }
$records = @()
foreach ($path in $paths) {
    $item = Get-Item -LiteralPath $path
    if ($item.PSIsContainer -or $item.Extension -notin @('.exe', '.dll')) { throw 'Expected explicit executable files' }
    if ($path -eq $outputPath) { throw 'Evidence must not replace an input file' }
    $before = (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLowerInvariant()
    $signature = Get-AuthenticodeSignature -LiteralPath $path
    if ($signature.Status -ne [System.Management.Automation.SignatureStatus]::Valid -or
        $signature.SignatureType -ne [System.Management.Automation.SignatureType]::Authenticode) {
        throw "Embedded Authenticode verification failed for $($item.Name): $($signature.Status)"
    }
    $certificate = $signature.SignerCertificate
    $certificateHash = $certificate.GetCertHashString([Security.Cryptography.HashAlgorithmName]::SHA256)
    if ($certificateHash -ine $CertificateSha256) { throw "Unexpected publisher certificate: $($item.Name)" }
    if ($null -eq $signature.TimeStamperCertificate) { throw "Missing trusted timestamp: $($item.Name)" }
    $after = (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($before -ne $after) { throw "File changed during signature verification: $($item.Name)" }
    $records += [ordered]@{
        name = $item.Name
        size = $item.Length
        sha256 = $after
        certificate_sha256 = $certificateHash.ToLowerInvariant()
        publisher = $certificate.Subject
        timestamp_certificate_sha256 = $signature.TimeStamperCertificate.GetCertHashString([Security.Cryptography.HashAlgorithmName]::SHA256).ToLowerInvariant()
        status = 'valid-embedded-authenticode'
    }
}
$evidence = [ordered]@{
    schema_version = 1
    checked_at_utc = [DateTime]::UtcNow.ToString('o')
    files = $records
    limitations = @('Windows trust verification on this host; not proof of SmartScreen reputation or clean-machine acceptance.')
}
$bytes = [Text.UTF8Encoding]::new($false).GetBytes(($evidence | ConvertTo-Json -Depth 6) + "`n")
# CreateNew also rejects a destination created concurrently after preflight.
$stream = [IO.File]::Open($outputPath, [IO.FileMode]::CreateNew, [IO.FileAccess]::Write, [IO.FileShare]::None)
try { $stream.Write($bytes, 0, $bytes.Length) } finally { $stream.Dispose() }
Write-Output "Verified $($records.Count) exact signed artifacts; SmartScreen reputation remains separate."
