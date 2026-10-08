; A thin wizard: the NOH controller owns authentication, installation and recovery.
; Build through build-installer.ps1, which authenticates the embedded inputs.
#include "payload.iss"

[Setup]
AppId=NOHBootstrap
AppName=NOH
AppVersion={#NohVersion}
AppPublisher=NOH
DefaultDirName={localappdata}\NOH
CreateAppDir=no
Uninstallable=no
CreateUninstallRegKey=no
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
DisableDirPage=yes
DisableProgramGroupPage=yes
DisableWelcomePage=no
UsePreviousAppDir=no
UsePreviousLanguage=no
SetupMutex=Local\NOHInstaller
Compression=none
SolidCompression=no
DiskSpanning=no
WizardStyle=modern
OutputDir={#OutputDirectory}
OutputBaseFilename={#OutputName}
SetupIconFile={#NohIcon}

[Languages]
Name: "en"; MessagesFile: "compiler:Default.isl"
Name: "fr"; MessagesFile: "compiler:Languages\French.isl"
Name: "de"; MessagesFile: "compiler:Languages\German.isl"
Name: "es"; MessagesFile: "compiler:Languages\Spanish.isl"
Name: "ja"; MessagesFile: "compiler:Languages\Japanese.isl"
Name: "ko"; MessagesFile: "compiler:Languages\Korean.isl"
Name: "zh"; MessagesFile: "compiler:Languages\ChineseSimplified.isl"

#include "messages.iss"

[Run]
Filename: "{localappdata}\NOH\application\current\noh.exe"; WorkingDir: "{localappdata}\NOH\application\current"; Description: "{cm:LaunchProgram,NOH}"; Flags: nowait postinstall skipifsilent; Check: CanLaunch

[Code]
var
  Choice: TInputOptionWizardPage;
  DownloadPage: TDownloadWizardPage;
  ApplyPage: TOutputMarqueeProgressWizardPage;
  InstalledProfile, Base, Bootstrap, CancelFile, AttemptLog, PackagePath: String;
  Downloading, Applying, Succeeded, InitializeIdentity, UserCancelled: Boolean;
  Attempt: Integer;
#if OfflineProfile == ""
  DownloadTimer: UINT_PTR;
  DownloadTimerCallback: NativeInt;

function SetTimer(hWnd: HWND; nIDEvent: UINT_PTR; uElapse: UINT; lpTimerFunc: NativeInt): UINT_PTR;
external 'SetTimer@user32.dll stdcall';
function KillTimer(hWnd: HWND; nIDEvent: UINT_PTR): BOOL;
external 'KillTimer@user32.dll stdcall';
function EnableWindow(hWnd: HWND; Enable: BOOL): BOOL;
external 'EnableWindow@user32.dll stdcall';

procedure StopDownloadTimer;
begin
  if DownloadTimer <> 0 then begin
    KillTimer(0, DownloadTimer);
    DownloadTimer := 0;
  end;
end;

procedure EnableDownloadInput(Window: HWND; Message: UINT; Timer: UINT_PTR; Time: DWORD);
begin
  if (DownloadTimer = 0) or (Timer <> DownloadTimer) then exit;
  StopDownloadTimer;
  { ExecAndLogOutput disables existing windows. Restore this page once, including
    before the first output line. Its progress-page style disables navigation.
    Never keep re-enabling a window: a later modal must retain its input lock. }
  if Downloading and not Applying and (WizardForm.CurPageID = DownloadPage.ID) then begin
    EnableWindow(WizardForm.Handle, True);
    if DownloadPage.AbortButton.CanFocus then
      WizardForm.ActiveControl := DownloadPage.AbortButton;
  end;
end;

procedure DeinitializeSetup;
begin
  StopDownloadTimer;
end;
#endif

function CanLaunch: Boolean;
begin
  Result := Succeeded;
end;

function Quote(const S: String): String;
begin
  Result := '"' + S + '"';
end;

function ProfileName(Index: Integer): String;
begin
  case Index of
    0: Result := 'minimal';
    1: Result := 'standard';
    2: Result := 'complete';
  else
    RaiseException('Invalid content profile');
  end;
end;

procedure ControllerLog(const S: String; const Error, FirstLine: Boolean);
var
  Percent: Integer;
begin
  if Downloading and (Pos('NOH_PROGRESS ', S) = 1) then begin
    Percent := StrToIntDef(Copy(S, 14, MaxInt), 0);
    DownloadPage.SetProgress(Percent, 100);
  end else begin
    Log(S);
    if AttemptLog <> '' then SaveStringToFile(AttemptLog, S + #13#10, True);
  end;
end;

function RunController(const Params: String): Boolean;
var
  ExitCode: Integer;
begin
  Result := ExecAndLogOutput(Bootstrap, Params, ExpandConstant('{tmp}'),
    SW_SHOWNORMAL, ewWaitUntilTerminated, ExitCode, @ControllerLog);
  Result := Result and (ExitCode = 0);
end;

procedure RequestDownloadCancel(Sender: TObject);
begin
  if not Downloading or UserCancelled then exit;
  UserCancelled := True;
  if not SaveStringToFile(CancelFile, 'cancel', False) then
    Log('Could not write cancellation marker; installation remains prohibited.');
  DownloadPage.AbortButton.Enabled := False;
  Log('Download cancellation requested; waiting for the controller to exit.');
end;

function InspectInstallation: Boolean;
var
  PlanFile, Registration: String;
begin
  Result := False;
  { Velopack is the only registered application owner. Refuse a second location. }
  if RegQueryStringValue(HKCU, 'Software\Microsoft\Windows\CurrentVersion\Uninstall\NOH',
    'InstallLocation', Registration) then begin
    if Pos('\\?\', Registration) = 1 then Delete(Registration, 1, 4);
    if CompareText(RemoveBackslashUnlessRoot(Registration), Base + '\application') <> 0 then begin
      MsgBox(CustomMessage('OtherInstallation'), mbError, MB_OK);
      exit;
    end;
  end;
  PlanFile := ExpandConstant('{tmp}\installation.ini');
  if FileExists(PlanFile) and not DeleteFile(PlanFile) then exit;
  if not RunController('inspect --base ' + Quote(Base) + ' --version {#NohVersion} --output ' + Quote(PlanFile)) then begin
    MsgBox(CustomMessage('InspectFailed'), mbError, MB_OK);
    exit;
  end;
  InitializeIdentity := GetIniInt('installation', 'initialize', -1, 0, 1, PlanFile) = 1;
  InstalledProfile := GetIniString('installation', 'profile', '', PlanFile);
  Result := InitializeIdentity or (InstalledProfile <> '');
end;

procedure InitializeWizard;
var
  Index: Integer;
begin
  Base := ExpandConstant('{localappdata}\NOH');
  ExtractTemporaryFile('bootstrap.exe');
  Bootstrap := ExpandConstant('{tmp}\bootstrap.exe');
  Choice := CreateInputOptionPage(wpWelcome, CustomMessage('ContentTitle'),
    CustomMessage('ContentDescription'), CustomMessage('ContentPrompt'), True, False);
  Choice.Add(CustomMessage('Minimal'));
  Choice.Add(CustomMessage('Standard'));
  Choice.Add(CustomMessage('Complete'));
  Choice.SelectedValueIndex := 1;
  DownloadPage := CreateDownloadPage(CustomMessage('DownloadTitle'), CustomMessage('DownloadDescription'), nil);
  DownloadPage.AbortButton.OnClick := @RequestDownloadCancel;
#if OfflineProfile == ""
  DownloadPage.AbortButton.Cancel := True;
  DownloadTimerCallback := CreateCallback(@EnableDownloadInput);
#endif
  ApplyPage := CreateOutputMarqueeProgressPage(CustomMessage('ApplyTitle'), CustomMessage('ApplyDescription'));
  if not InspectInstallation then RaiseException(CustomMessage('InspectFailed'));
  for Index := 0 to 2 do begin
    if InstalledProfile = ProfileName(Index) then Choice.SelectedValueIndex := Index;
#if OfflineProfile != ""
    if '{#OfflineProfile}' = ProfileName(Index) then Choice.SelectedValueIndex := Index;
#endif
  end;
  if InstalledProfile <> '' then begin
#if OfflineProfile != ""
    if InstalledProfile <> '{#OfflineProfile}' then RaiseException(CustomMessage('ProfileMismatch'));
#endif
    Choice.CheckListBox.Enabled := False;
    Choice.SubCaptionLabel.Caption := CustomMessage('KeepProfile');
  end;
#if OfflineProfile != ""
  Choice.CheckListBox.Enabled := False;
#endif
end;

function UpdateReadyMemo(Space, NewLine, MemoUserInfoInfo, MemoDirInfo, MemoTypeInfo,
  MemoComponentsInfo, MemoGroupInfo, MemoTasksInfo: String): String;
var
  Content: String;
begin
  Content := ProfileName(Choice.SelectedValueIndex);
  Result := Choice.CheckListBox.ItemCaption[Choice.SelectedValueIndex] + NewLine + NewLine +
    Base + NewLine + FmtMessage(CustomMessage('Sizes'), [IntToStr(PackageBytes(Content) div 1048576),
    IntToStr(InstalledBytes(Content) div 1048576)]) + NewLine + NewLine + CustomMessage('BeforeInstall');
end;

function PrepareToInstall(var NeedsRestart: Boolean): String;
var
  Content, Envelope, Destination, Params, Source: String;
  FreeBytes, TotalBytes, RequiredBytes: Int64;
begin
  Result := CustomMessage('InstallFailed');
  Content := ProfileName(Choice.SelectedValueIndex);
  if not InspectInstallation then exit;
  if (InstalledProfile <> '') and (InstalledProfile <> Content) then begin
    Result := CustomMessage('ProfileMismatch');
    exit;
  end;
  Attempt := Attempt + 1;
  if not ForceDirectories(Base + '\installer-logs') then exit;
  RequiredBytes := 2 * PackageBytes(Content) + 2 * InstalledBytes(Content) + 67108864;
  if not GetSpaceOnDisk64(Base, FreeBytes, TotalBytes) or (FreeBytes < RequiredBytes) then begin
    Result := CustomMessage('NotEnoughSpace');
    exit;
  end;
  if not GetSpaceOnDisk64(ExpandConstant('{tmp}'), FreeBytes, TotalBytes) or
    (FreeBytes < PackageBytes(Content) + 67108864) then begin
    Result := CustomMessage('NotEnoughSpace');
    exit;
  end;
  AttemptLog := Base + '\installer-logs\' + GetDateTimeString('yyyymmdd-hhnnss-zzz', '-', ':') + '-' + IntToStr(Attempt) + '.log';
  CancelFile := ExpandConstant('{tmp}\cancel-') + IntToStr(Attempt);
  UserCancelled := False;
  Envelope := ExpandConstant('{tmp}\envelope-') + Content + '.json';
  ExtractTemporaryFile('envelope-' + Content + '.json');
  PackagePath := ExpandConstant('{tmp}\') + PackageName(Content);
  try
#if OfflineProfile == ""
    Destination := ExpandConstant('{tmp}\download-') + IntToStr(Attempt);
    Params := 'fetch --envelope ' + Quote(Envelope) + ' --destination ' + Quote(Destination) +
      ' --version {#NohVersion} --profile ' + Content + ' --cancel-file ' + Quote(CancelFile);
    Source := DownloadSource(Content);
    if Source <> '' then Params := Params + ' --source ' + Quote(Source);
    DownloadPage.SetText(CustomMessage('DownloadDescription'), '');
    DownloadPage.SetProgress(0, 100);
    DownloadPage.AbortButton.Enabled := True;
    Downloading := True;
    DownloadPage.Show;
    try
      DownloadPage.SetProgress(0, 100);
      DownloadTimer := SetTimer(0, 0, 100, DownloadTimerCallback);
      if DownloadTimer = 0 then begin
        Log('Cannot enable download cancellation; refusing to start.');
        Result := CustomMessage('DownloadFailed') + #13#10 + AttemptLog;
        exit;
      end;
      if not RunController(Params) then begin
        if UserCancelled then Result := CustomMessage('DownloadCancelled')
        else Result := CustomMessage('DownloadFailed') + #13#10 + AttemptLog;
        exit;
      end;
      PackagePath := Destination + '\' + PackageName(Content);
    finally
      StopDownloadTimer;
      Downloading := False;
      DownloadPage.Hide;
    end;
#else
    ExtractTemporaryFile(PackageName(Content));
#endif
    if UserCancelled then begin
      Result := CustomMessage('DownloadCancelled');
      exit;
    end;
    Params := 'setup --base ' + Quote(Base) + ' --package ' + Quote(PackagePath) +
      ' --envelope ' + Quote(Envelope) + ' --version {#NohVersion} --profile ' + Content +
      ' --confirm-install --no-downgrade';
    if InitializeIdentity then Params := Params + ' --initialize';
    Applying := True;
    ApplyPage.SetText(CustomMessage('ApplyDescription'), CustomMessage('WaitUntilDone'));
    ApplyPage.Show;
    ApplyPage.Animate;
    try
      if not RunController(Params) then begin
        Result := CustomMessage('InstallFailed') + #13#10 + AttemptLog;
        exit;
      end;
    finally
      Applying := False;
      ApplyPage.Hide;
    end;
    Succeeded := True;
    Result := '';
  except
    Log(GetExceptionMessage);
    Result := CustomMessage('InstallFailed') + #13#10 + AttemptLog;
  end;
end;

procedure CancelButtonClick(CurPageID: Integer; var Cancel, Confirm: Boolean);
begin
  if Downloading then begin
    RequestDownloadCancel(nil);
    Cancel := False;
    Confirm := False;
  end else if Applying then begin
    Cancel := False;
    Confirm := False;
  end;
end;

procedure CurStepChanged(CurStep: TSetupStep);
var
  Shortcut: String;
begin
  if (CurStep = ssPostInstall) and Succeeded then begin
    if not FileExists(ExpandConstant('{userprograms}\NOH.lnk')) then
      Shortcut := CreateShellLink(ExpandConstant('{userprograms}\NOH.lnk'), 'NOH',
        Base + '\application\current\noh.exe', '', Base + '\application\current',
        Base + '\application\current\noh.ico', 0, SW_SHOWNORMAL);
  end;
end;
