// Silent updates started by NOH itself (src/auto_update): /WAITPID and /RELAUNCH.
// Exercised on its own by tools/installer/update-native.py.
function OpenWaitedProcess(Access: Cardinal; Inherit: Boolean; Pid: Cardinal): NativeUInt;
  external 'OpenProcess@kernel32.dll stdcall';
function WaitForWaitedProcess(Handle: NativeUInt; Milliseconds: Cardinal): Cardinal;
  external 'WaitForSingleObject@kernel32.dll stdcall';
function CloseWaitedProcess(Handle: NativeUInt): Boolean;
  external 'CloseHandle@kernel32.dll stdcall';
function WaitClock: Cardinal;
  external 'GetTickCount@kernel32.dll stdcall';

// /WAITPID=<pid>: NOH starts its own silent update and then exits. With
// /NOCLOSEAPPLICATIONS nothing closes it for us, so no file is touched until
// that process is gone; after 60 s Setup gives up and changes nothing.
function InitializeSetup: Boolean;
var
  Text: String;
  Pid: Integer;
  Process: NativeUInt;
  Started: Cardinal;
begin
  Result := True;
  Text := ExpandConstant('{param:WAITPID|}');
  if Text = '' then exit;
  Pid := StrToIntDef(Text, 0);
  if Pid <= 0 then begin
    Log('Invalid WAITPID value: ' + Text);
    Result := False;
    exit;
  end;
  // SYNCHRONIZE. A process that cannot be opened has already exited.
  Process := OpenWaitedProcess($00100000, False, Pid);
  if Process = 0 then begin
    Log('WAITPID ' + Text + ' had already exited');
    exit;
  end;
  try
    Started := WaitClock;
    // WAIT_OBJECT_0 = 0: it exited. The release CI reads this line.
    if WaitForWaitedProcess(Process, 60000) = 0 then
      Log('WAITPID ' + Text + ' exited after ' + IntToStr(WaitClock - Started) + ' ms')
    else begin
      Log('Process ' + Text + ' still runs after 60 s; nothing was changed');
      Result := False;
    end;
  finally
    CloseWaitedProcess(Process);
  end;
end;

// /RELAUNCH, only with a silent update: open NOH again at the end.
function RelaunchRequested: Boolean;
var
  I: Integer;
begin
  Result := False;
  if not WizardSilent then exit;
  for I := 1 to ParamCount do
    if CompareText(ParamStr(I), '/RELAUNCH') = 0 then Result := True;
end;
