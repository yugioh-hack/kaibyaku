"""Fetch one selected official set in Python and compare its hashes with Rust."""
import argparse
import json
from pathlib import Path
import sys
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT))


def distance(a, b):
    return (int(a[:32], 16) ^ int(b[:32], 16)).bit_count()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--pid", required=True)
    parser.add_argument("--rust-file", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    root = args.output.resolve()
    root.mkdir(parents=True, exist_ok=False)
    user = root / "isolated-user-data"
    user.mkdir()
    from mdript.catalog import Catalog
    from mdript.updater import fetch_pack_cards, update_hashes

    members = fetch_pack_cards(args.pid)
    with patch("mdript.catalog.user_data_dir", return_value=str(user)):
        catalog = Catalog()
        done, added, failed, _ = update_hashes(catalog.hashes, members)
        catalog.save_user_hashes()
    rust = json.loads(args.rust_file.read_text(encoding="utf-8"))
    python = json.loads((user / "card_hashes_user.json").read_text(encoding="utf-8"))
    r_hashes, p_hashes = rust["hashes"], python["hashes"]
    different = []
    distances = []
    for cid in members:
        left, right = p_hashes.get(cid, []), r_hashes.get(cid, [])
        if left != right:
            different.append(cid)
        for a, b in zip(left, right):
            distances.append(distance(a, b))
    report = {
        "pid": args.pid, "members": members, "python_cards": done,
        "python_new_hashes": added, "python_failed": failed,
        "python_extra_cards": len(p_hashes), "rust_extra_cards": len(r_hashes),
        "different_cards": different, "different_hash_pairs": sum(value > 0 for value in distances),
        "max_hash_distance": max(distances, default=0),
    }
    (root / "report.json").write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding="utf-8")
    print(json.dumps(report, ensure_ascii=False))
    return 0 if done and not failed and len(p_hashes) == len(r_hashes) else 1


if __name__ == "__main__":
    raise SystemExit(main())
