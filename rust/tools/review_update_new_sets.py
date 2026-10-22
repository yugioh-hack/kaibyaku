"""Compare fixed update branches with the unchanged Python implementation."""
import json
from pathlib import Path
import sys
from types import SimpleNamespace
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT))
from mdript import updater

GOLDEN = ROOT / "rust/crates/mdript-cli/tests/fixtures/update-new-sets-golden.json"


def run_case(case):
    hashes = {"4138": ["0" * 32]}
    cards = {"4138": {"n": "Sample"}}
    known = set() if case in ("initial_seed_with_missing_card", "pack_list_failure") else {"11" if case == "new_card_in_known_pack" else "10"}
    if case in ("initial_seed_with_missing_card", "new_card_in_known_pack", "pack_list_failure"):
        cards["99999"] = {"n": "New"}
    if case == "new_pack_reprint":
        cards["99999"] = {"n": "Reprint"}
        hashes["99999"] = ["0" * 32]
    catalog = SimpleNamespace(hashes=hashes, known_packs=known)

    def pack_list(_):
        if case == "pack_list_failure":
            raise OSError("fixed list failure")
        return [{"pid": "11", "name": "Sample Pack"}]

    def pack_cards(_pid, _op):
        return ["4138", "99999"] if case == "new_pack_reprint" else ["4138"]

    def update_hashes(db, cids, *args, **kwargs):
        added = 0
        for cid in cids:
            db.setdefault(cid, []).append("1" * 32)
            added += 1
        return len(cids), added, [], {"11"}

    with patch.object(updater, "_opener", return_value=object()), \
         patch.object(updater, "fetch_pack_list", side_effect=pack_list), \
         patch.object(updater, "fetch_pack_cards", side_effect=pack_cards), \
         patch.object(updater, "update_hashes", side_effect=update_hashes), \
         patch.object(updater.time, "sleep"):
        return updater.update_new_sets(catalog, cards, max_cards=0 if case == "max_cards_exceeded" else 4000)


def main():
    expected = json.loads(GOLDEN.read_text(encoding="utf-8"))
    actual = {case: run_case(case) for case in expected}
    for case in expected:
        assert actual[case] == expected[case], f"{case}: {actual[case]} != {expected[case]}"
    print(json.dumps({case: {"packs": value["packs"], "refreshed": value["refreshed"]}
                      for case, value in actual.items()}, ensure_ascii=False))


if __name__ == "__main__":
    main()
