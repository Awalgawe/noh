"""Identify raw Windows downloads independently of extraction implementation."""
import argparse
import hashlib
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
LOCKS = (
    "assets/delivery-windows.lock.json", "assets/windows-native.lock.json",
    "assets/speech-bundle.json", "assets/preview-runtime.json",
    "assets/delivery-policy.json", "assets/rust-notices.lock.json",
)
PRODUCERS = (
    "tools/delivery.py", "tools/windows_runtime.py",
    "tools/windows_test_runtime.py", "tools/rust_notices.py",
)
# Validated by hosted E1; never fall back across changed locked inputs.
LEGACY_INPUTS = "a0cfdc77e4fba63d2b8c8e5110bac3b5fd3f96cdcd933c07ee8f7368dbc3eb48"
LEGACY_KEY = "noh-runtime-v1-windows-x64-d88c3148333c2bdc3010ac0778a8920f2c165bf5fe17dba938f27600c32be8dd"


def fingerprint(root, paths):
    # Match the existing ordered hashFiles recipe, including actual checkout bytes.
    return hashlib.sha256(b"".join(hashlib.sha256((root / name).read_bytes()).digest()
                                   for name in paths)).hexdigest()


def resolve(root=ROOT):
    inputs = fingerprint(root, LOCKS)
    return {
        # Bump v2 if download paths/selection change without a lock change;
        # re-audit or remove the legacy transition when changing that layout.
        "primary": "noh-runtime-v2-windows-x64-" + inputs,
        "legacy": LEGACY_KEY if inputs == LEGACY_INPUTS else "",
        "locked_inputs": inputs,
        "implementation_key": "noh-runtime-v1-windows-x64-" + fingerprint(root, LOCKS + PRODUCERS),
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--record", type=Path)
    parser.add_argument("--github-output", type=Path)
    args = parser.parse_args()
    value = resolve()
    if args.record:
        args.record.parent.mkdir(parents=True, exist_ok=True)
        with args.record.open("x", encoding="utf-8") as output:
            output.write(json.dumps(value, indent=2) + "\n")
    if args.github_output:
        with args.github_output.open("a", encoding="utf-8") as output:
            output.writelines(f"{key}={item}\n" for key, item in value.items())
    print(json.dumps(value, indent=2))


if __name__ == "__main__":
    main()
