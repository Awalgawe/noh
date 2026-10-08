"""Measure production raw-cache selection, including the pre-QA FFmpeg consumer."""
import argparse
import hashlib
import json
import math
from pathlib import Path
import platform
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / "tools"))
import runtime_cache


def write(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("x", encoding="utf-8") as stream:
        json.dump(value, stream, indent=2)
        stream.write("\n")


def read(path):
    return json.loads(path.read_text(encoding="utf-8"))


def digest(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def acquire(args):
    if sys.platform != "win32" or sys.version_info[:2] != (3, 14):
        raise ValueError("Windows and CPython 3.14 required")
    import delivery
    import windows_test_runtime

    cache = args.cache.resolve(strict=True)
    if args.arm == "baseline" and any(cache.iterdir()):
        raise ValueError("Baseline requires an empty raw cache")
    if args.output.exists() or args.record.exists():
        raise FileExistsError("Fresh outputs and evidence required")
    paths = runtime_cache.LOCKS + runtime_cache.PRODUCERS + ("tools/runtime_cache.py",)
    record = {
        "arm": args.arm, "success": False, "python": sys.version,
        "platform": platform.platform(), "keys": runtime_cache.resolve(),
        "commit": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip(),
        "source": {name: digest(ROOT / name) for name in paths},
        "harness_sha256": digest(Path(__file__)), "requests": [], "phases": {},
        "scope": "Raw-cache restore, pre-QA FFmpeg preparation, full validated runtime; Rust/QA gap excluded",
    }
    fetch = delivery.fetch
    phase = "test-runtime"

    def tracked_fetch(spec, requested_cache, name):
        path = Path(requested_cache) / str(delivery.relative(name))
        cached = path.is_file()
        if args.arm == "candidate" and not cached:
            raise FileNotFoundError("Candidate cached input missing; download refused: " + name)
        started = time.perf_counter()
        result = fetch(spec, requested_cache, name)  # Includes the production SHA-256 check.
        record["requests"].append({"phase": phase, "name": name, "sha256": spec["sha256"],
                                   "bytes": path.stat().st_size, "cached": cached,
                                   "seconds": time.perf_counter() - started})
        return result

    delivery.fetch = tracked_fetch
    started = time.perf_counter()
    try:
        # Match windows_test_runtime.main, including the containing lock check.
        spec = delivery.read(delivery.ROOT / "assets/delivery-windows.lock.json")["native_runtime"]
        lock = delivery.ROOT / str(delivery.relative(spec["lock"]))
        if delivery.digest(lock) != spec["sha256"]:
            raise ValueError("Native runtime lock checksum mismatch")
        phase_started = time.perf_counter()
        windows_test_runtime.acquire(delivery.read(lock), cache, args.output / "test-runtime")
        record["phases"][phase] = time.perf_counter() - phase_started
        phase = "full-runtime"
        phase_started = time.perf_counter()
        delivery.windows_inputs(cache, args.output / "full-runtime")
        record["phases"][phase] = time.perf_counter() - phase_started
        record["success"] = True
    except Exception as error:
        record["error"] = str(error)
        raise
    finally:
        record["seconds"] = time.perf_counter() - started
        if record["success"]:
            # Extra byte comparison is outside the production acquisition timer.
            record["test_inventory"] = delivery.inventory(args.output / "test-runtime")
            record["full_inventory"] = delivery.inventory(args.output / "full-runtime")
        write(args.record, record)
        print(json.dumps({key: value for key, value in record.items()
                          if key not in {"requests", "test_inventory", "full_inventory"}}, indent=2))


def compare(baseline, candidate):
    if baseline["arm"] != "baseline" or candidate["arm"] != "candidate":
        raise ValueError("Expected one baseline and one candidate")
    for record in (baseline, candidate):
        if not record["success"] or not record["test_inventory"] or not record["full_inventory"]:
            raise ValueError("Both consumers must succeed with nonempty inventories")
        if not math.isfinite(record["seconds"]) or record["seconds"] <= 0:
            raise ValueError("Invalid production acquisition duration")
    for key in ("python", "platform", "keys", "commit", "source", "harness_sha256",
                "scope", "test_inventory", "full_inventory"):
        if baseline[key] != candidate[key]:
            raise ValueError("Uncontrolled comparison difference: " + key)
    def requests(record):
        return [{key: item[key] for key in ("phase", "name", "sha256", "bytes")}
                for item in record["requests"]]
    if not baseline["requests"] or requests(baseline) != requests(candidate):
        raise ValueError("Requested locked objects differ")
    if not any(not item["cached"] for item in baseline["requests"]):
        raise ValueError("Baseline did not perform a cold acquisition")
    if any(not item["cached"] for item in candidate["requests"]):
        raise ValueError("Candidate performed a network fetch")
    saved = baseline["seconds"] - candidate["seconds"]
    return {
        "equal_outputs": True, "test_files": len(baseline["test_inventory"]),
        "full_files": len(baseline["full_inventory"]), "fetch_calls": len(baseline["requests"]),
        "baseline_network_bytes": sum(item["bytes"] for item in baseline["requests"] if not item["cached"]),
        "candidate_network_bytes": 0, "baseline_seconds": baseline["seconds"],
        "candidate_seconds": candidate["seconds"], "acquisition_saved_seconds": saved,
        "promotion_eligible": saved >= 120 and saved / baseline["seconds"] >= 0.20,
        "final_decision_pending": True,
        "scope": "Prescreen only; final selection must charge actual cache restore and full promotion costs",
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    run = commands.add_parser("acquire")
    run.add_argument("--arm", choices=("baseline", "candidate"), required=True)
    for name in ("cache", "output", "record"):
        run.add_argument("--" + name, type=Path, required=True)
    comparison = commands.add_parser("compare")
    for name in ("baseline", "candidate", "record", "github-output"):
        comparison.add_argument("--" + name, type=Path, required=True)
    args = parser.parse_args()
    if args.command == "acquire":
        acquire(args)
    else:
        value = compare(read(args.baseline), read(args.candidate))
        write(args.record, value)
        with args.github_output.open("a", encoding="utf-8") as stream:
            stream.write("eligible=" + str(value["promotion_eligible"]).lower() + "\n")
        print(json.dumps(value, indent=2))


if __name__ == "__main__":
    main()
