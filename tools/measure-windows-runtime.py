"""Compare cached Windows acquisition without building or publishing NOH."""
import argparse
import hashlib
import json
from pathlib import Path
import platform
import subprocess
import sys
import time


def read(path):
    return json.loads(path.read_text(encoding="utf-8"))


def digest(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def acquire(args):
    if sys.version_info[:2] != (3, 14) or sys.platform != "win32":
        raise ValueError("This comparison requires Windows and CPython 3.14")
    source = args.source.resolve(strict=True)
    cache = args.cache.resolve(strict=True)
    # Separate processes select each source tree before importing production code.
    sys.path.insert(0, str(source / "tools"))
    import delivery
    import windows_runtime

    if Path(delivery.__file__).resolve() != source / "tools/delivery.py":
        raise ValueError("Wrong acquisition module source")
    paths = ["tools/delivery.py", "tools/windows_runtime.py", "tools/rust_notices.py",
             "assets/delivery-windows.lock.json", "assets/windows-native.lock.json",
             "assets/speech-bundle.json", "assets/preview-runtime.json",
             "assets/delivery-policy.json", "assets/rust-notices.lock.json"]
    record = {"scope": "Windows warm cached acquisition; no application build or downloads",
              "python": sys.version, "platform": platform.platform(),
              "commit": subprocess.check_output(["git", "-C", str(source), "rev-parse", "HEAD"], text=True).strip(),
              "source": {name: digest(source / name) for name in paths},
              "harness_sha256": digest(Path(__file__)), "fetch_calls": 0,
              "fetch_bytes": 0, "fetch_seconds": 0, "success": False}
    fetch = delivery.fetch
    notices = windows_runtime.native_rust_notices
    def cached_fetch(spec, requested_cache, name):
        path = Path(requested_cache) / str(delivery.relative(name))
        if not path.is_file():
            raise FileNotFoundError("Required cached input missing; download refused: " + name)
        started = time.perf_counter()
        result = fetch(spec, requested_cache, name)
        record["fetch_seconds"] += time.perf_counter() - started
        record["fetch_calls"] += 1
        record["fetch_bytes"] += path.stat().st_size
        return result
    def timed_notices(*values):
        started, cpu = time.perf_counter(), time.process_time()
        try:
            return notices(*values)
        finally:
            record["native_notices_seconds"] = time.perf_counter() - started
            record["native_notices_cpu_seconds"] = time.process_time() - cpu
    delivery.fetch = cached_fetch
    windows_runtime.native_rust_notices = timed_notices
    started, cpu = time.perf_counter(), time.process_time()
    try:
        delivery.windows_inputs(cache, args.output)
        record["success"] = True
    except Exception as error:
        record["error"] = str(error)
        raise
    finally:
        record["seconds"] = time.perf_counter() - started
        record["cpu_seconds"] = time.process_time() - cpu
        if record["success"]:
            # This independent read is outside the timed acquisition command.
            record["inventory"] = delivery.inventory(args.output)
        args.record.parent.mkdir(parents=True, exist_ok=True)
        args.record.write_text(json.dumps(record, indent=2) + "\n", encoding="utf-8")
        print(json.dumps({key: value for key, value in record.items() if key != "inventory"}, indent=2))


def compare(args):
    baseline, candidate = read(args.baseline), read(args.candidate)
    for record in (baseline, candidate):
        if not record["success"] or not record.get("inventory"):
            raise ValueError("Both acquisition arms must succeed with an inventory")
    for key in ("python", "platform", "harness_sha256", "fetch_calls", "fetch_bytes"):
        if baseline[key] != candidate[key]:
            raise ValueError("Comparison inputs differ: " + key)
    if baseline["source"].keys() != candidate["source"].keys():
        raise ValueError("Comparison source inventory differs")
    for name, value in baseline["source"].items():
        if name != "tools/windows_runtime.py" and candidate["source"][name] != value:
            raise ValueError("Uncontrolled acquisition input changed: " + name)
    if baseline["inventory"] != candidate["inventory"]:
        raise ValueError("Acquired output bytes differ")
    result = {"equal_output_inventory": True, "files": len(baseline["inventory"]),
              "baseline_seconds": baseline["seconds"], "candidate_seconds": candidate["seconds"],
              "saved_seconds": baseline["seconds"] - candidate["seconds"],
              "baseline_native_notices_seconds": baseline["native_notices_seconds"],
              "candidate_native_notices_seconds": candidate["native_notices_seconds"],
              "scope": baseline["scope"], "order": "baseline then candidate; raw downloads cached, OS cache uncontrolled"}
    args.record.write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(result, indent=2))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    run = commands.add_parser("acquire")
    for option in ("source", "cache", "output", "record"):
        run.add_argument("--" + option, type=Path, required=True)
    comparison = commands.add_parser("compare")
    for option in ("baseline", "candidate", "record"):
        comparison.add_argument("--" + option, type=Path, required=True)
    args = parser.parse_args()
    (acquire if args.command == "acquire" else compare)(args)


if __name__ == "__main__":
    main()
