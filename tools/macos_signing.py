"""Prepare or execute Developer ID signing; execution requires an explicit CI choice."""
import argparse
import base64
import json
import os
from pathlib import Path
import platform
import plistlib
import re
import secrets
import subprocess
import tempfile

import delivery

MAGIC = {bytes.fromhex(value) for value in ("cffaedfe", "feedfacf", "cefaedfe", "feedface", "cafebabe", "bebafeca", "cafebabf", "bfbafeca")}
SUBMISSION = re.compile(r"[0-9a-fA-F]{8}-(?:[0-9a-fA-F]{4}-){3}[0-9a-fA-F]{12}")


def run(command, timeout=180):
    # Never include command arguments or stderr in exceptions: credential
    # operations contain passwords, while subprocess exceptions echo arguments.
    try:
        result = subprocess.run([str(arg) for arg in command], capture_output=True, text=True, timeout=timeout)
    except subprocess.TimeoutExpired:
        raise ValueError(Path(command[0]).name + " timed out") from None
    if result.returncode:
        raise ValueError(Path(command[0]).name + " failed (exit " + str(result.returncode) + ")")
    return result.stdout + result.stderr


def plan(bundle, commit):
    if not delivery.COMMIT.fullmatch(commit):
        raise ValueError("Expected an exact source commit")
    manifest = delivery.verify_bundle(bundle, "aarch64-apple-darwin", commit)
    app = bundle / "NOH.app"
    with (app / "Contents/Info.plist").open("rb") as source:
        info = plistlib.load(source)
    if info.get("CFBundleExecutable") != "noh-app" or info.get("CFBundlePackageType") != "APPL":
        raise ValueError("Unexpected application layout")
    code = []
    for name in delivery.inventory(app):
        path = app / name
        with path.open("rb") as source:
            if source.read(4) in MAGIC:
                code.append(name)
    recorded = {item["path"] for item in manifest["runtime_dependencies"]["portable"]["mach_o"]}
    if set(code) != recorded or "Contents/MacOS/noh-app" not in code:
        raise ValueError("Signing requires the exact recorded Mach-O closure")
    if any(path != app and path.suffix in {".app", ".framework", ".bundle"} for path in app.rglob("*") if path.is_dir()):
        raise ValueError("Nested code bundles require an explicit signing layout")
    code.sort(key=lambda name: (-name.count("/"), name))
    return {"status": "unexecuted-signing-plan", "source_commit": commit,
            "app": "NOH.app", "code": code, "signature_options": ["runtime", "timestamp"],
            "qualification": "not established by signing or notarization"}


def require_ci_host():
    if (platform.system() != "Darwin" or platform.machine() != "arm64"
            or os.environ.get("GITHUB_ACTIONS") != "true"
            or os.environ.get("GITHUB_REF") != "refs/heads/main"):
        raise ValueError("Signing execution requires an explicitly approved main-branch Apple Silicon GitHub runner")


def signature_check(app, code, team):
    for path in [*(app / name for name in code), app]:
        details = run(["/usr/bin/codesign", "--display", "--verbose=4", path])
        lines = details.splitlines()
        if (not any(line.startswith("Authority=Developer ID Application:") for line in lines)
                or "TeamIdentifier=" + team not in lines
                or not any(re.search(r"^CodeDirectory\b.*\bflags=0x([0-9a-fA-F]+)\(.*\bruntime\b", line)
                           and int(re.search(r"\bflags=0x([0-9a-fA-F]+)", line)[1], 16) & 0x10000
                           for line in lines)
                or not any(line.startswith("Timestamp=") and line[10:].strip() not in {"", "none", "<none>"} for line in lines)):
            raise ValueError("Developer ID/team/runtime/timestamp verification failed")
    run(["/usr/bin/codesign", "--verify", "--deep", "--strict", app])


def accepted_submission(result, log):
    submission = result.get("id", "")
    if not SUBMISSION.fullmatch(submission) or result.get("status") != "Accepted":
        raise ValueError("Apple did not accept notarization")
    if log.get("jobId", "").lower() != submission.lower() or log.get("status") != "Accepted" or "issues" not in log or log["issues"]:
        raise ValueError("Notarization log mismatch or issues require review")
    return submission


def configuration(identity, team):
    if not re.fullmatch(r"[0-9A-Fa-f]{40}", identity) or not re.fullmatch(r"[A-Z0-9]{10}", team):
        raise ValueError("Expected an exact signing certificate SHA-1 and Apple team identifier")
    credentials = {name: os.environ.get(name, "") for name in
                   ("NOH_MACOS_CERTIFICATE_BASE64", "NOH_MACOS_CERTIFICATE_PASSWORD", "NOH_APPLE_ID", "NOH_APPLE_APP_PASSWORD")}
    if not all(credentials.values()):
        raise ValueError("Owner-provided Developer ID and notarization credentials are required")
    certificate = base64.b64decode(credentials["NOH_MACOS_CERTIFICATE_BASE64"], validate=True)
    if not certificate:
        raise ValueError("Empty signing certificate input")
    return credentials, certificate


def execute(bundle, commit, identity, team):
    require_ci_host()
    credentials, certificate = configuration(identity, team)
    report_path = bundle / "MACOS-SIGNING.json"
    if report_path.exists():
        raise ValueError("Refusing to overwrite signing evidence")
    signing = plan(bundle, commit)
    app = bundle / signing["app"]
    with tempfile.TemporaryDirectory(prefix="noh-signing-", dir=os.environ["RUNNER_TEMP"]) as temporary:
        work = Path(temporary)
        keychain = work / "signing.keychain-db"
        cert = work / "developer-id.p12"
        cert.write_bytes(certificate)
        cert.chmod(0o600)
        password = secrets.token_hex(32)
        created = False
        try:
            run(["/usr/bin/security", "create-keychain", "-p", password, keychain])
            created = True
            run(["/usr/bin/security", "set-keychain-settings", "-lut", "7200", keychain])
            run(["/usr/bin/security", "unlock-keychain", "-p", password, keychain])
            run(["/usr/bin/security", "import", cert, "-k", keychain, "-P", credentials["NOH_MACOS_CERTIFICATE_PASSWORD"], "-T", "/usr/bin/codesign"])
            run(["/usr/bin/security", "set-key-partition-list", "-S", "apple-tool:,apple:,codesign:", "-s", "-k", password, keychain])
            identities = run(["/usr/bin/security", "find-identity", "-v", "-p", "codesigning", keychain])
            if not any(identity.lower() in line.lower() and '"Developer ID Application:' in line and "(" + team + ")" in line for line in identities.splitlines()):
                raise ValueError("Requested Developer ID certificate/team is not in the temporary keychain")
            run(["/usr/bin/xcrun", "notarytool", "store-credentials", "noh-notary", "--apple-id", credentials["NOH_APPLE_ID"], "--team-id", team,
                 "--password", credentials["NOH_APPLE_APP_PASSWORD"], "--keychain", keychain])
            # Update the development provenance wording before signatures seal it.
            runtime_path = app / "Contents/Resources/PORTABLE-RUNTIME.json"
            runtime = delivery.read(runtime_path)
            runtime["signature"] = "Developer ID signing requested; final evidence is outside the app in MACOS-SIGNING.json"
            delivery.write(runtime_path, runtime)
            for readme in (bundle / "README.md", app / "Contents/Resources/README.md"):
                data = readme.read_bytes().replace(
                    b"This app has an ad-hoc integrity signature, not a Developer ID signature or\nApple notarization. It is a private portable build, not a public release.",
                    b"This is a private signing candidate. See MACOS-SIGNING.json alongside NOH.app\nfor signing/notarization evidence. Public release qualification remains separate.")
                readme.write_bytes(data)
            for name in signing["code"]:
                path = app / name
                run(["/usr/bin/lipo", path, "-verify_arch", "arm64"])
                run(["/usr/bin/codesign", "--force", "--sign", identity, "--keychain", keychain, "--timestamp", "--options", "runtime", path])
            run(["/usr/bin/codesign", "--force", "--sign", identity, "--keychain", keychain, "--timestamp", "--options", "runtime", app])
            signature_check(app, signing["code"], team)
            submission_zip = work / "submission.zip"
            run(["/usr/bin/ditto", "-c", "-k", "--keepParent", app, submission_zip], timeout=900)
            auth = ["--keychain-profile", "noh-notary", "--keychain", keychain]
            result = json.loads(run(["/usr/bin/xcrun", "notarytool", "submit", submission_zip, *auth, "--wait", "--timeout", "1h", "--output-format", "json"], timeout=3900))
            # Preserve Apple's log even when the submission is rejected.
            submission = result.get("id", "")
            if not SUBMISSION.fullmatch(submission):
                raise ValueError("Invalid Apple submission identifier")
            log_path = bundle / "MACOS-NOTARY-LOG.json"
            if log_path.exists():
                raise ValueError("Refusing to overwrite notarization log")
            run(["/usr/bin/xcrun", "notarytool", "log", submission, *auth, log_path])
            log = delivery.read(log_path)
            accepted_submission(result, log)
            run(["/usr/bin/xcrun", "stapler", "staple", app])
            run(["/usr/bin/xcrun", "stapler", "validate", app])
            signature_check(app, signing["code"], team)
            run(["/usr/sbin/spctl", "--assess", "--type", "execute", "--verbose=2", app])
            report = signing | {"status": "signed-notarized-stapled-candidate", "team_id": team,
                                "certificate_sha1": identity.lower(), "submission_id": submission,
                                "notary_log_sha256": delivery.digest(log_path), "app_files": delivery.inventory(app),
                                "qualification": "native commands passed; clean/download/audio/GPU/media and independent qualification remain required"}
            delivery.write(report_path, report)
            manifest = delivery.read(bundle / "manifest.json")
            manifest["runtime_dependencies"]["portable"]["signature"] = "Developer ID; notarized and stapled; see MACOS-SIGNING.json"
            manifest["runtime_dependencies"]["signing"] = {"report": report_path.name, "sha256": delivery.digest(report_path)}
            files = delivery.inventory(bundle)
            manifest["sha256"] = {name: item["sha256"] for name, item in files.items() if name != "manifest.json"}
            ffmpeg_path = manifest["tools"]["ffmpeg"]["path"]
            manifest["tools"]["ffmpeg"]["sha256"] = manifest["sha256"][ffmpeg_path]
            delivery.write(bundle / "manifest.json", manifest)
            delivery.verify_bundle(bundle, "aarch64-apple-darwin", commit)
        finally:
            if created:
                run(["/usr/bin/security", "delete-keychain", keychain])


def verify_archive(archive, bundle):
    """Check the exact final distribution archive, including ticket transport."""
    require_ci_host()
    report = delivery.read(bundle / "MACOS-SIGNING.json")
    with tempfile.TemporaryDirectory(prefix="noh-signed-zip-", dir=os.environ["RUNNER_TEMP"]) as temporary:
        extracted = Path(temporary)
        run(["/usr/bin/ditto", "-x", "-k", archive, extracted], timeout=900)
        app = extracted / "NOH.app"
        if delivery.inventory(app) != report["app_files"]:
            raise ValueError("Final ZIP changes the signed application bytes")
        signature_check(app, report["code"], report["team_id"])
        run(["/usr/bin/xcrun", "stapler", "validate", app])
        run(["/usr/sbin/spctl", "--assess", "--type", "execute", "--verbose=2", app])


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--bundle", type=Path)
    parser.add_argument("--commit")
    parser.add_argument("--preflight", action="store_true", help="Validate host/configuration syntax without signing or uploading software")
    parser.add_argument("--execute", action="store_true", help="Sign and upload privately to Apple's notary service")
    parser.add_argument("--identity", default="")
    parser.add_argument("--team-id", default="")
    args = parser.parse_args()
    if args.preflight:
        if args.execute:
            parser.error("Choose preflight or execution")
        require_ci_host()
        configuration(args.identity, args.team_id)
        print("Mac signing configuration syntax passed; credentials still require native validation")
    elif not args.bundle or not args.commit:
        parser.error("Plan/execution requires --bundle and --commit")
    elif args.execute:
        execute(args.bundle.resolve(), args.commit, args.identity, args.team_id)
        print("Mac signing completed; public qualification remains separate")
    else:
        print(json.dumps(plan(args.bundle.resolve(), args.commit), indent=2))


if __name__ == "__main__":
    try:
        main()
    except (ValueError, OSError, KeyError) as error:
        raise SystemExit("Mac signing refused: " + str(error)) from None
