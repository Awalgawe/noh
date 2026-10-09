// Shared optional-content acquisition for initial setup and later maintenance.
// Catalog functions and file entries are generated from verified portable bytes.
function ProfileIndex(Value: String): Integer;
begin
  Result := -1;
  if Value = 'minimal' then Result := 0;
  if Value = 'standard' then Result := 1;
  if Value = 'complete' then Result := 2;
end;

function SelectedProfile: Integer;
begin
  Result := ContentPage.SelectedValueIndex;
end;

function SelectedProfileName: String;
begin
  case SelectedProfile of
    0: Result := 'minimal';
    1: Result := 'standard';
    2: Result := 'complete';
  else RaiseException('Unknown content profile');
  end;
end;

function InstallMedia: Boolean;
begin
  Result := (SelectedProfile >= 1) and ({#EmbeddedMedia} or MediaRequested);
end;

function InstallSpeech: Boolean;
begin
  Result := (SelectedProfile >= 2) and ({#EmbeddedSpeech} or SpeechRequested);
end;

function IsMinimal: Boolean;
begin
  Result := SelectedProfile = 0;
end;
function IsStandard: Boolean;
begin
  Result := SelectedProfile = 1;
end;
function IsComplete: Boolean;
begin
  Result := SelectedProfile = 2;
end;

procedure InitializeContent;
var
  Previous, Requested: String;
  Selected, I: Integer;
begin
  MinimumProfile := 0;
  Previous := InstalledProfile;
  if Previous <> '' then begin
    MinimumProfile := ProfileIndex(Previous);
    if MinimumProfile < 0 then RaiseException(CustomMessage('ProfileMismatch'));
  end;
  Selected := ProfileIndex('{#DefaultProfile}');
#if NohWeb
  if Previous <> '' then Selected := MinimumProfile;
#endif
  Requested := ExpandConstant('{param:PROFILE|}');
  if Requested <> '' then begin
    Selected := ProfileIndex(Requested);
    if Selected < 0 then RaiseException(CustomMessage('ProfileMismatch'));
  end;
  if Selected < MinimumProfile then Selected := MinimumProfile;
  ContentPage := CreateInputOptionPage(wpWelcome, CustomMessage('ContentTitle'),
    CustomMessage('ContentDescription'), CustomMessage('PublicContentPrompt'), True, False);
  ContentPage.Add(CustomMessage('Minimal'));
  ContentPage.Add(CustomMessage('Standard'));
  ContentPage.Add(CustomMessage('Complete'));
  ContentPage.SelectedValueIndex := Selected;
  for I := 0 to 2 do ContentPage.CheckListBox.ItemEnabled[I] := I >= MinimumProfile;
  MediaRequested := True;
  SpeechRequested := True;
  ContentDownload := CreateDownloadPage(CustomMessage('DownloadTitle'), CustomMessage('DownloadDescription'), nil);
  ContentDownload.ShowBaseNameInsteadOfUrl := True;
  ContentExtraction := CreateExtractionPage(CustomMessage('DownloadTitle'), CustomMessage('PublicExtracting'), nil);
end;

procedure CleanupContent;
var
  I: Integer;
  Group, ArchivePath, StagingPath: String;
begin
  for I := 0 to 1 do begin
    if I = 0 then Group := 'media' else Group := 'speech';
    // Only exact files/directories owned by this unique Inno temporary folder.
    // Adjacent archives supplied by the user are never removed.
    ArchivePath := ExpandConstant('{tmp}\') + ComponentName(Group);
    StagingPath := ExpandConstant('{tmp}\noh-') + Group;
    if FileExists(ArchivePath) and not DeleteFile(ArchivePath) then
      Log('Temporary archive cleanup pending at setup exit: ' + ArchivePath);
    if DirExists(StagingPath) and not DelTree(StagingPath, True, True, True) then
      Log('Temporary extraction cleanup pending at setup exit: ' + StagingPath);
  end;
end;

function GroupReady(Group: String): Boolean;
var
  Files: TStringList;
  I, Separator: Integer;
  Name, Expected: String;
  Matches: Boolean;
begin
  Result := False;
  Files := TStringList.Create;
  try
    ComponentFiles(Group, Files);
    for I := 0 to Files.Count - 1 do begin
      Separator := Pos('=', Files[I]);
      Name := AddBackslash(WizardDirValue) + Copy(Files[I], 1, Separator - 1);
      Expected := Copy(Files[I], Separator + 1, MaxInt);
      if not FileExists(Name) then exit;
      VerifyFile(Name, Expected, Matches);
      if not Matches then exit;
    end;
    Result := Files.Count > 0;
  finally
    Files.Free;
  end;
end;

function PrepareContent: Boolean;
var
  Groups: TStringList;
  I: Integer;
  Group, Name, ArchivePath: String;
  FreeSpace, TotalSpace, DownloadBytes, InstalledBytes: Int64;
  Matches: Boolean;
begin
  Result := False;
  VerificationAborted := False;
  if SelectedProfile < MinimumProfile then exit;
  Groups := TStringList.Create;
  try
    try
    MediaRequested := (SelectedProfile >= 1) and not {#EmbeddedMedia} and not GroupReady('media');
    SpeechRequested := (SelectedProfile >= 2) and not {#EmbeddedSpeech} and not GroupReady('speech');
    if VerificationAborted then exit;
    if MediaRequested then Groups.Add('media');
    if SpeechRequested then Groups.Add('speech');
    if Groups.Count = 0 then begin
      Result := True;
      exit;
    end;
    DownloadBytes := 0;
    InstalledBytes := 0;
    for I := 0 to Groups.Count - 1 do begin
      DownloadBytes := DownloadBytes + ComponentBytes(Groups[I]);
      InstalledBytes := InstalledBytes + ComponentInstalledBytes(Groups[I]);
    end;
    if not GetSpaceOnDisk64(ExpandConstant('{tmp}'), FreeSpace, TotalSpace) or
      (FreeSpace < DownloadBytes + InstalledBytes + 67108864) then begin
      SuppressibleMsgBox(CustomMessage('PublicNoSpace'), mbError, MB_OK, IDOK);
      exit;
    end;
    ContentDownload.Clear;
    for I := 0 to Groups.Count - 1 do begin
      Group := Groups[I];
      Name := ComponentName(Group);
      ArchivePath := ExpandConstant('{src}\') + Name;
      if not FileExists(ArchivePath) then
        // Verify explicitly below with progress/cancellation, before extraction.
        // Inno's built-in post-download hash blocks its UI on large archives.
        ContentDownload.Add('{#ComponentBaseUrl}' + Name, Name, '');
    end;
    ContentDownload.Show;
    try
      ContentDownload.Download;
      if ContentDownload.AbortedByUser then exit;
    finally
      ContentDownload.Hide;
    end;
    ContentExtraction.Clear;
    for I := 0 to Groups.Count - 1 do begin
      Group := Groups[I];
      Name := ComponentName(Group);
      ArchivePath := ExpandConstant('{src}\') + Name;
      if not FileExists(ArchivePath) then ArchivePath := ExpandConstant('{tmp}\') + Name;
      VerifyFile(ArchivePath, ComponentHash(Group), Matches);
      if not Matches then
        RaiseException('Component archive checksum mismatch');
      ContentExtraction.AddEx(ArchivePath, ExpandConstant('{tmp}\noh-') + Group, '', True);
    end;
    ContentExtraction.Show;
    try
      ContentExtraction.Extract;
      if ContentExtraction.AbortedByUser then exit;
    finally
      ContentExtraction.Hide;
    end;
    Result := True;
  except
    Log(GetExceptionMessage);
    if not VerificationAborted and not ContentDownload.AbortedByUser and not ContentExtraction.AbortedByUser then
      SuppressibleMsgBox(CustomMessage('PublicDownloadFailed'), mbError, MB_OK, IDOK);
    end;
  finally
    Groups.Free;
    if not Result then CleanupContent;
  end;
end;

procedure RegisterPreviousData(PreviousDataKey: Integer);
var
  I: Integer;
  Name, Base: String;
begin
  SetPreviousData(PreviousDataKey, 'ContentProfile', SelectedProfileName);
  for I := 0 to 2 do begin
      case I of
        0: Name := 'minimal';
        1: Name := 'standard';
        2: Name := 'complete';
      end;
      Base := ExpandConstant('{app}\components\');
      SetPreviousData(PreviousDataKey, 'Manifest-' + Name, GetSHA256OfFile(Base + 'manifest-' + Name + '.json'));
      SetPreviousData(PreviousDataKey, 'Readme-' + Name, GetSHA256OfFile(Base + 'README-' + Name + '.md'));
  end;
end;
