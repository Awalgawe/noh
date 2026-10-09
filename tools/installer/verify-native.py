"""Exercise the actual Inno SHA-256/abort code on synthetic files (no app install)."""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess

ROOT = Path(__file__).resolve().parent


def verify(compiler, output):
    output.mkdir(parents=True, exist_ok=False)
    source = output / 'verify.iss'
    languages = '\n'.join(f'Name: "{lang}"; MessagesFile: "compiler:Default.isl"'
                          for lang in ('en', 'fr', 'de', 'es', 'ja', 'ko', 'zh'))
    source.write_text(f'''[Setup]
AppName=NOH verification test
AppVersion=1
DefaultDirName={{tmp}}\\unused
CreateAppDir=no
Uninstallable=no
PrivilegesRequired=lowest
SetupArchitecture=x64
OutputDir=.
OutputBaseFilename=verify
[Languages]
{languages}
#include "{ROOT / 'public-messages.iss'}"
[Code]
#include "{ROOT / 'public-verify.iss'}"
function SetTimer(Window, ID: NativeUInt; Milliseconds: Cardinal; Callback: NativeUInt): NativeUInt;
  external 'SetTimer@user32.dll stdcall';
function KillTimer(Window, ID: NativeUInt): Boolean;
  external 'KillTimer@user32.dll stdcall';
var
  Passed: Boolean;
  Timer: NativeUInt;
  CancelPosition: Integer;
procedure TimedCancel(Window: NativeUInt; Message: Cardinal; ID: NativeUInt; Time: Cardinal);
begin
  if VerificationPage.ProgressBar.Position > 0 then begin
    CancelPosition := VerificationPage.ProgressBar.Position;
    KillTimer(0, Timer);
    Timer := 0;
    VerificationCancel.OnClick(VerificationCancel);
  end;
end;
procedure InitializeWizard;
begin
  InitializeVerification;
end;
function NextButtonClick(CurPageID: Integer): Boolean;
begin
  Result := True;
  if CurPageID <> wpReady then exit;
  if ExpandConstant('{{param:CANCEL|0}}') = '1' then
    Timer := SetTimer(0, 0, 1, CreateCallback(@TimedCancel));
  try
    VerifyFile(ExpandConstant('{{param:INPUT}}'), ExpandConstant('{{param:EXPECTED}}'), Passed);
  finally
    if Timer <> 0 then KillTimer(0, Timer);
  end;
  Log('Verification result: ' + IntToStr(Ord(Passed)));
  Log('Verification aborted: ' + IntToStr(Ord(VerificationAborted)));
  Log('Cancellation progress: ' + IntToStr(CancelPosition));
  Result := Passed;
end;
function PrepareToInstall(var NeedsRestart: Boolean): String;
begin
  Result := '';
  if not Passed then Result := 'Verification rejected';
end;
''', encoding='utf-8-sig')
    with (output / 'compiler.log').open('wb') as log:
        subprocess.run([str(compiler / 'ISCC.exe'), str(source)], stdout=log,
                       stderr=subprocess.STDOUT, check=True, timeout=60)
    cases = []
    for name, data in [('empty', b''), ('small', b'abc'),
                       ('block-boundary', bytes(range(256)) * 4096 + b'last bytes')]:
        fixture = output / (name + ' space é.bin')
        fixture.write_bytes(data)
        cases.append((name, fixture, hashlib.sha256(data).hexdigest(), True, False))
    large = output / 'large.bin'
    with large.open('wb') as stream:
        chunk = bytes(range(256)) * 4096
        for _ in range(128):
            stream.write(chunk)
    with large.open('rb') as stream:
        digest = hashlib.file_digest(stream, 'sha256').hexdigest()
    cases += [('large', large, digest, True, False),
              ('changed', large, 'a' * 64, False, False),
              ('missing', output / 'missing.bin', digest, False, False),
              ('cancel-during-read', large, digest, False, True)]
    results = []
    for name, fixture, expected, accepted, cancel in cases:
        log = output / (name + '.log')
        result = subprocess.run([str(output / 'verify.exe'), '/VERYSILENT', '/SUPPRESSMSGBOXES',
            '/NORESTART', '/INPUT=' + str(fixture), '/EXPECTED=' + expected,
            '/CANCEL=' + str(int(cancel)), '/LOG=' + str(log)], timeout=60)
        text = log.read_text(encoding='utf-8-sig')
        if ((result.returncode == 0) != accepted or 'Runtime error' in text
                or f'Verification result: {int(accepted)}' not in text):
            raise ValueError('Native verifier failed: ' + name)
        if not accepted and 'Starting the installation process.' in text:
            raise ValueError('A rejected verification started installation: ' + name)
        if cancel and ('Verification aborted: 1' not in text or 'Cancellation progress: 0\n' in text):
            raise ValueError('Cancellation did not run during an active verification')
        results.append({'case': name, 'exit': result.returncode})
    report = {'status': 'passed', 'cases': results,
              'scope': 'Native hash comparison and cancellation through a timer-delivered wizard event; not human UI acceptance.'}
    (output / 'VERIFICATION.json').write_text(json.dumps(report, indent=2) + '\n', encoding='utf-8')
    print(json.dumps(report))


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--compiler', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    verify(args.compiler.resolve(), args.output.resolve())
