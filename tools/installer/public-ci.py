"""Build/test Windows installers from an exact successful Release build.

The qualified application is reused byte for byte. Source, wrapper and workflow
identities remain distinct. Staging never publishes or modifies the live release.
"""
import argparse
import importlib.util
import json
import os
from pathlib import Path
import platform
import re
import shutil
import subprocess
import sys
import time
import zipfile

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools"))
import delivery
import public_components
from setup_delivery import GitHub

spec = importlib.util.spec_from_file_location('delivery_release', ROOT / 'tools/delivery-release.py')
provenance = importlib.util.module_from_spec(spec)
spec.loader.exec_module(provenance)
PROFILES = ('minimal', 'standard', 'complete')
UNINSTALL_KEY = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\NOH.Public_is1"


def require(value, message):
    if not value:
        raise ValueError(message)


def runner():
    require(os.name == "nt" and os.environ.get("GITHUB_ACTIONS") == "true"
            and os.environ.get("GITHUB_REPOSITORY_ID") == "1411000018"
            and os.environ.get("NOH_RUNNER_ENVIRONMENT") == "github-hosted",
            "This installation test requires the disposable hosted Windows runner")
    return Path(os.environ["RUNNER_TEMP"]) / "noh-public-installer"


def prepare(work):
    work.mkdir(exist_ok=False)
    delivery.authorize_redistribution("windows-x64")
    source, attempt, run_id, workflow = (os.environ[name] for name in
        ('NOH_SOURCE_COMMIT', 'NOH_SOURCE_ATTEMPT', 'NOH_SOURCE_RUN', 'NOH_DELIVERY_WORKFLOW_ID'))
    require(re.fullmatch('[0-9a-f]{40}', source) and
            all(re.fullmatch('[1-9][0-9]*', value) for value in (attempt, run_id, workflow)), 'Exact source inputs required')
    github = GitHub()
    require(github.request('')['id'] == 1411000018, 'Wrong repository')
    run_record = github.request('/actions/runs/' + run_id)
    provenance.validate_run(run_record, 'Awalgawe/noh', int(workflow), source, int(attempt))
    require(github.request('/compare/' + source + '...main')['status'] in ('ahead', 'identical'),
            'Source is not retained on trusted main')
    inventory = github.request('/actions/runs/' + run_id + '/artifacts?per_page=100')
    platforms = ['windows-x64']
    if any(a['name'] == 'delivery-macos-arm64' for a in inventory['artifacts']):
        platforms.append('macos-arm64')
    provenance.validate_jobs(github.request('/actions/runs/' + run_id + '/attempts/' + attempt + '/jobs?per_page=100'),
                             platforms, run_record['path'])
    artifacts = provenance.validate_artifacts(inventory, platforms, 'rust-audit-' + run_id + '-' + attempt)
    artifact = next(a for a in artifacts if a['name'] == 'delivery-windows-x64')
    envelope = work / 'candidate.zip'
    github.download('/actions/artifacts/' + str(artifact['id']) + '/zip', envelope,
                    artifact['digest'].removeprefix('sha256:'))
    candidate = work / 'candidate'
    candidate.mkdir()
    with zipfile.ZipFile(envelope) as archive:
        members = delivery.zip_members(archive)
        require(sum(m.file_size for m in members) <= 5 * delivery.LIMIT, 'Candidate expansion exceeds limit')
        archive.extractall(candidate, members=members)
    record = delivery.verify_envelope(candidate)
    require(record['source_commit'] == source and record['run_id'] == run_id
            and record['run_attempt'] == attempt and record['platform'] == 'windows-x64', 'Candidate identity differs')
    archive = candidate / ('NOH-' + record['version'] + '-windows-x64.zip')
    bundle = work / "portable"
    bundle.mkdir()
    with zipfile.ZipFile(archive) as portable_zip:
        members = delivery.zip_members(portable_zip)
        portable_zip.extractall(bundle, members=members)
    delivery.verify_bundle(bundle, "x86_64-pc-windows-gnu", source)
    delivery.write(work / 'SOURCE.json', {'source_commit': source, 'source_run_id': run_id, 'source_run_attempt': attempt,
        'version': record['version'], 'portable_sha256': delivery.digest(archive),
        'manifest_sha256': delivery.digest(bundle / 'manifest.json'), 'artifact_id': artifact['id'],
        'artifact_digest': artifact['digest'], 'delivery_sha256': delivery.digest(candidate / 'DELIVERY.json')})
    # Application and wrapper identities are separate: the current reviewed
    # workflow may fix the wrapper without rebuilding the qualified application.
    for name in ('tools/public-installer.py', 'tools/installer/public.iss',
                 'tools/installer/public-messages.iss', 'tools/installer/public-profile.iss',
                 'tools/installer/public-web.iss', 'tools/installer/public-web.py',
                 'tools/installer/public-ci.py', 'tools/installer/public-content.iss',
                 'tools/public_components.py', 'assets/installer-extraction.lock.json'):
        checked_in = subprocess.check_output(['git', 'show', os.environ['GITHUB_SHA'] + ':' + name], cwd=ROOT)
        require(checked_in.replace(b'\r\n', b'\n') == (ROOT / name).read_bytes().replace(b'\r\n', b'\n'),
                'Installer source differs from the workflow commit: ' + name)
    print("Exact successful build, portable and wrapper verified; independent release qualification remains required", flush=True)


def run(work, name, args, timeout=600, env=None, allow_missing_opengl=False, expect_failure=False):
    started = time.monotonic()
    with (work / (name + ".log")).open("wb") as log:
        result = subprocess.run([str(a) for a in args], cwd=work, env=env,
                                stdout=log, stderr=subprocess.STDOUT, timeout=timeout)
    record = {"name": name, "exit": result.returncode, "seconds": round(time.monotonic() - started, 3)}
    if (allow_missing_opengl and result.returncode == 1
            and (work / (name + ".log")).read_text(encoding="utf-8").strip()
            == 'Error: OpenGL(PainterError("egui_glow requires opengl 2.0+. "))'):
        record["outcome"] = "unavailable: hosted runner lacks OpenGL 2.0"
        print(json.dumps(record), flush=True)
        return record
    print(json.dumps(record), flush=True)
    require((result.returncode != 0) if expect_failure else (result.returncode == 0),
            name + " returned an unexpected exit; inspect " + name + ".log")
    return record


def build(work):
    source = delivery.read(work / 'SOURCE.json')['source_commit']
    public_components.prepare(work / 'portable', source, work / 'components')
    run(work, 'compile-maintenance', [sys.executable, ROOT / 'tools/public-installer.py',
        '--bundle', work / 'portable', '--commit', source, '--profile', 'minimal', '--maintenance',
        '--components', work / 'components', '--compiler', os.environ['NOH_ISCC_DIRECTORY'],
        '--output', work / 'output/maintenance'], 1200)
    for profile in PROFILES:
        run(work, 'compile-' + profile, [sys.executable, ROOT / 'tools/public-installer.py',
            '--bundle', work / 'portable', '--commit', source, '--profile', profile,
            '--components', work / 'components', '--helper', work / 'output/maintenance/noh-components.exe',
            '--compiler', os.environ['NOH_ISCC_DIRECTORY'], '--output', work / 'output' / profile], 1200)
    run(work, 'compile-web', [sys.executable, ROOT / 'tools/installer/public-web.py',
        '--profiles', work / 'output', '--compiler', os.environ['NOH_ISCC_DIRECTORY'],
        '--output', work / 'output/web'], 180)


def registration():
    import winreg
    try:
        with winreg.OpenKey(winreg.HKEY_CURRENT_USER, UNINSTALL_KEY, 0,
                            winreg.KEY_READ | winreg.KEY_WOW64_64KEY) as key:
            return winreg.QueryValueEx(key, "InstallLocation")[0]
    except FileNotFoundError:
        return None


def verify_installed(folder, expected):
    actual = delivery.inventory(folder)
    for name, spec in expected.items():
        require(actual.get(name) == spec, "Installed payload differs: " + name)
    extras = set(actual) - set(expected)
    require(extras <= {"licenses/INNO-SETUP-LICENSE.txt", "unins000.exe", "unins000.dat", "user-data.txt"},
            "Unexpected installed files: " + repr(extras))
    require((folder / "unins000.exe").is_file(), "Uninstaller missing")


def remove_test_installation(work, installed):
    require(registration() and Path(registration()).resolve() == installed.resolve(),
            'Refusing to uninstall a different registered application')
    result = run(work, 'uninstall', [installed / 'unins000.exe', '/VERYSILENT', '/SUPPRESSMSGBOXES',
                                    '/NORESTART', '/LOG=' + str(work / 'uninstall-inno.log')], 180)
    deadline = time.monotonic() + 30
    while time.monotonic() < deadline and (registration() is not None or (installed / 'unins000.exe').exists()):
        time.sleep(0.5)
    require(registration() is None, 'Uninstall registry entry remains')
    require(set(delivery.inventory(installed)) == {'user-data.txt'}, 'Uninstall left app files or removed user data')
    return result


def test_additions(work, setup, record, components):
    """Exercise the exact helper shipped inside Minimal, including repair and refusal."""
    require(registration() is None, 'Refusing to replace an existing NOH installation')
    work.mkdir()
    installed = work / 'installation with spaces'
    catalog = delivery.read(components / 'COMPONENTS.json')
    common = ['/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', '/CURRENTUSER', '/NOICONS',
              '/TASKS=!desktopicon', '/DIR=' + str(installed)]
    results = []

    def execute(name, executable, profile, expected, failure=False, corrupt=False):
        before = delivery.inventory(installed) if installed.exists() else {}
        caches = []
        try:
            for group, item in catalog['groups'].items():
                cache = executable.parent / item['name']
                require(not cache.exists(), 'Test cache path is already occupied')
                if corrupt and group == 'media':
                    cache.write_bytes(b'Synthetic corrupt media archive')
                else:
                    os.link(components / item['name'], cache)
                caches.append(cache)
            results.append(run(work, name, [executable, '/PROFILE=' + profile, *common,
                '/LOG=' + str(work / (name + '-inno.log'))], expect_failure=failure))
        finally:
            for cache in caches:
                cache.unlink()
        if failure:
            require(delivery.inventory(installed) == before, 'Rejected component operation changed the installation')
            if corrupt:
                log = (work / (name + '-inno.log')).read_text(encoding='utf-8-sig')
                require('Adjacent component archive checksum mismatch' in log, 'Failure did not reach the corrupt archive guard')
        else:
            verify_installed(installed, record['profile_files'][expected])

    execute('initial-minimal', setup, 'minimal', 'minimal')
    (installed / 'user-data.txt').write_text('User data must survive component changes.\n', encoding='utf-8')
    helper = installed / 'noh-components.exe'
    execute('reject-corrupt-media', helper, 'standard', 'minimal', failure=True, corrupt=True)
    execute('add-media', helper, 'standard', 'standard')
    (installed / 'bin/ffmpeg.exe').unlink()
    execute('repair-missing-media', helper, 'standard', 'standard')
    execute('add-speech', helper, 'complete', 'complete')
    execute('helper-retains-complete', helper, 'minimal', 'complete')
    execute('minimal-reinstall-retains-complete', setup, 'minimal', 'complete')
    require((installed / 'user-data.txt').read_text(encoding='utf-8') == 'User data must survive component changes.\n',
            'Component changes altered user data')
    results.append(remove_test_installation(work, installed))

    # A fresh Minimal wizard uses the same acquisition code before installing.
    other = work / 'setup-time'
    other.mkdir()
    installed = other / 'installation with spaces'
    common[-1] = '/DIR=' + str(installed)
    execute('minimal-downloads-media-during-setup', setup, 'standard', 'standard')
    (installed / 'user-data.txt').write_text('Keep this file.\n', encoding='utf-8')
    results.append(remove_test_installation(other, installed))
    return {'status': 'passed', 'results': results, 'user_data_preserved': True}


def test_profile(work, profile, records, web):
    work.mkdir()
    installed = work / 'installation with spaces'
    record = records[profile]
    expected = record['installed_files']
    setup = work.parent / 'output' / profile / record['installer']['name']
    require(delivery.digest(setup) == record["installer"]["sha256"], "Installer digest changed")
    results = []
    for stage in ('install-via-web', 'reinstall-offline', 'reinstall-web-keeps-profile'):
        command = [setup] if stage == 'reinstall-offline' else [web]
        if stage == 'install-via-web':
            command.append('/PROFILE=' + profile)
        results.append(run(work, stage, command + ["/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART",
                                       "/CURRENTUSER", "/NOICONS", "/LANG=fr", "/TASKS=!desktopicon",
                                       "/DIR=" + str(installed), "/LOG=" + str(work / (stage + "-inno.log"))]))
        require(registration() and Path(registration()).resolve() == installed.resolve(),
                "Per-user uninstall registration points elsewhere")
        verify_installed(installed, expected)
        marker = installed / "user-data.txt"
        if stage == 'install-via-web':
            marker.write_text("Preserve user data during reinstall and uninstall.\n", encoding="utf-8")
        else:
            require(marker.read_text(encoding="utf-8") == "Preserve user data during reinstall and uninstall.\n",
                    "Reinstall changed user data")
    before = delivery.inventory(installed)
    for name, command in (('reject-other-folder', [setup]),):
        destination = (work / 'other location') if name == 'reject-other-folder' else installed
        results.append(run(work, name, command + ['/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART',
            '/DIR=' + str(destination), '/LOG=' + str(work / (name + '-inno.log'))], expect_failure=True))
        require(delivery.inventory(installed) == before, 'Rejected operation changed the installed payload')
        require(not (work / 'other location').exists(), 'Rejected operation created another installation')
    env = {k: v for k, v in os.environ.items() if not k.startswith("NOH_")}
    env["PATH"] = os.environ["SystemRoot"] + "/System32;" + os.environ["SystemRoot"]
    env["APPDATA"] = str(work / "profile/roaming")
    env["LOCALAPPDATA"] = str(work / "profile/local")
    for key in ("APPDATA", "LOCALAPPDATA"):
        Path(env[key]).mkdir(parents=True)
    env["NOH_CAPTURE_UI"] = str(work / "installed-gui.ppm")
    env["NOH_LANGUAGE"] = "fr"
    results.append(run(work, "installed-gui", [installed / "noh.exe"], 90, env, allow_missing_opengl=True))
    gui_passed = results[-1]["exit"] == 0
    if gui_passed:
        gui = delivery.read(work / "installed-gui.json")
        require(gui["capture_ready"] and not gui["timed_out"], "Installed GUI capture failed")
    results.append(run(work, "installed-cli", [installed / "bin/noh-cli.exe", "--version"], 30, env))
    results.append(run(work, "uninstall", [installed / "unins000.exe", "/VERYSILENT",
                                          "/SUPPRESSMSGBOXES", "/NORESTART",
                                          "/LOG=" + str(work / "uninstall-inno.log")], 180))
    deadline = time.monotonic() + 30
    while time.monotonic() < deadline and (registration() is not None or (installed / "unins000.exe").exists()):
        time.sleep(0.5)
    require(registration() is None, "Uninstall registry entry remains")
    remaining = delivery.inventory(installed)
    require(set(remaining) == {"user-data.txt"}, "Uninstall left application files or removed user data")
    require(delivery.digest(setup) == record["installer"]["sha256"], "Test changed installer bytes")
    return {"status": "passed", "profile": profile,
              "results": results, "installed_files_verified": len(expected),
              "installed_gui_passed": gui_passed,
              "user_data_preserved": True, "uninstall_registration_removed": True}


def test(work):
    require(registration() is None, 'Refusing to replace an existing NOH installation')
    records = {p: delivery.read(work / 'output' / p / 'INSTALLER.json') for p in PROFILES + ('web', 'maintenance')}
    helper_spec = records['maintenance']['installer']
    for profile in PROFILES:
        require(records[profile]['installed_files']['noh-components.exe'] ==
                {key: helper_spec[key] for key in ('size', 'sha256')}, 'Profile includes another maintenance helper')
    web = work / 'output/web' / records['web']['installer']['name']
    require(delivery.digest(web) == records['web']['installer']['sha256'], 'Web installer changed')
    # Exercise the final web executable before public URLs exist, using its exact
    # hash-checked adjacent cache. Real HTTPS download acceptance follows staging.
    for profile in PROFILES:
        asset = records[profile]['installer']
        require(records['web']['profiles'][profile] == asset, 'Web installer references different bytes')
        os.link(work / 'output' / profile / asset['name'], web.parent / asset['name'])
    results = [test_profile(work / ('test-' + p), p, records, web) for p in PROFILES]
    additions = test_additions(work / 'test-additions',
        work / 'output/minimal' / records['minimal']['installer']['name'], records['minimal'], work / 'components')
    guard = work / 'test-web-rejection'
    guard.mkdir()
    isolated_web = guard / web.name
    shutil.copy2(web, isolated_web)
    (guard / records['minimal']['installer']['name']).write_bytes(b'Corrupt download cache')
    rejects = []
    for profile in ('minimal', 'unknown'):
        rejects.append(run(guard, 'reject-' + profile, [isolated_web, '/PROFILE=' + profile,
            '/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', '/DIR=' + str(guard / 'application'),
            '/LOG=' + str(guard / (profile + '-inno.log'))], expect_failure=True))
        require(registration() is None and not (guard / 'application').exists(), 'Rejected web run installed files')
    source = delivery.read(work / 'SOURCE.json')
    report = {**source, "status": "passed",
              "component_catalog_sha256": delivery.digest(work / 'components/COMPONENTS.json'),
              "wrapper_commit": os.environ.get('GITHUB_SHA', 'local-uncommitted'),
              "run_id": os.environ.get('GITHUB_RUN_ID'), "run_attempt": os.environ.get('GITHUB_RUN_ATTEMPT'),
              "host": platform.platform(), "profiles": results, "additions": additions,
              "rejections": rejects, "artifacts": records,
              "limitations": ["Silent profile and cached web installation; no interactive wizard acceptance.",
                              "The web executable verifies adjacent cached installers; real HTTPS acquisition is a separate acceptance check.",
                              "Microsoft prerequisite download is not executed in silent mode.",
                              "The exact OpenGL-unavailable runner error is recorded, not a GUI pass; local installed-GUI validation is required before publication when it occurs.",
                              "Audio/GPU/media qualification of the source portable remains a separate release gate.",
                              "No Authenticode signing or SmartScreen reputation is claimed."]}
    delivery.write(work / "output/INSTALLER-TESTS.json", report)


def stage(work):
    report = delivery.read(work / "output/INSTALLER-TESTS.json")
    require(report["status"] == "passed" and report["wrapper_commit"] == os.environ["GITHUB_SHA"]
            and report["run_id"] == os.environ["GITHUB_RUN_ID"]
            and report["run_attempt"] == os.environ["GITHUB_RUN_ATTEMPT"], "Test provenance differs")
    delivery.authorize_redistribution("windows-x64")
    github = GitHub()
    repo = github.request("")
    require(repo["id"] == 1411000018 and repo["full_name"] == "Awalgawe/noh", "Wrong repository")
    paths = [work / 'output/INSTALLER-TESTS.json']
    for profile, record in report['artifacts'].items():
        for key in ('installer', 'materials'):
            spec = record[key]
            path = work / 'output' / profile / spec['name']
            require(path.stat().st_size == spec['size'] and delivery.digest(path) == spec['sha256'], 'Tested bytes changed')
            paths.append(path)
    catalog = delivery.read(work / 'components/COMPONENTS.json')
    require(delivery.digest(work / 'components/COMPONENTS.json') == report['component_catalog_sha256'],
            'Tested component catalog changed')
    public_components.validate(catalog, work / 'components')
    paths.append(work / 'components/COMPONENTS.json')
    paths.extend(work / 'components' / item['name'] for item in catalog['groups'].values())
    evidence = work / "output/installer-test-evidence.zip"
    with zipfile.ZipFile(evidence, "x", zipfile.ZIP_DEFLATED) as archive:
        for path in sorted(work.rglob('*.log')) + sorted(work.glob('test-*/installed-gui.*')):
            archive.write(path, path.relative_to(work).as_posix())
    paths.append(evidence)
    tag = "profile-installer-check-v" + report['version'] + '-' + os.environ["GITHUB_RUN_ID"] + "-" + os.environ["GITHUB_RUN_ATTEMPT"]
    draft = github.request("/releases", "POST", {"tag_name": tag, "target_commitish": os.environ["GITHUB_SHA"],
        "name": "Windows profile installers review (unpublished)", "draft": True, "prerelease": False,
        "body": "Exact hosted installer test output; independent review pending. Do not publish this staging release."})
    assets = []
    for path in paths:
        asset = github.upload(draft["id"], path, path.name, delivery.digest(path))
        assets.append({key: asset[key] for key in ("id", "name", "digest", "size")})
    receipt = {"draft_id": draft["id"], "tag": tag, "assets": assets, "wrapper_commit": os.environ["GITHUB_SHA"]}
    print(json.dumps(receipt, indent=2), flush=True)
    with open(os.environ["GITHUB_STEP_SUMMARY"], "a", encoding="utf-8") as summary:
        summary.write("Installer built and tested. Unpublished review draft: " + str(draft["id"]) + "\n\n")
        summary.write("```json\n" + json.dumps(receipt, indent=2) + "\n```\n")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("stage", choices=("prepare", "build", "test", "stage"))
    arguments = parser.parse_args()
    globals()[arguments.stage](runner())
