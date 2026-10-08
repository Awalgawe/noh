param(
    [ValidateSet('Verify','Publish')][string]$Mode='Verify',
    [Parameter(Mandatory)][string]$Directory,
    [Parameter(Mandatory)][string]$Verifier,
    [Parameter(Mandatory)][string]$PublicKeys,
    [Parameter(Mandatory)][string]$Version,
    [Parameter(Mandatory)][string]$Tag,
    [Parameter(Mandatory)][ValidatePattern('^[0-9a-f]{40}$')][string]$SourceCommit
)
$ErrorActionPreference='Stop'
$workspace=(Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$Directory=(Resolve-Path -LiteralPath $Directory).Path
$Verifier=(Resolve-Path -LiteralPath $Verifier).Path
$PublicKeys=(Resolve-Path -LiteralPath $PublicKeys).Path
$policyPath=Join-Path $workspace '.github/update-publication-policy.json'
$policy=Get-Content -LiteralPath $policyPath -Raw|ConvertFrom-Json
if($policy.repository -ne $env:GITHUB_REPOSITORY){throw 'Repository differs from independent publication policy'}
if($Tag -notmatch '^[A-Za-z0-9][A-Za-z0-9._-]*$'){throw 'Unsupported release tag'}
$envelope=Join-Path $Directory 'envelope.json'
$packages=Join-Path $Directory 'packages'
if(@(Get-ChildItem -LiteralPath $Directory -Force).Count -ne 2){throw 'Unexpected staged artifact entries'}

# Windows-only publication guard mirrors the already qualified ProtectedFile contract.
if(-not ('NohPublicationGuard' -as [type])){
Add-Type -TypeDefinition @'
using System;
using System.ComponentModel;
using System.Runtime.InteropServices;
using Microsoft.Win32.SafeHandles;
public static class NohPublicationGuard {
  [DllImport("kernel32.dll", CharSet=CharSet.Unicode, SetLastError=true)]
  static extern SafeFileHandle CreateFileW(string p,uint access,uint share,IntPtr security,uint creation,uint flags,IntPtr template);
  [DllImport("kernel32.dll", SetLastError=true)]
  static extern bool GetFileInformationByHandleEx(SafeFileHandle h,int kind,out AttributeTag info,uint size);
  [StructLayout(LayoutKind.Sequential)] struct AttributeTag { public uint attributes; public uint tag; }
  public static SafeFileHandle Open(string path,bool directory) {
    var h=CreateFileW(path,directory?0u:0x80000000u,directory?3u:1u,IntPtr.Zero,3,
      0x00200000u|(directory?0x02000000u:0u),IntPtr.Zero);
    if(h.IsInvalid){h.Dispose();throw new Win32Exception(Marshal.GetLastWin32Error());}
    AttributeTag info;
    if(!GetFileInformationByHandleEx(h,9,out info,8)){h.Dispose();throw new Win32Exception(Marshal.GetLastWin32Error());}
    if((info.attributes&0x400)!=0 || ((info.attributes&0x10)!=0)!=directory){h.Dispose();throw new Exception("Reparse point or unexpected file type");}
    return h;
  }
}
'@
}
$guards=[Collections.Generic.List[IDisposable]]::new()
$protectedDirectories=[Collections.Generic.HashSet[string]]::new([StringComparer]::OrdinalIgnoreCase)
function Protect-Input([string]$Path){
    $absolute=[IO.Path]::GetFullPath($Path)
    $parents=[Collections.Generic.List[string]]::new()
    $parent=[IO.DirectoryInfo]::new([IO.Path]::GetDirectoryName($absolute))
    while($null -ne $parent){$parents.Add($parent.FullName);$parent=$parent.Parent}
    for($index=$parents.Count-1;$index -ge 0;$index--){
        if($protectedDirectories.Add($parents[$index])){$guards.Add([NohPublicationGuard]::Open($parents[$index],$true))}
    }
    $guards.Add([NohPublicationGuard]::Open($absolute,$false))
}
function Invoke-Verifier([string]$Root){
    & $Verifier verify-release --public-keys $PublicKeys --policy $policyPath --envelope (Join-Path $Root 'envelope.json') --packages (Join-Path $Root 'packages') --expected-version $Version --tag $Tag
    if($LASTEXITCODE -ne 0){throw 'Exhaustive release verification failed'}
}
try {
    Protect-Input $policyPath
    Protect-Input $PublicKeys
    Protect-Input $envelope
    $members=@(Get-ChildItem -LiteralPath $packages -Force)
    foreach($member in $members){Protect-Input $member.FullName}
    Invoke-Verifier $Directory
    if($Mode -eq 'Verify'){Write-Output 'Complete staged trial verified; nothing published';return}

    $qualificationPath=Join-Path $workspace '.github/update-qualification.json'
    Protect-Input $qualificationPath
    $qualification=Get-Content -LiteralPath $qualificationPath -Raw|ConvertFrom-Json
    if($qualification.schema_version -ne 1){throw 'Unsupported qualification record'}
    $matches=@($qualification.qualified_releases|Where-Object { $_.version -eq $Version -and $_.source_commit -eq $SourceCommit })
    if($matches.Count -ne 1 -or $matches[0].gui_and_external_repair_passed -ne $true){throw 'Independent GUI/repair qualification is missing'}
    $record=$matches[0]
    if((Get-FileHash -LiteralPath $envelope).Hash.ToLowerInvariant() -ne $record.envelope_sha256){throw 'Qualification envelope mismatch'}
    if(@($record.packages.PSObject.Properties).Count -ne $members.Count){throw 'Qualification package matrix mismatch'}
    foreach($member in $members){
        if((Get-FileHash -LiteralPath $member.FullName).Hash.ToLowerInvariant() -ne $record.packages.($member.Name)){throw 'Qualification package mismatch'}
    }
    $repo=$policy.repository
    $repositoryJson=& gh api "repos/$repo"
    if($LASTEXITCODE -ne 0){throw 'Cannot inspect repository visibility'}
    if(($repositoryJson|ConvertFrom-Json).private -ne $true){throw 'This workflow permits private trials only'}
    # Check all tags/releases, including drafts. Any API failure is a refusal.
    $releaseJson=& gh api --paginate --slurp "repos/$repo/releases?per_page=100"
    if($LASTEXITCODE -ne 0){throw 'Cannot inspect existing releases'}
    foreach($page in @($releaseJson|ConvertFrom-Json)){foreach($release in @($page)){if($release.tag_name -eq $Tag){throw 'Release already exists'}}}
    $tagJson=& gh api --paginate --slurp "repos/$repo/git/matching-refs/tags/$Tag"
    if($LASTEXITCODE -ne 0){throw 'Cannot inspect existing tags'}
    foreach($page in @($tagJson|ConvertFrom-Json)){foreach($ref in @($page)){if($ref.ref -eq "refs/tags/$Tag"){throw 'Tag already exists'}}}
    $notes=Join-Path $env:RUNNER_TEMP ('noh-release-notes-'+[guid]::NewGuid().ToString('N')+'.txt')
    Set-Content -LiteralPath $notes 'Controlled Windows update qualification trial; no production or macOS/Linux support claim.'
    & gh release create $Tag --repo $repo --target $SourceCommit --draft --prerelease --title "NOH controlled trial $Version" --notes-file $notes
    if($LASTEXITCODE -ne 0){throw 'Draft creation failed'}
    $uploads=@($envelope)+@($members|ForEach-Object {$_.FullName})
    & gh release upload $Tag @uploads --repo $repo
    if($LASTEXITCODE -ne 0){throw 'Upload failed; draft remains unexposed'}
    $downloadRoot=Join-Path $env:RUNNER_TEMP ('noh-remote-verification-'+[guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $downloadRoot|Out-Null
    & gh release download $Tag --repo $repo --dir $downloadRoot
    if($LASTEXITCODE -ne 0){throw 'Remote asset download failed; draft remains unexposed'}
    $remoteNames=@(Get-ChildItem -LiteralPath $downloadRoot -Force|ForEach-Object {$_.Name})
    $expectedNames=@('envelope.json')+@($members|ForEach-Object {$_.Name})
    if(@(Compare-Object $remoteNames $expectedNames).Count -ne 0){throw 'Remote asset list differs; draft remains unexposed'}
    $remotePackages=Join-Path $downloadRoot 'packages'
    New-Item -ItemType Directory -Path $remotePackages|Out-Null
    foreach($member in $members){Move-Item -LiteralPath (Join-Path $downloadRoot $member.Name) -Destination (Join-Path $remotePackages $member.Name)}
    if((Get-FileHash -LiteralPath (Join-Path $downloadRoot 'envelope.json')).Hash -ne (Get-FileHash -LiteralPath $envelope).Hash){throw 'Remote envelope differs; draft remains unexposed'}
    Invoke-Verifier $downloadRoot
    & gh release edit $Tag --repo $repo --draft=false --prerelease
    if($LASTEXITCODE -ne 0){throw 'Complete trial publication failed'}
    Write-Output 'Complete authenticated private trial published'
} finally {
    for($index=$guards.Count-1;$index -ge 0;$index--){$guards[$index].Dispose()}
}
