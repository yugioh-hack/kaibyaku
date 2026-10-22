"""Whole-process Python/Rust benchmark with isolated user data and fresh outputs."""
import argparse
import csv
from dataclasses import asdict
import json
import os
from pathlib import Path
import platform
import shutil
import statistics
import subprocess
import sys
import time
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]


def python_run(argv):
    parser = argparse.ArgumentParser()
    parser.add_argument("--python-run", action="store_true")
    parser.add_argument("--images", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--user-data", type=Path, required=True)
    parser.add_argument("--workers", type=int, required=True)
    args = parser.parse_args(argv)
    sys.path.insert(0, str(ROOT))
    from mdript.catalog import Catalog
    from mdript.pipeline import Cropper, Options

    with patch("mdript.catalog.user_data_dir", return_value=str(args.user_data)):
        catalog = Catalog()
    options = Options(output_dir=str(args.output), workers=args.workers)
    summary = Cropper(catalog, options).process([str(args.images)])
    print(json.dumps(asdict(summary), ensure_ascii=False))


def relative_path(value, output):
    return str(Path(value).relative_to(output)) if value else ""


def snapshot(summary, output):
    rows = []
    for row in summary["results"]:
        rows.append({
            field: relative_path(row["saved_path"], output) if field == "saved_path" else row[field]
            for field in ("source", "status", "card_id", "name", "distance", "margin", "saved_path", "note", "english")
        })
    with (output / "log.csv").open(encoding="utf-8-sig", newline="") as handle:
        csv_rows = list(csv.reader(handle))
    normalized_csv = []
    for row in csv_rows[1:]:
        assert len(row) == 9, row
        normalized_csv.append(row[1:6] + [relative_path(row[6], output)] + row[7:])
    names = sorted(str(p.relative_to(output)) for p in output.rglob("*") if p.is_file() and p.name != "log.csv")
    return {
        "counts": {key: summary[key] for key in ("confirmed", "review", "not_card", "error", "skipped")},
        "rows": rows,
        "csv": normalized_csv,
        "filenames": names,
        "csv_header": csv_rows[0],
    }


def differences(expected, actual, allowed):
    issues = []
    if expected["counts"] != actual["counts"]:
        issues.append("status counts differ")
    if expected["filenames"] != actual["filenames"]:
        issues.append("saved filename list differs")
    if expected["csv_header"] != actual["csv_header"]:
        issues.append("CSV header differs")
    if len(expected["rows"]) != len(actual["rows"]) or len(expected["csv"]) != len(actual["csv"]):
        issues.append("row count differs")
        return issues
    for index, (left, right) in enumerate(zip(expected["rows"], actual["rows"])):
        name = Path(left["source"]).name
        for key in left:
            if left[key] != right[key] and key not in allowed.get(name, set()):
                issues.append(f"JSON row {index} {name} {key}: {left[key]!r} != {right[key]!r}")
    for index, (left, right) in enumerate(zip(expected["csv"], actual["csv"])):
        name = Path(left[7]).name
        for field, (a, b) in enumerate(zip(left, right)):
            if a != b and (field, name) not in {(3, name) for name in allowed if "distance" in allowed[name]} | {(4, name) for name in allowed if "margin" in allowed[name]}:
                issues.append(f"CSV row {index} {name} column {field}: {a!r} != {b!r}")
    return issues


def main(argv):
    parser = argparse.ArgumentParser()
    parser.add_argument("images", type=Path)
    parser.add_argument("--exe", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--report", type=Path, required=True)
    parser.add_argument("--known", type=Path, required=True)
    args = parser.parse_args(argv)
    images, exe = args.images.resolve(), args.exe.resolve()
    assert images.is_dir() and exe.is_file()
    root = args.output.resolve()
    root.mkdir(parents=True, exist_ok=False)
    isolated_appdata = root / "isolated-appdata"
    isolated_appdata.mkdir()
    user_data = root / "empty-user-data"
    user_data.mkdir()
    env = os.environ.copy()
    env["APPDATA"] = str(isolated_appdata)
    env["PYTHONIOENCODING"] = "utf-8"
    known_report = json.loads(args.known.read_text(encoding="utf-8"))
    allowed = {item["file"]: set(item["fields"]) for item in known_report["differences"]}
    modes = ("python_1", "rust_1", "python_default", "rust_default")
    runs = {mode: [] for mode in modes}
    snapshots = {mode: [] for mode in modes}
    for repeat in range(6):
        for mode in modes:
            output = root / f"{mode}-{repeat}"
            output.mkdir()
            workers = 1 if mode.endswith("_1") else 0
            if mode.startswith("python"):
                cmd = [sys.executable, str(Path(__file__).resolve()), "--python-run", "--images", str(images), "--output", str(output), "--user-data", str(user_data), "--workers", str(workers)]
            else:
                cmd = [str(exe), "identify", str(images), "--output", str(output), "--user-data", str(user_data)]
                if workers:
                    cmd.extend(["--workers", str(workers)])
            start = time.perf_counter()
            process = subprocess.run(cmd, capture_output=True, env=env, timeout=1200)
            elapsed = time.perf_counter() - start
            if process.returncode != 0:
                raise RuntimeError(f"{mode} repeat {repeat}: {process.stderr[-1000:].decode('utf-8', errors='replace')}")
            summary = json.loads(process.stdout.decode("utf-8"))
            snap = snapshot(summary, output)
            runs[mode].append({"kind": "warmup" if repeat == 0 else "measured", "elapsed_s": elapsed, "counts": snap["counts"], "files": len(snap["filenames"])})
            snapshots[mode].append(snap)
            print(f"{mode} {'warmup' if repeat == 0 else repeat}: {elapsed:.3f}s, {len(snap['filenames'])} files", flush=True)
            # Every run starts with a fresh empty directory. Keep only representative measured outputs.
            if repeat != 1:
                assert output.resolve().is_relative_to(root) and output != root
                shutil.rmtree(output)
    baseline = snapshots["python_1"][1]
    checks = {}
    for mode in modes:
        mode_allowed = allowed if mode.startswith("rust") else {}
        checks[mode] = [differences(baseline, snap, mode_allowed) for snap in snapshots[mode][1:]]
    statistics_by_mode = {}
    for mode in modes:
        times = [item["elapsed_s"] for item in runs[mode][1:]]
        statistics_by_mode[mode] = {"median_s": statistics.median(times), "min_s": min(times), "max_s": max(times), "times_s": times, "warmup_s": runs[mode][0]["elapsed_s"]}
    report = {
        "method": "Fresh output per run; one warmup then five measured process launches per mode; modes interleaved; no concurrent builds or benchmarks",
        "environment": {"platform": platform.platform(), "processor": platform.processor(), "logical_cpus": os.cpu_count(), "python": platform.python_version(), "power": "Windows Balanced (powercfg /getactivescheme)"},
        "images": str(images), "image_count": len(baseline["rows"]),
        "modes": statistics_by_mode,
        "counts": baseline["counts"], "filenames": baseline["filenames"],
        "known_jpeg_difference_files": sorted(allowed),
        "checks": {mode: {"all_passed": all(not item for item in items), "issues_by_run": items} for mode, items in checks.items()},
        "output_examples": {mode: str(root / f"{mode}-1") for mode in modes},
    }
    args.report.write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding="utf-8")
    print(json.dumps({"modes": statistics_by_mode, "checks": {mode: item["all_passed"] for mode, item in report["checks"].items()}}, indent=2), flush=True)
    return 0 if all(item["all_passed"] for item in report["checks"].values()) else 1


if __name__ == "__main__":
    raise SystemExit(python_run(sys.argv[1:]) if "--python-run" in sys.argv else main(sys.argv[1:]))
