"""Quantify decision-boundary proximity without modifying Python sources or APPDATA."""
import argparse
from collections import Counter
import json
from pathlib import Path
import sys
from unittest.mock import patch

from PIL import Image

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT))
from mdript.catalog import Catalog
from mdript.layout import box_is_on_card_edges
from mdript.pipeline import Cropper, Options


def classification(distance, margin, origin):
    if distance > 36:
        return "カードなし"
    if origin == "estimated":
        confirmed = distance <= 16 and margin >= 6
    else:
        confirmed = (distance <= 16 and margin >= 2) or (distance <= 32 and margin >= 4)
    return "確定" if confirmed else "要確認"


def distance_to_status_change(distance, margin, origin):
    current = classification(distance, margin, origin)
    return min(
        max(abs(other_distance - distance), abs(other_margin - margin))
        for other_distance in range(0, max(40, distance + 2))
        for other_margin in range(0, max(10, margin + 2))
        if classification(other_distance, other_margin, origin) != current
    )


def boundary_gaps(distance, margin, origin):
    thresholds = {"card_or_not_distance_36": abs(distance - 36)}
    if origin == "estimated":
        thresholds.update({"confirmed_distance_16": abs(distance - 16), "confirmed_margin_6": abs(margin - 6)})
    else:
        thresholds.update({
            "confirmed_distance_16": abs(distance - 16),
            "confirmed_distance_32": abs(distance - 32),
            "confirmed_margin_2": abs(margin - 2),
            "confirmed_margin_4": abs(margin - 4),
            "estimated_switch_distance_16": abs(distance - 16),
        })
    return thresholds


def by_source(rows):
    result = {Path(row["source"]).name: row for row in rows}
    if len(result) != len(rows):
        raise ValueError("duplicate source filenames")
    return result


def status_swaps(reference, compare):
    python_rows = by_source(reference["results"])
    rust_rows = by_source(compare["rust_results"])
    if python_rows.keys() != rust_rows.keys():
        raise ValueError("Python/Rust source sets differ")
    swaps = [name for name in python_rows if python_rows[name]["status"] != rust_rows[name]["status"]]
    return {"cases": len(python_rows), "count": len(swaps), "files": swaps}


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--reference", type=Path, required=True)
    parser.add_argument("--compare", type=Path, required=True)
    parser.add_argument("--transformed-reference", type=Path, required=True)
    parser.add_argument("--transformed-compare", type=Path, required=True)
    parser.add_argument("--scenarios", type=Path, required=True)
    parser.add_argument("--user-data", type=Path, required=True)
    parser.add_argument("--report", type=Path, required=True)
    parser.add_argument("--markdown", type=Path, required=True)
    args = parser.parse_args()
    args.user_data.mkdir(parents=True, exist_ok=True)
    reference = json.loads(args.reference.read_text(encoding="utf-8"))
    compare = json.loads(args.compare.read_text(encoding="utf-8"))
    transformed_reference = json.loads(args.transformed_reference.read_text(encoding="utf-8"))
    transformed_compare = json.loads(args.transformed_compare.read_text(encoding="utf-8"))
    scenarios = json.loads(args.scenarios.read_text(encoding="utf-8"))
    if compare["exact_result_matches"] != 248 or len(compare["differences"]) != 8:
        raise ValueError("native comparison is not the expected 248/256 with eight differences")
    with patch("mdript.catalog.user_data_dir", return_value=str(args.user_data)):
        catalog = Catalog()
    cropper = Cropper(catalog, Options(output_dir=str(args.user_data.parent / "unused-output"), workers=1))
    python_rows = by_source(reference["results"])
    rust_rows = by_source(compare["rust_results"])
    if python_rows.keys() != rust_rows.keys():
        raise ValueError("native source sets differ")
    rows = []
    counts = Counter()
    for index, (name, py) in enumerate(python_rows.items(), 1):
        with Image.open(py["source"]) as image:
            rgb = image.convert("RGB")
        found = cropper.identify(rgb)
        if found is None or (found["distance"], found["margin"]) != (py["distance"], py["margin"]):
            raise ValueError(f"Python origin could not be verified for {name}")
        python_origin = found["origin"]
        switch_eligible = (
            python_origin == "trusted"
            and found["box"] != (0, 0) + rgb.size
            and not box_is_on_card_edges(rgb, found["box"])
        )
        rust = rust_rows[name]
        values = {
            "python": {"distance": py["distance"], "margin": py["margin"], "status": py["status"], "origin": python_origin},
            "rust": {"distance": rust["distance"], "margin": rust["margin"], "status": rust["status"], "origin": rust["origin"]},
        }
        near = {}
        for side, value in values.items():
            value["boundary_gaps"] = boundary_gaps(value["distance"], value["margin"], value["origin"])
            near[side] = {name: gap for name, gap in value["boundary_gaps"].items() if gap <= 3}
            value["minimum_status_change_linf"] = distance_to_status_change(value["distance"], value["margin"], value["origin"])
        labels = sorted(set(near["python"]) | set(near["rust"]))
        for label in labels:
            counts[label] += 1
        rows.append({"file": name, **values, "near_within_3": near, "near_labels": labels, "python_frame_switch_eligible_if_above_16": switch_eligible})
        if index % 50 == 0:
            print(f"Boundary analysis {index}/{len(python_rows)}", flush=True)
    scenario_cases = sum(len(case["python"]) for case in scenarios)
    scenario_swaps = [
        {"case": case["case"], "file": name}
        for case in scenarios
        for name, py in by_source(case["python"]).items()
        if py["status"] != by_source(case["rust"])[name]["status"]
    ]
    report = {
        "definition": "Absolute integer gap <=3 to each listed threshold, using each side's own distance/margin/origin. estimated_switch_distance_16 is raw proximity only: frame-position and edge conditions also apply. minimum_status_change_linf is the smallest simultaneous max change in distance or margin that changes status at fixed origin.",
        "image_count": len(rows),
        "near_any_count": sum(bool(row["near_labels"]) for row in rows),
        "near_counts_by_boundary": dict(sorted(counts.items())),
        "frame_switch_eligible_near_16_count": sum(row["python_frame_switch_eligible_if_above_16"] and "estimated_switch_distance_16" in row["near_labels"] for row in rows),
        "effective_status_change_within_3_count": sum(min(row[side]["minimum_status_change_linf"] for side in ("python", "rust")) <= 3 for row in rows),
        "minimum_effective_status_change": min(row[side]["minimum_status_change_linf"] for row in rows for side in ("python", "rust")),
        "observed_status_swaps": {
            "native": status_swaps(reference, compare),
            "transformed": status_swaps(transformed_reference, transformed_compare),
            "synthetic_scenarios": {"cases": scenario_cases, "count": len(scenario_swaps), "files": scenario_swaps},
        },
        "rows": rows,
    }
    args.report.write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding="utf-8")
    lines = [
        "<details>",
        f"<summary>境界から3以内の実画像{report['near_any_count']}枚（ファイル名と両版の値）</summary>",
        "",
        "| ファイル | Python 距離/差 | Rust 距離/差 | 近い境界 |",
        "|---|---:|---:|---|",
    ]
    for row in rows:
        if row["near_labels"]:
            labels = (
                "距離16（確定・枠扱い）"
                if set(row["near_labels"]) == {"confirmed_distance_16", "estimated_switch_distance_16"}
                else ", ".join(row["near_labels"])
            )
            lines.append(f"| {row['file']} | {row['python']['distance']}/{row['python']['margin']} | {row['rust']['distance']}/{row['rust']['margin']} | {labels} |")
    lines += ["", "</details>", ""]
    args.markdown.parent.mkdir(parents=True, exist_ok=True)
    args.markdown.write_text("\n".join(lines), encoding="utf-8")
    print(json.dumps({key: value for key, value in report.items() if key != "rows"}, ensure_ascii=False, indent=2))


if __name__ == "__main__":
    main()
