"""Make one Python official-name fetch and compare with one saved Rust fetch."""
import argparse
import json
from pathlib import Path
import sys
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT))


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--rust-file", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    root = args.output.resolve()
    root.mkdir(parents=True, exist_ok=False)
    user = root / "isolated-user-data"
    user.mkdir()
    from mdript.catalog import Catalog
    from mdript.updater import fetch_card_names

    cards = fetch_card_names()
    with patch("mdript.catalog.user_data_dir", return_value=str(user)):
        Catalog().save_names(cards, fetched="2026-10-22 12:34")
    rust = json.loads(args.rust_file.read_text(encoding="utf-8"))
    python = json.loads((user / "card_names.json").read_text(encoding="utf-8"))
    difference_ids = sorted(cid for cid in set(cards) | set(rust["cards"])
                            if cards.get(cid) != rust["cards"].get(cid))
    report = {
        "python_count": len(cards), "rust_count": rust["meta"]["count"],
        "different_ids": difference_ids, "cards_match": python["cards"] == rust["cards"],
    }
    (root / "report.json").write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding="utf-8")
    print(json.dumps(report, ensure_ascii=False))
    return 0 if report["cards_match"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
