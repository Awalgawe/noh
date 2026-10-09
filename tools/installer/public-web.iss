; Small public selector: fetch one exact offline installer, then run its wizard.
#include "web-payload.iss"
[Setup]
AppId=NOH.Public
AppName=NOH
AppVersion={#NohVersion}
AppPublisher=Awalgawe
AppPublisherURL=https://github.com/Awalgawe/noh
DefaultDirName={localappdata}\Programs\NOH
CreateAppDir=no
Uninstallable=no
CreateUninstallRegKey=no
PrivilegesRequired=lowest
SetupArchitecture=x64
ArchitecturesAllowed=x64os
MinVersion=10.0.19045
DisableDirPage=yes
DisableProgramGroupPage=yes
DisableWelcomePage=no
DisableFinishedPage=yes
WizardStyle=modern
OutputDir={#OutputDirectory}
OutputBaseFilename=NOH-{#NohVersion}-windows-x64-Web-Setup
SetupIconFile={#NohIcon}
SetupMutex=Local\NOHPublicWebInstaller

[Languages]
Name: "en"; MessagesFile: "compiler:Default.isl"
Name: "fr"; MessagesFile: "compiler:Languages\French.isl"
Name: "de"; MessagesFile: "compiler:Languages\German.isl"
Name: "es"; MessagesFile: "compiler:Languages\Spanish.isl"
Name: "ja"; MessagesFile: "compiler:Languages\Japanese.isl"
Name: "ko"; MessagesFile: "compiler:Languages\Korean.isl"
Name: "zh"; MessagesFile: "compiler:Languages\ChineseSimplified.isl"
#include "public-messages.iss"

[Code]
const
  PublicUninstallKey = 'Software\Microsoft\Windows\CurrentVersion\Uninstall\NOH.Public_is1';
var
  ContentPage: TInputOptionWizardPage;
  DownloadPage: TDownloadWizardPage;
  InvalidSelection: Boolean;

#include "web-catalog.iss"
#include "public-profile.iss"

function ProfileIndex(Value: String): Integer;
begin
  Result := -1;
  if Value = 'minimal' then Result := 0;
  if Value = 'standard' then Result := 1;
  if Value = 'complete' then Result := 2;
end;

procedure InitializeWizard;
var
  Previous, Requested: String;
  Selected, I: Integer;
begin
  ContentPage := CreateInputOptionPage(wpWelcome, CustomMessage('ContentTitle'),
    CustomMessage('ContentDescription'), CustomMessage('PublicContentPrompt'), True, False);
  ContentPage.Add(CustomMessage('Minimal') + ' (' + ProfileSize(0) + ')');
  ContentPage.Add(CustomMessage('Standard') + ' (' + ProfileSize(1) + ')');
  ContentPage.Add(CustomMessage('Complete') + ' (' + ProfileSize(2) + ')');
  Previous := InstalledProfile;
  Requested := ExpandConstant('{param:PROFILE|}');
  Selected := 1;
  InvalidSelection := False;
  if Requested <> '' then begin
    Selected := ProfileIndex(Requested);
    InvalidSelection := Selected < 0;
  end;
  if Previous <> '' then begin
    InvalidSelection := InvalidSelection or (ProfileIndex(Previous) < 0);
    if (Requested = '') or (Selected < ProfileIndex(Previous)) then
      Selected := ProfileIndex(Previous);
    for I := 0 to 2 do ContentPage.CheckListBox.ItemEnabled[I] := I >= ProfileIndex(Previous);
  end;
  if Selected < 0 then Selected := 1;
  for I := 0 to 2 do ContentPage.Values[I] := I = Selected;
  DownloadPage := CreateDownloadPage(CustomMessage('DownloadTitle'), CustomMessage('DownloadDescription'), nil);
  DownloadPage.ShowBaseNameInsteadOfUrl := True;
end;

function NextButtonClick(CurPageID: Integer): Boolean;
var
  Selected, I, ExitCode: Integer;
  Name, Path, Parameters, Location: String;
begin
  Result := True;
  if CurPageID <> wpReady then exit;
  Result := False;
  if InvalidSelection then begin
    SuppressibleMsgBox(CustomMessage('ProfileMismatch'), mbError, MB_OK, IDOK);
    exit;
  end;
  Selected := 1;
  for I := 0 to 2 do if ContentPage.Values[I] then Selected := I;
  Name := ProfileName(Selected);
  Path := ExpandConstant('{src}\') + Name;
  // A neighbouring offline installer is a verified cache, never a URL override.
  if not FileExists(Path) then begin
    DownloadPage.Clear;
    DownloadPage.Add('https://github.com/Awalgawe/noh/releases/download/v{#NohVersion}/' + Name,
      Name, ProfileHash(Selected));
    DownloadPage.Show;
    try
      try
        DownloadPage.Download;
        Path := ExpandConstant('{tmp}\') + Name;
      except
        if not DownloadPage.AbortedByUser then
          SuppressibleMsgBox(CustomMessage('PublicDownloadFailed'), mbError, MB_OK, IDOK);
        exit;
      end;
    finally
      DownloadPage.Hide;
    end;
  end;
  if GetSHA256OfFile(Path) <> ProfileHash(Selected) then begin
    SuppressibleMsgBox(CustomMessage('PublicDownloadFailed'), mbError, MB_OK, IDOK);
    exit;
  end;
  Parameters := '/CURRENTUSER /NORESTART /LANG=' + ExpandConstant('{language}');
  Location := ExpandConstant('{param:DIR|}');
  if Location <> '' then begin
    if Pos('"', Location) <> 0 then exit;
    Parameters := Parameters + ' /DIR="' + Location + '"';
  end;
  if WizardSilent then Parameters := Parameters + ' /VERYSILENT /SUPPRESSMSGBOXES /NOICONS /TASKS=!desktopicon';
  Log('Starting verified profile installer: ' + Name);
  if Exec(Path, Parameters, '', SW_SHOWNORMAL, ewWaitUntilTerminated, ExitCode) then
    Result := ExitCode = 0;
  if not Result then
    SuppressibleMsgBox(CustomMessage('PublicInstallFailed'), mbError, MB_OK, IDOK);
end;
