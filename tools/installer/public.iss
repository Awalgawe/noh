; Ordinary per-user installer for the independently verified portable payload.
; Microsoft supplies the optional speech prerequisite directly to the user.
#include "public-payload.iss"

[Setup]
AppId=NOH.Public
AppName=NOH
AppVersion={#NohVersion}
AppPublisher=Awalgawe
AppPublisherURL=https://github.com/Awalgawe/noh
AppSupportURL=https://github.com/Awalgawe/noh/issues
AppUpdatesURL=https://github.com/Awalgawe/noh/releases/latest
DefaultDirName={localappdata}\Programs\NOH
DefaultGroupName=NOH
PrivilegesRequired=lowest
SetupArchitecture=x64
ArchitecturesAllowed=x64os
ArchitecturesInstallIn64BitMode=x64os
MinVersion=10.0.19045
DisableProgramGroupPage=yes
DisableWelcomePage=no
UsePreviousLanguage=yes
WizardStyle=modern
Compression=lzma2/fast
SolidCompression=yes
OutputDir={#OutputDirectory}
OutputBaseFilename=NOH-{#NohVersion}-windows-x64-Setup
SetupIconFile={#NohIcon}
UninstallDisplayIcon={app}\noh.exe
CloseApplications=yes
RestartApplications=no
SetupMutex=Local\NOHPublicInstaller

[Languages]
Name: "en"; MessagesFile: "compiler:Default.isl"
Name: "fr"; MessagesFile: "compiler:Languages\French.isl"
Name: "de"; MessagesFile: "compiler:Languages\German.isl"
Name: "es"; MessagesFile: "compiler:Languages\Spanish.isl"
Name: "ja"; MessagesFile: "compiler:Languages\Japanese.isl"
Name: "ko"; MessagesFile: "compiler:Languages\Korean.isl"
Name: "zh"; MessagesFile: "compiler:Languages\ChineseSimplified.isl"

#include "public-messages.iss"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; Flags: unchecked

[Icons]
Name: "{group}\NOH"; Filename: "{app}\noh.exe"; WorkingDir: "{app}"
Name: "{autodesktop}\NOH"; Filename: "{app}\noh.exe"; WorkingDir: "{app}"; Tasks: desktopicon

[Run]
Filename: "{app}\noh.exe"; Description: "{cm:LaunchProgram,NOH}"; WorkingDir: "{app}"; Flags: nowait postinstall skipifsilent

[Code]
const
  RuntimeKey = 'SOFTWARE\Microsoft\VisualStudio\14.0\VC\Runtimes\x64';
  RuntimeUrl = 'https://download.visualstudio.microsoft.com/download/pr/ebdab8e5-1d7b-4d9f-a11b-cbb1720c3b12/843068991DAAA1F73AD9F6239BCE4D0F6A07A51F18C37EA2A867E9BECA71295C/VC_redist.x64.exe';
  RuntimeHash = '843068991daaa1f73ad9f6239bce4d0f6a07a51f18c37ea2a867e9beca71295c';
var
  SpeechPage: TInputOptionWizardPage;
  DownloadPage: TDownloadWizardPage;

function RuntimeReady: Boolean;
var
  Installed: Cardinal;
  Text: String;
  Version: Int64;
begin
  Result := False;
  if not RegQueryDWordValue(HKLM64, RuntimeKey, 'Installed', Installed) or (Installed <> 1) then exit;
  if not RegQueryStringValue(HKLM64, RuntimeKey, 'Version', Text) then exit;
  if Copy(Text, 1, 1) = 'v' then Delete(Text, 1, 1);
  if Copy(Text, 1, 3) <> '14.' then exit;
  if not StrToVersion(Text, Version) then exit;
  Result := (ComparePackedVersion(Version, PackVersionComponents(14, 51, 36247, 0)) >= 0)
    and FileExists(ExpandConstant('{sys}\msvcp140.dll'))
    and FileExists(ExpandConstant('{sys}\vcruntime140.dll'))
    and FileExists(ExpandConstant('{sys}\vcruntime140_1.dll'))
    and FileExists(ExpandConstant('{sys}\vcomp140.dll'));
end;

procedure InitializeWizard;
begin
  SpeechPage := CreateInputOptionPage(wpSelectTasks, CustomMessage('SpeechTitle'),
    CustomMessage('SpeechDescription'), CustomMessage('SpeechExplanation'), False, False);
  SpeechPage.Add(CustomMessage('SpeechChoice'));
  SpeechPage.Values[0] := True;
  DownloadPage := CreateDownloadPage(CustomMessage('RuntimeDownloadTitle'),
    CustomMessage('RuntimeDownloadDescription'), nil);
  DownloadPage.ShowBaseNameInsteadOfUrl := True;
end;

function ShouldSkipPage(PageID: Integer): Boolean;
begin
  Result := (PageID = SpeechPage.ID) and RuntimeReady;
end;

function NextButtonClick(CurPageID: Integer): Boolean;
var
  ErrorCode: Integer;
begin
  Result := True;
  if (CurPageID <> wpReady) or RuntimeReady or not SpeechPage.Values[0] then exit;
  // Silent installation never grants consent to an external elevated installer.
  if WizardSilent then exit;
  DownloadPage.Clear;
  DownloadPage.Add(RuntimeUrl, 'vc_redist.x64.exe', RuntimeHash);
  DownloadPage.Show;
  try
    try
      DownloadPage.Download;
    except
      Result := False;
      if not DownloadPage.AbortedByUser then
        SuppressibleMsgBox(CustomMessage('RuntimeDownloadFailed'), mbError, MB_OK, IDOK);
    end;
  finally
    DownloadPage.Hide;
  end;
  if not Result then exit;
  // Hash-pinned, originally Microsoft-signed bytes; Windows handles UAC and the
  // Microsoft installer presents its own licence and installation choices.
  if not ShellExec('', ExpandConstant('{tmp}\vc_redist.x64.exe'), '/install /norestart',
    '', SW_SHOWNORMAL, ewWaitUntilTerminated, ErrorCode) then begin
    Result := False;
    SuppressibleMsgBox(CustomMessage('RuntimeNotInstalled'), mbInformation, MB_OK, IDOK);
    exit;
  end;
  Result := RuntimeReady;
  if not Result then
    SuppressibleMsgBox(CustomMessage('RuntimeNotInstalled'), mbInformation, MB_OK, IDOK);
end;
