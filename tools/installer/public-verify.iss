// Incremental Windows SHA-256: pump the wizard between bounded reads. Inno's
// GetSHA256OfFile (including DownloadTemporaryFile's hash) blocks on large files.
function OpenVerificationFile(Name: String; Access, Share: Cardinal; Security: NativeUInt;
  Creation, Flags: Cardinal; Template: NativeUInt): NativeUInt;
  external 'CreateFileW@kernel32.dll stdcall';
function ReadVerificationFile(Handle: NativeUInt; var Buffer: Byte; Count: Cardinal;
  var ReadCount: Cardinal; Overlapped: NativeUInt): Boolean;
  external 'ReadFile@kernel32.dll stdcall';
function VerificationFileSize(Handle: NativeUInt; var Size: Int64): Boolean;
  external 'GetFileSizeEx@kernel32.dll stdcall';
function CloseVerificationFile(Handle: NativeUInt): Boolean;
  external 'CloseHandle@kernel32.dll stdcall';
function AcquireHashProvider(var Provider: NativeUInt; Container, ProviderName: NativeUInt;
  ProviderType, Flags: Cardinal): Boolean;
  external 'CryptAcquireContextW@advapi32.dll stdcall';
function CreateVerificationHash(Provider: NativeUInt; Algorithm: Cardinal; Key: NativeUInt;
  Flags: Cardinal; var Hash: NativeUInt): Boolean;
  external 'CryptCreateHash@advapi32.dll stdcall';
function AddVerificationBytes(Hash: NativeUInt; var Buffer: Byte; Count, Flags: Cardinal): Boolean;
  external 'CryptHashData@advapi32.dll stdcall';
function ReadVerificationHash(Hash: NativeUInt; Parameter: Cardinal; var Buffer: Byte;
  var Count: Cardinal; Flags: Cardinal): Boolean;
  external 'CryptGetHashParam@advapi32.dll stdcall';
function DestroyVerificationHash(Hash: NativeUInt): Boolean;
  external 'CryptDestroyHash@advapi32.dll stdcall';
function ReleaseHashProvider(Provider: NativeUInt; Flags: Cardinal): Boolean;
  external 'CryptReleaseContext@advapi32.dll stdcall';

var
  VerificationPage: TOutputProgressWizardPage;
  VerificationCancel: TNewButton;
  VerificationAborted: Boolean;

procedure CancelVerification(Sender: TObject);
begin
  VerificationAborted := True;
end;

procedure InitializeVerification;
begin
  VerificationPage := CreateOutputProgressPage(CustomMessage('PublicVerifying'),
    CustomMessage('PublicVerifyingDescription'));
  VerificationCancel := TNewButton.Create(WizardForm);
  VerificationCancel.Parent := WizardForm;
  VerificationCancel.SetBounds(WizardForm.CancelButton.Left, WizardForm.CancelButton.Top,
    WizardForm.CancelButton.Width, WizardForm.CancelButton.Height);
  VerificationCancel.Caption := SetupMessage(msgButtonCancel);
  VerificationCancel.Cancel := True;
  VerificationCancel.OnClick := @CancelVerification;
  VerificationCancel.Visible := False;
end;

procedure VerifyFile(Name, Expected: String; var Matches: Boolean);
var
  Handle, Provider, Hash: NativeUInt;
  Buffer: array of Byte;
  Digest: array[0..31] of Byte;
  Count, DigestSize: Cardinal;
  Size, Done: Int64;
  I: Integer;
  Actual: String;
begin
  Matches := False;
  if VerificationAborted then exit;
  if not FileExists(Name) then begin
    Log('Verification file is missing: ' + Name);
    exit;
  end;
  // Read sharing only: deny writes and deletion for the duration of verification.
  Handle := OpenVerificationFile(Name, $80000000, 1, 0, 3, $08000000, 0);
  if Handle = NativeUInt(-1) then RaiseException('Cannot open file for verification');
  Provider := 0;
  Hash := 0;
  Actual := '';
  try
    // Keep large storage off the Pascal Script stack (including var arguments).
    SetArrayLength(Buffer, 1048576);
    VerificationPage.Show;
    VerificationCancel.Visible := not WizardSilent;
    VerificationPage.SetText(CustomMessage('PublicVerifying'), ExtractFileName(Name));
    if not VerificationFileSize(Handle, Size) or
      not AcquireHashProvider(Provider, 0, 0, 24, $F0000000) or
      not CreateVerificationHash(Provider, $800C, 0, 0, Hash) then
      RaiseException('Cannot initialize SHA-256 verification');
    Done := 0;
    repeat
      VerificationPage.SetProgress(Done div 65536, (Size div 65536) + 1);
      if VerificationAborted then break;
      if not ReadVerificationFile(Handle, Buffer[0], 1048576, Count, 0) then
        RaiseException('Cannot read file for verification');
      if Count = 0 then break;
      if not AddVerificationBytes(Hash, Buffer[0], Count, 0) then
        RaiseException('Cannot update SHA-256 verification');
      Done := Done + Count;
    until False;
    if not VerificationAborted then begin
      DigestSize := 32;
      if (Done <> Size) or not ReadVerificationHash(Hash, 2, Digest[0], DigestSize, 0) or
        (DigestSize <> 32) then RaiseException('Cannot finish SHA-256 verification');
      for I := 0 to 31 do Actual := Actual + Format('%.2x', [Digest[I]]);
      VerificationPage.SetProgress(1, 1);
    end;
  finally
    if Hash <> 0 then DestroyVerificationHash(Hash);
    if Provider <> 0 then ReleaseHashProvider(Provider, 0);
    CloseVerificationFile(Handle);
    VerificationCancel.Visible := False;
    VerificationPage.Hide;
  end;
  Matches := not VerificationAborted and (CompareText(Actual, Expected) = 0);
  Log('SHA-256 verification result: ' + IntToStr(Ord(Matches)) + ' for ' + Name);
end;
