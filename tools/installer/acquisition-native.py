"""Test the shared Inno acquisition engine with synthetic archives on loopback."""
import argparse
import functools
import hashlib
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
import json
from pathlib import Path
import re
import subprocess
import threading
import zipfile

ROOT = Path(__file__).resolve().parent


class QuietHandler(SimpleHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def copyfile(self, source, outputfile):
        try:
            super().copyfile(source, outputfile)
        except (BrokenPipeError, ConnectionResetError, ConnectionAbortedError):
            pass  # Expected when the client cancels the synthetic transfer.


def verify(compiler, output):
    output.mkdir(parents=True, exist_ok=False)
    served = output / 'served'
    served.mkdir()
    archive = served / 'synthetic.zip'
    with zipfile.ZipFile(archive, 'x', zipfile.ZIP_STORED) as package:
        package.writestr('bin/synthetic.bin', bytes(range(256)) * 65536)
    digest = hashlib.sha256(archive.read_bytes()).hexdigest()
    server = ThreadingHTTPServer(('127.0.0.1', 0), functools.partial(QuietHandler, directory=served))
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        languages = '\n'.join(f'Name: "{lang}"; MessagesFile: "compiler:Default.isl"'
                              for lang in ('en', 'fr', 'de', 'es', 'ja', 'ko', 'zh'))
        source = output / 'acquisition.iss'
        source.write_text(f'''#define NohWeb 1
#define DefaultProfile "standard"
#define EmbeddedMedia "False"
#define EmbeddedSpeech "False"
#define ComponentBaseUrl "http://127.0.0.1:{server.server_port}/"
[Setup]
AppName=NOH acquisition test
AppVersion=1
DefaultDirName={{tmp}}\\unused
CreateAppDir=no
Uninstallable=no
PrivilegesRequired=lowest
SetupArchitecture=x64
ArchiveExtraction=full
OutputDir=.
OutputBaseFilename=acquisition
[Languages]
{languages}
#include "{ROOT / 'public-messages.iss'}"
[Code]
var
  ContentPage: TInputOptionWizardPage;
  ContentDownload: TDownloadWizardPage;
  ContentExtraction: TExtractionWizardPage;
  MediaRequested, SpeechRequested, CancelSeen, Passed: Boolean;
  MinimumProfile: Integer;
  Mode: String;
function InstalledProfile: String;
begin Result := ''; end;
function ComponentName(Group: String): String;
begin Result := 'synthetic.zip'; end;
function ComponentHash(Group: String): String;
begin Result := '{digest}'; end;
function ComponentBytes(Group: String): Int64;
begin Result := {archive.stat().st_size}; end;
function ComponentInstalledBytes(Group: String): Int64;
begin Result := 16777216; end;
procedure ComponentFiles(Group: String; Files: TStringList);
begin Files.Add('bin/synthetic.bin=' + '{'a' * 64}'); end;
#include "{ROOT / 'public-verify.iss'}"
#include "{ROOT / 'public-content.iss'}"
function DownloadProgress(Url, Name: String; Progress, ProgressMax: Int64): Boolean;
begin
  if not CancelSeen and (Progress > 0) and
    ((Mode = 'download-cancel') or (Mode = 'retry-download')) then begin
    CancelSeen := True;
    ContentDownload.AbortButton.OnClick(ContentDownload.AbortButton);
  end;
  Result := True;
end;
function ExtractionProgress(ArchiveName, Name: String; Progress, ProgressMax: Int64): Boolean;
begin
  if not CancelSeen and (Progress > 0) and
    ((Mode = 'extraction-cancel') or (Mode = 'retry-extraction')) then begin
    CancelSeen := True;
    ContentExtraction.AbortButton.OnClick(ContentExtraction.AbortButton);
  end;
  Result := True;
end;
procedure InitializeWizard;
begin
  InitializeContent;
  InitializeVerification;
  ContentDownload := CreateDownloadPage('Acquisition test', '', @DownloadProgress);
  ContentExtraction := CreateExtractionPage('Acquisition test', '', @ExtractionProgress);
end;
procedure CheckClean;
begin
  if FileExists(ExpandConstant('{{tmp}}\\synthetic.zip')) or
    DirExists(ExpandConstant('{{tmp}}\\noh-media')) then
    RaiseException('Acquisition left temporary content');
end;
function NextButtonClick(CurPageID: Integer): Boolean;
begin
  Result := True;
  if CurPageID <> wpReady then exit;
  Mode := ExpandConstant('{{param:CASE}}');
  Passed := PrepareContent;
  Log('First acquisition result: ' + IntToStr(Ord(Passed)));
  if not Passed then CheckClean;
  Log('Cancel event reached: ' + IntToStr(Ord(CancelSeen)));
  if (Mode = 'retry-download') or (Mode = 'retry-extraction') then begin
    if Passed or not CancelSeen then RaiseException('Cancellation was not exercised');
    Mode := 'success';
    Passed := PrepareContent;
  end;
  if Passed and not FileExists(ExpandConstant('{{tmp}}\\noh-media\\bin\\synthetic.bin')) then
    RaiseException('Acquisition did not extract the fixture');
  CleanupContent;
  CheckClean;
  Log('Final acquisition result: ' + IntToStr(Ord(Passed)));
  Log('Temporary content removed: 1');
  Result := False;
end;
function PrepareToInstall(var NeedsRestart: Boolean): String;
begin Result := 'Test only: no application installation'; end;
''', encoding='utf-8-sig')
        with (output / 'compiler.log').open('wb') as log:
            subprocess.run([str(compiler / 'ISCC.exe'), str(source)], stdout=log,
                           stderr=subprocess.STDOUT, check=True, timeout=60)
        results = []
        for case in ('success', 'download-cancel', 'extraction-cancel',
                     'retry-download', 'retry-extraction', 'corrupt-cache'):
            cache = output / 'synthetic.zip'
            if case == 'corrupt-cache':
                cache.write_bytes(b'Corrupt user-provided archive')
            log = output / (case + '.log')
            result = subprocess.run([str(output / 'acquisition.exe'), '/VERYSILENT',
                '/SUPPRESSMSGBOXES', '/NORESTART', '/CASE=' + case, '/LOG=' + str(log)], timeout=90)
            text = log.read_text(encoding='utf-8-sig')
            accepted = case in ('success', 'retry-download', 'retry-extraction')
            if (result.returncode == 0 or 'Runtime error' in text
                    or 'Starting the installation process.' in text
                    or f'Final acquisition result: {int(accepted)}' not in text
                    or 'Temporary content removed: 1' not in text):
                raise ValueError('Native acquisition failed: ' + case)
            if ('cancel' in case or 'retry' in case) and 'Cancel event reached: 1' not in text:
                raise ValueError('Native cancellation was not exercised: ' + case)
            if any(Path(p).exists() for p in re.findall(r'Created temporary directory: ([^\r\n]+)', text)):
                raise ValueError('Setup left its temporary directory: ' + case)
            if case == 'corrupt-cache' and cache.read_bytes() != b'Corrupt user-provided archive':
                raise ValueError('Setup altered the adjacent user archive')
            results.append({'case': case, 'acquired': accepted})
        report = {'status': 'passed', 'cases': results,
                  'scope': 'Actual acquisition code; synthetic loopback transfer and native cancellation callbacks, no app installation.'}
        (output / 'ACQUISITION.json').write_text(json.dumps(report, indent=2) + '\n', encoding='utf-8')
        print(json.dumps(report))
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=5)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--compiler', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    verify(args.compiler.resolve(), args.output.resolve())
